//! Compressed storage form of the large native NBT rows (immutable baselines and
//! chunk metadata). Light arrays, heightmaps, biomes and block-state palettes are
//! highly repetitive; measured rows shrink 30-100x. Storage tuning only: the
//! decoded NBT, and so the world identity, is unchanged.
//!
//! A row is either plain named NBT (first byte is a tag type, `0x0A` for the root
//! compound) or `PACKED | u32 BE raw length | zstd frame` with a content checksum.
//! `0xF0` is not a valid tag type, so the two forms cannot be confused. Decoding
//! is fail-closed: any size, frame, checksum or length disagreement is an error.
use std::borrow::Cow;
use std::cell::RefCell;

use zstd::bulk::{Compressor, Decompressor};
use zstd::zstd_safe::CParameter;

use super::NativeError;

pub(super) const PACKED: u8 = 0xF0;
/// Level 3 is zstd's default speed/ratio point; level 9 measured about 10%
/// smaller baselines at several times the encode cost.
const LEVEL: i32 = 3;
/// Rows below this size are stored plain: the frame overhead would not pay off.
const MIN_PACKED: usize = 256;
const HEADER: usize = 5;

thread_local! {
    static COMPRESSOR: RefCell<Option<Compressor<'static>>> = const { RefCell::new(None) };
    static DECOMPRESSOR: RefCell<Option<Decompressor<'static>>> = const { RefCell::new(None) };
}

fn corrupt(reason: &'static str) -> NativeError {
    NativeError::Invalid(reason)
}

/// Compress a complete named-NBT row. Returns the input unchanged when packing
/// does not make it smaller. Deterministic for equal input, so stored rows can be
/// compared byte-for-byte with a freshly encoded capture.
pub(super) fn pack(raw: Vec<u8>) -> Result<Vec<u8>, NativeError> {
    if raw.len() < MIN_PACKED {
        return Ok(raw);
    }
    let length = u32::try_from(raw.len()).map_err(|_| corrupt("native record too large"))?;
    let frame = COMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let mut compressor = Compressor::new(LEVEL)?;
            compressor.set_parameter(CParameter::ChecksumFlag(true))?;
            *slot = Some(compressor);
        }
        slot.as_mut().expect("initialised").compress(&raw)
    })?;
    if HEADER + frame.len() >= raw.len() {
        return Ok(raw);
    }
    let mut bytes = Vec::with_capacity(HEADER + frame.len());
    bytes.push(PACKED);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(&frame);
    Ok(bytes)
}

pub(super) fn is_packed(bytes: &[u8]) -> bool {
    bytes.first() == Some(&PACKED)
}

/// The plain named-NBT bytes of a stored row, bounded by `limit`.
pub(super) fn unpack(bytes: &[u8], limit: usize) -> Result<Cow<'_, [u8]>, NativeError> {
    if !is_packed(bytes) {
        return Ok(Cow::Borrowed(bytes));
    }
    let (length, frame) = bytes[1..]
        .split_first_chunk::<4>()
        .map(|(length, frame)| (u32::from_be_bytes(*length) as usize, frame))
        .ok_or_else(|| corrupt("truncated packed native record"))?;
    if length > limit {
        return Err(corrupt("native record exceeds byte budget"));
    }
    let raw = DECOMPRESSOR
        .with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                *slot = Some(Decompressor::new()?);
            }
            slot.as_mut()
                .expect("initialised")
                .decompress(frame, length)
        })
        .map_err(|_| corrupt("invalid packed native record"))?;
    if raw.len() != length {
        return Err(corrupt("packed native record length mismatch"));
    }
    Ok(Cow::Owned(raw))
}
