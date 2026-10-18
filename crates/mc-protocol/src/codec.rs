//! Encode/decode primitives for the Minecraft Java Edition wire protocol.
//!
//! Reading is exposed via [`ReadMc`], an extension trait on `bytes::Buf`;
//! writing via [`WriteMc`], an extension trait on `bytes::BufMut`. This
//! lets callers reuse whatever buffer type is most convenient (`&[u8]`,
//! `bytes::Bytes`, `bytes::BytesMut`, `Vec<u8>`, …) without conversion.
//!
//! Two important properties:
//!
//! - **All-or-something semantics inside the trait methods.** When a
//!   method returns `Err`, the underlying buffer may have already advanced
//!   past some of the field's bytes — partial reads are not rolled back.
//!   This is fine for *post-framing* packet decoding (the framer hands us
//!   exactly one packet's worth of bytes; an error there means malformed
//!   input, and we bail on the connection anyway).
//! - **The framing layer uses [`read_varint_partial`] instead**, which
//!   peeks at a slice and reports "need more bytes" without consuming
//!   anything. That is the only place in the codebase that needs the
//!   peek-and-rewind dance.

use bytes::{Buf, BufMut};
use uuid::Uuid;

use crate::error::CodecError;

/// Maximum number of bytes a Minecraft VarInt may occupy on the wire.
pub const MAX_VARINT_BYTES: usize = 5;

/// Maximum number of bytes a Minecraft VarLong may occupy on the wire.
pub const MAX_VARLONG_BYTES: usize = 10;

/// VarInt-prefixed strings cap at this many UTF-16 code units in vanilla.
pub const DEFAULT_MAX_STRING_LEN: usize = 32_767;

/// Count-prefixed collections grow beyond this only after elements are decoded.
/// This prevents a valid large declared count from becoming an eager allocation.
const MAX_INITIAL_COLLECTION_CAPACITY: usize = 1_024;

fn utf16_code_units(s: &str) -> usize {
    s.encode_utf16().count()
}

// -----------------------------------------------------------------------
// VarInt / VarLong as standalone functions (used by the framing layer).
// -----------------------------------------------------------------------

/// Try to decode a VarInt out of `buf` without consuming any bytes.
///
/// Returns:
///
/// - `Ok(Some((value, bytes_read)))` on success. The caller is expected
///   to advance its own cursor by `bytes_read`.
/// - `Ok(None)` if the buffer does not yet contain a complete VarInt.
///   The caller should wait for more bytes and retry.
/// - `Err(CodecError::VarIntTooLong)` if the encoding is malformed.
///
/// This is the only varint API that distinguishes "incomplete" from
/// "malformed"; the trait-based [`ReadMc::read_varint`] folds both into
/// errors because by that point framing has guaranteed enough bytes.
pub fn read_varint_partial(buf: &[u8]) -> Result<Option<(i32, usize)>, CodecError> {
    let mut value: u32 = 0;
    for i in 0..MAX_VARINT_BYTES {
        let Some(&byte) = buf.get(i) else {
            return Ok(None);
        };
        value |= u32::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(Some((value as i32, i + 1)));
        }
    }
    Err(CodecError::VarIntTooLong)
}

/// Compute how many bytes [`WriteMc::write_varint`] will emit for `value`.
///
/// Used by the framing layer to size a length prefix without serialising
/// twice.
#[must_use]
pub const fn varint_encoded_len(value: i32) -> usize {
    let mut v = value as u32;
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// Same as [`varint_encoded_len`] but for VarLong.
#[must_use]
pub const fn varlong_encoded_len(value: i64) -> usize {
    let mut v = value as u64;
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

// -----------------------------------------------------------------------
// ReadMc: extension trait over bytes::Buf.
// -----------------------------------------------------------------------

/// Decoding side of the codec layer.
///
/// All methods consume bytes from the underlying buffer on success.
/// On error the buffer is in an unspecified state — callers should
/// discard the connection.
pub trait ReadMc: Buf {
    fn read_u8(&mut self) -> Result<u8, CodecError> {
        ensure_remaining(self, 1)?;
        Ok(self.get_u8())
    }

    fn read_i8(&mut self) -> Result<i8, CodecError> {
        ensure_remaining(self, 1)?;
        Ok(self.get_i8())
    }

    fn read_u16(&mut self) -> Result<u16, CodecError> {
        ensure_remaining(self, 2)?;
        Ok(self.get_u16())
    }

    fn read_i16(&mut self) -> Result<i16, CodecError> {
        ensure_remaining(self, 2)?;
        Ok(self.get_i16())
    }

    fn read_i32(&mut self) -> Result<i32, CodecError> {
        ensure_remaining(self, 4)?;
        Ok(self.get_i32())
    }

    fn read_u32(&mut self) -> Result<u32, CodecError> {
        ensure_remaining(self, 4)?;
        Ok(self.get_u32())
    }

    fn read_i64(&mut self) -> Result<i64, CodecError> {
        ensure_remaining(self, 8)?;
        Ok(self.get_i64())
    }

    fn read_u64(&mut self) -> Result<u64, CodecError> {
        ensure_remaining(self, 8)?;
        Ok(self.get_u64())
    }

    fn read_f32(&mut self) -> Result<f32, CodecError> {
        ensure_remaining(self, 4)?;
        Ok(self.get_f32())
    }

    fn read_f64(&mut self) -> Result<f64, CodecError> {
        ensure_remaining(self, 8)?;
        Ok(self.get_f64())
    }

    fn read_bool(&mut self) -> Result<bool, CodecError> {
        Ok(self.read_u8()? != 0)
    }

    fn read_varint(&mut self) -> Result<i32, CodecError> {
        let mut value: u32 = 0;
        for i in 0..MAX_VARINT_BYTES {
            let byte = self.read_u8()?;
            value |= u32::from(byte & 0x7F) << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(CodecError::VarIntTooLong)
    }

    fn read_varlong(&mut self) -> Result<i64, CodecError> {
        let mut value: u64 = 0;
        for i in 0..MAX_VARLONG_BYTES {
            let byte = self.read_u8()?;
            value |= u64::from(byte & 0x7F) << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(value as i64);
            }
        }
        Err(CodecError::VarLongTooLong)
    }

    /// Length-prefixed string. `max` is the maximum permitted number of
    /// UTF-16 code units, matching Java's `String.length()`.
    fn read_string(&mut self, max: usize) -> Result<String, CodecError> {
        let len_signed = self.read_varint()?;
        if len_signed < 0 {
            return Err(CodecError::NegativeLength(len_signed));
        }
        let len = len_signed as usize;
        // Netty bounds UTF-8 at three bytes per permitted UTF-16 code unit.
        // Rejecting the declared byte length here gates payload allocation.
        let max_bytes = max.saturating_mul(3);
        if len > max_bytes {
            return Err(CodecError::StringTooLong {
                len,
                max: max_bytes,
            });
        }
        ensure_remaining(self, len)?;
        let mut bytes = vec![0u8; len];
        self.copy_to_slice(&mut bytes);
        let s = String::from_utf8(bytes).map_err(|_| CodecError::InvalidUtf8)?;
        let code_units = utf16_code_units(&s);
        if code_units > max {
            return Err(CodecError::StringTooLong {
                len: code_units,
                max,
            });
        }
        Ok(s)
    }

    /// Reads a `namespace:path` identifier as a length-prefixed string and
    /// validates its shape.
    fn read_identifier(&mut self) -> Result<Identifier, CodecError> {
        let s = self.read_string(DEFAULT_MAX_STRING_LEN)?;
        Identifier::parse(s).map_err(|e| CodecError::InvalidIdentifier(e.0))
    }

    /// 128-bit big-endian UUID.
    fn read_uuid(&mut self) -> Result<Uuid, CodecError> {
        let mut bytes = [0u8; 16];
        ensure_remaining(self, 16)?;
        self.copy_to_slice(&mut bytes);
        Ok(Uuid::from_bytes(bytes))
    }

    /// Length-prefixed (VarInt) byte array.
    fn read_byte_array(&mut self, max: usize) -> Result<Vec<u8>, CodecError> {
        let len_signed = self.read_varint()?;
        if len_signed < 0 {
            return Err(CodecError::NegativeLength(len_signed));
        }
        let len = len_signed as usize;
        if len > max {
            return Err(CodecError::ArrayTooLong { len, max });
        }
        ensure_remaining(self, len)?;
        let mut bytes = vec![0u8; len];
        self.copy_to_slice(&mut bytes);
        Ok(bytes)
    }
}

impl<T: Buf + ?Sized> ReadMc for T {}

fn ensure_remaining<B: Buf + ?Sized>(buf: &B, needed: usize) -> Result<(), CodecError> {
    let available = buf.remaining();
    if available < needed {
        return Err(CodecError::Underflow {
            needed: needed - available,
            available,
        });
    }
    Ok(())
}

/// Decode one VarInt-counted collection with allocation and wire-feasibility fences.
///
/// `min_wire_size` is a conservative lower bound for one encoded element. The
/// body-size check runs before the first collection allocation or item callback.
/// Large valid collections begin with a bounded allocation and grow only as
/// their elements are successfully decoded.
pub(crate) fn read_bounded_count<B: Buf>(
    buf: &mut B,
    semantic_max: usize,
    min_wire_size: usize,
) -> Result<usize, CodecError> {
    assert!(
        min_wire_size > 0,
        "collection minimum wire size must be non-zero"
    );
    let count = buf.read_varint()?;
    if count < 0 {
        return Err(CodecError::NegativeLength(count));
    }
    let count = count as usize;
    if count > semantic_max {
        return Err(CodecError::StringTooLong {
            len: count,
            max: semantic_max,
        });
    }
    let available = buf.remaining();
    if count > available / min_wire_size {
        let minimum_body_bytes = count.saturating_mul(min_wire_size);
        return Err(CodecError::Underflow {
            needed: minimum_body_bytes.saturating_sub(available),
            available,
        });
    }
    Ok(count)
}

pub(crate) fn bounded_vec<T>(count: usize) -> Vec<T> {
    Vec::with_capacity(count.min(MAX_INITIAL_COLLECTION_CAPACITY))
}

pub(crate) fn read_bounded_vec<B, T, F>(
    buf: &mut B,
    semantic_max: usize,
    min_wire_size: usize,
    mut read_item: F,
) -> Result<Vec<T>, CodecError>
where
    B: Buf,
    F: FnMut(&mut B) -> Result<T, CodecError>,
{
    let count = read_bounded_count(buf, semantic_max, min_wire_size)?;
    let mut values = bounded_vec(count);
    for _ in 0..count {
        values.push(read_item(buf)?);
    }
    Ok(values)
}

// -----------------------------------------------------------------------
// WriteMc: extension trait over bytes::BufMut.
// -----------------------------------------------------------------------

/// Encoding side of the codec layer.
///
/// Methods that cannot fail return `()` directly; the only fallible
/// writers are the length-bounded ones (`write_string`, …).
pub trait WriteMc: BufMut {
    fn write_u8(&mut self, v: u8) {
        self.put_u8(v);
    }

    fn write_i8(&mut self, v: i8) {
        self.put_i8(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.put_u16(v);
    }

    fn write_i16(&mut self, v: i16) {
        self.put_i16(v);
    }

    fn write_i32(&mut self, v: i32) {
        self.put_i32(v);
    }

    fn write_u32(&mut self, v: u32) {
        self.put_u32(v);
    }

    fn write_i64(&mut self, v: i64) {
        self.put_i64(v);
    }

    fn write_u64(&mut self, v: u64) {
        self.put_u64(v);
    }

    fn write_f32(&mut self, v: f32) {
        self.put_f32(v);
    }

    fn write_f64(&mut self, v: f64) {
        self.put_f64(v);
    }

    fn write_bool(&mut self, v: bool) {
        self.put_u8(u8::from(v));
    }

    fn write_varint(&mut self, value: i32) {
        let mut v = value as u32;
        loop {
            if v & !0x7F == 0 {
                self.put_u8(v as u8);
                return;
            }
            self.put_u8((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }

    fn write_varlong(&mut self, value: i64) {
        let mut v = value as u64;
        loop {
            if v & !0x7F == 0 {
                self.put_u8(v as u8);
                return;
            }
            self.put_u8((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }

    fn write_string(&mut self, s: &str, max: usize) -> Result<(), CodecError> {
        let code_units = utf16_code_units(s);
        if code_units > max {
            return Err(CodecError::StringTooLong {
                len: code_units,
                max,
            });
        }
        self.write_varint(
            i32::try_from(s.len()).map_err(|_| CodecError::StringTooLong { len: s.len(), max })?,
        );
        self.put_slice(s.as_bytes());
        Ok(())
    }

    fn write_identifier(&mut self, id: &Identifier) -> Result<(), CodecError> {
        self.write_string(id.as_str(), DEFAULT_MAX_STRING_LEN)
    }

    fn write_uuid(&mut self, uuid: Uuid) {
        self.put_slice(uuid.as_bytes());
    }

    fn write_byte_array(&mut self, bytes: &[u8]) -> Result<(), CodecError> {
        let len = i32::try_from(bytes.len()).map_err(|_| CodecError::ArrayTooLong {
            len: bytes.len(),
            max: i32::MAX as usize,
        })?;
        self.write_varint(len);
        self.put_slice(bytes);
        Ok(())
    }
}

impl<T: BufMut + ?Sized> WriteMc for T {}

// -----------------------------------------------------------------------
// Identifier — re-exported from mc-data, where the type now lives.
// -----------------------------------------------------------------------

pub use mc_data::Identifier;

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
