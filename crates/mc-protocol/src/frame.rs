//! Packet framing: length-prefix on top of [`crate::codec`], optionally
//! with zlib compression once the per-connection threshold has been
//! negotiated.
//!
//! Layout on the wire **without compression** (one packet):
//!
//! ```text
//! [VarInt21 packet_length] [VarInt packet_id] [body bytes…]
//!                        \__________________________/
//!                         exactly packet_length bytes
//! ```
//!
//! Layout on the wire **with compression** (one packet, threshold T):
//!
//! ```text
//! [VarInt21 packet_length] [VarInt data_length] [packet body — see below]
//!                        \__________________________________________/
//!                         exactly packet_length bytes
//!
//!   data_length == 0   → body is raw   [VarInt packet_id][payload…]   (when uncompressed body
//!                                                                      length was below T)
//!   data_length >  0   → body is zlib-compressed bytes whose decompression
//!                        is exactly data_length long and contains
//!                        [VarInt packet_id][payload…]
//! ```
//!
//! Encryption (AES-128/CFB8) wraps the stream below framing and is owned by
//! the edge transport. This module deliberately deals only in plaintext bytes.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use flate2::Compression as ZlibLevel;
use flate2::write::ZlibEncoder;
use flate2::{Decompress, FlushDecompress, Status};
use std::io::Write;
use thiserror::Error;

use crate::codec::{ReadMc, WriteMc, varint_encoded_len};
use crate::error::CodecError;

/// Maximum value of the outer `packet_length` VarInt21.
pub const MAX_PACKET_LENGTH: usize = (1 << 21) - 1;

/// Maximum declared size of a decompressed packet, including its packet ID.
pub const MAX_DECOMPRESSED_BYTES: usize = 8 * 1024 * 1024;

/// One decoded frame: packet ID plus the body bytes that follow it.
///
/// `body` is a `Bytes` so callers can split it into the decoded packet
/// without copying. The bytes do **not** include the leading packet ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    pub id: i32,
    pub body: Bytes,
}

/// Compression configuration for the framing layer.
///
/// Negotiated during the Login state; vanilla servers normally enable it
/// with a threshold of 256 bytes via the `Set Compression` packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// No `Set Compression` packet has been sent yet. The frame on the
    /// wire has no `data_length` field.
    #[default]
    Disabled,
    /// `Set Compression` has been sent with the contained threshold.
    /// Bodies whose uncompressed length is below `threshold` are still
    /// transmitted as a `data_length = 0` plaintext frame; bodies at or
    /// above the threshold are zlib-compressed.
    Threshold(usize),
    /// Same wire shape as [`Compression::Threshold`], with an explicit
    /// zlib level for server-side frame encoding.
    ThresholdWithLevel { threshold: usize, level: u32 },
}

impl Compression {
    /// Returns `true` if this configuration adds a `data_length` field to
    /// every frame.
    #[must_use]
    pub const fn header_present(self) -> bool {
        matches!(self, Self::Threshold(_) | Self::ThresholdWithLevel { .. })
    }

    #[must_use]
    pub const fn threshold(self) -> Option<usize> {
        match self {
            Self::Disabled => None,
            Self::Threshold(threshold) | Self::ThresholdWithLevel { threshold, .. } => {
                Some(threshold)
            }
        }
    }

    #[must_use]
    pub const fn with_level(self, level: u32) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            Self::Threshold(threshold) | Self::ThresholdWithLevel { threshold, .. } => {
                Self::ThresholdWithLevel { threshold, level }
            }
        }
    }
}

/// Everything the framing layer can complain about.
#[derive(Debug, Error)]
pub enum FramingError {
    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    #[error("frame exceeds the applicable maximum of {max} bytes (got {got})")]
    FrameTooLarge { got: usize, max: usize },

    #[error("declared decompressed payload length is negative ({0})")]
    NegativeLength(i32),

    /// `data_length` claimed the uncompressed body would be this long, but
    /// a bounded decompression pass observed a different length.
    #[error(
        "compressed payload decompressed to at least {decompressed_at_least} bytes, header claimed {claimed}"
    )]
    DecompressedLengthMismatch {
        claimed: usize,
        decompressed_at_least: usize,
    },

    /// A frame had compression enabled and a positive `data_length`, but
    /// `data_length` was smaller than the per-connection threshold, which
    /// vanilla treats as a protocol violation (such packets must be sent
    /// uncompressed with `data_length = 0`).
    #[error("compressed frame declared data_length {claimed} below threshold {threshold}")]
    CompressedBelowThreshold { claimed: usize, threshold: usize },

    #[error("uncompressed frame contained {actual} bytes at or above threshold {threshold}")]
    UncompressedAboveThreshold { actual: usize, threshold: usize },

    #[error("compressed zlib stream did not reach its end marker")]
    IncompleteCompressedStream,

    #[error("compressed zlib stream left {trailing} trailing byte(s)")]
    TrailingCompressedData { trailing: usize },

    #[error("frame size arithmetic overflowed")]
    LengthOverflow,

    #[error("zlib decompression error: {0}")]
    ZlibDecompress(#[from] flate2::DecompressError),

    #[error("zlib I/O error: {0}")]
    Zlib(#[from] std::io::Error),
}

#[derive(Debug)]
pub enum FrameDecodePlan {
    Ready(RawFrame),
    Compressed(CompressedFrame),
}

#[derive(Debug)]
pub struct CompressedFrame {
    claimed_len: usize,
    payload: Bytes,
}

impl CompressedFrame {
    pub fn decode(self) -> Result<RawFrame, FramingError> {
        decode_compressed_frame(self.claimed_len, self.payload)
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

fn read_packet_length(buf: &[u8]) -> Result<Option<(usize, usize)>, CodecError> {
    let mut value = 0_usize;
    for index in 0..3 {
        let Some(&byte) = buf.get(index) else {
            return Ok(None);
        };
        value |= usize::from(byte & 0x7F) << (7 * index);
        if byte & 0x80 == 0 {
            return Ok(Some((value, index + 1)));
        }
    }
    Err(CodecError::VarIntTooLong)
}

/// Try to pull a single frame off the front of `buf`.
///
/// Returns:
///
/// - `Ok(Some(frame))` and advances `buf` past the consumed bytes.
/// - `Ok(None)` if `buf` does not yet contain a complete frame. `buf` is
///   left untouched so the caller can append more bytes and retry.
/// - `Err(_)` if the bytes that ARE there are malformed; the connection
///   should be dropped.
pub fn try_decode_frame_plan(
    buf: &mut BytesMut,
    compression: Compression,
) -> Result<Option<FrameDecodePlan>, FramingError> {
    let (packet_length, length_bytes) = match read_packet_length(buf)? {
        None => return Ok(None),
        Some(parts) => parts,
    };
    let complete_len = length_bytes
        .checked_add(packet_length)
        .ok_or(FramingError::LengthOverflow)?;
    if buf.len() < complete_len {
        return Ok(None);
    }

    buf.advance(length_bytes);
    let mut body = buf.split_to(packet_length).freeze();
    let plan = match compression {
        Compression::Disabled => {
            let id = body.read_varint()?;
            FrameDecodePlan::Ready(RawFrame { id, body })
        }
        Compression::Threshold(threshold) | Compression::ThresholdWithLevel { threshold, .. } => {
            let data_length_signed = body.read_varint()?;
            if data_length_signed < 0 {
                return Err(FramingError::NegativeLength(data_length_signed));
            }
            let data_length = data_length_signed as usize;
            if data_length == 0 {
                let actual = body.len();
                if actual >= threshold {
                    return Err(FramingError::UncompressedAboveThreshold { actual, threshold });
                }
                let id = body.read_varint()?;
                FrameDecodePlan::Ready(RawFrame { id, body })
            } else {
                if data_length < threshold {
                    return Err(FramingError::CompressedBelowThreshold {
                        claimed: data_length,
                        threshold,
                    });
                }
                if data_length > MAX_DECOMPRESSED_BYTES {
                    return Err(FramingError::FrameTooLarge {
                        got: data_length,
                        max: MAX_DECOMPRESSED_BYTES,
                    });
                }
                FrameDecodePlan::Compressed(CompressedFrame {
                    claimed_len: data_length,
                    payload: body,
                })
            }
        }
    };
    Ok(Some(plan))
}

fn decode_compressed_frame(claimed_len: usize, payload: Bytes) -> Result<RawFrame, FramingError> {
    let output_len = claimed_len
        .checked_add(1)
        .ok_or(FramingError::LengthOverflow)?;
    let mut decompressed = vec![0_u8; output_len];
    let mut decoder = Decompress::new(true);
    let status = decoder.decompress(
        payload.as_ref(),
        decompressed.as_mut_slice(),
        FlushDecompress::Finish,
    )?;
    let produced =
        usize::try_from(decoder.total_out()).map_err(|_| FramingError::LengthOverflow)?;
    if produced != claimed_len {
        return Err(FramingError::DecompressedLengthMismatch {
            claimed: claimed_len,
            decompressed_at_least: produced,
        });
    }
    if status != Status::StreamEnd {
        return Err(FramingError::IncompleteCompressedStream);
    }
    let consumed = usize::try_from(decoder.total_in()).map_err(|_| FramingError::LengthOverflow)?;
    if consumed != payload.len() {
        return Err(FramingError::TrailingCompressedData {
            trailing: payload.len() - consumed,
        });
    }

    decompressed.truncate(claimed_len);
    let mut body = Bytes::from(decompressed);
    let id = body.read_varint()?;
    Ok(RawFrame { id, body })
}

pub fn try_decode_frame(
    buf: &mut BytesMut,
    compression: Compression,
) -> Result<Option<RawFrame>, FramingError> {
    match try_decode_frame_plan(buf, compression)? {
        None => Ok(None),
        Some(FrameDecodePlan::Ready(frame)) => Ok(Some(frame)),
        Some(FrameDecodePlan::Compressed(frame)) => frame.decode().map(Some),
    }
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Serialise one frame onto the wire.
///
/// `body` is the packet payload (no leading packet ID — the encoder writes
/// it). The returned `Bytes` is the exact byte sequence to push into the
/// transport (or the encryption layer that wraps the transport).
pub fn encode_frame(id: i32, body: &[u8], compression: Compression) -> Result<Bytes, FramingError> {
    let mut out = BytesMut::new();
    append_frame(&mut out, id, body, compression)?;
    Ok(out.freeze())
}

/// Append a complete frame without allocating a separate output buffer.
/// Packet boundaries, compression, and errors are identical to [`encode_frame`].
pub fn append_frame(
    out: &mut BytesMut,
    id: i32,
    body: &[u8],
    compression: Compression,
) -> Result<(), FramingError> {
    let id_len = varint_encoded_len(id);
    let uncompressed_len = id_len + body.len();

    match compression {
        Compression::Disabled => {
            if uncompressed_len > MAX_PACKET_LENGTH {
                return Err(FramingError::FrameTooLarge {
                    got: uncompressed_len,
                    max: MAX_PACKET_LENGTH,
                });
            }
            out.reserve(varint_encoded_len(uncompressed_len as i32) + uncompressed_len);
            out.write_varint(uncompressed_len as i32);
            out.write_varint(id);
            out.put_slice(body);
            Ok(())
        }
        Compression::Threshold(threshold) | Compression::ThresholdWithLevel { threshold, .. } => {
            if uncompressed_len < threshold {
                // data_length = 0 sentinel; transmit plaintext.
                let inner_len = 1 /* data_length zero */ + uncompressed_len;
                if inner_len > MAX_PACKET_LENGTH {
                    return Err(FramingError::FrameTooLarge {
                        got: inner_len,
                        max: MAX_PACKET_LENGTH,
                    });
                }
                out.reserve(varint_encoded_len(inner_len as i32) + inner_len);
                out.write_varint(inner_len as i32);
                out.write_varint(0);
                out.write_varint(id);
                out.put_slice(body);
                Ok(())
            } else {
                if uncompressed_len > MAX_DECOMPRESSED_BYTES {
                    return Err(FramingError::FrameTooLarge {
                        got: uncompressed_len,
                        max: MAX_DECOMPRESSED_BYTES,
                    });
                }
                // Build [VarInt id][body] then compress it.
                let mut plain = Vec::with_capacity(uncompressed_len);
                plain.write_varint(id);
                plain.extend_from_slice(body);
                let level = match compression {
                    Compression::Threshold(_) => ZlibLevel::fast(),
                    Compression::ThresholdWithLevel { level, .. } => ZlibLevel::new(level.min(9)),
                    Compression::Disabled => unreachable!("compression branch already matched"),
                };
                let mut encoder = ZlibEncoder::new(Vec::new(), level);
                encoder.write_all(&plain)?;
                let compressed = encoder.finish()?;

                let data_length = plain.len();
                let inner_len = varint_encoded_len(data_length as i32) + compressed.len();
                if inner_len > MAX_PACKET_LENGTH {
                    return Err(FramingError::FrameTooLarge {
                        got: inner_len,
                        max: MAX_PACKET_LENGTH,
                    });
                }
                out.reserve(varint_encoded_len(inner_len as i32) + inner_len);
                out.write_varint(inner_len as i32);
                out.write_varint(data_length as i32);
                out.put_slice(&compressed);
                Ok(())
            }
        }
    }
}

/// Number of bytes [`encode_frame`] would emit for an uncompressed frame.
/// Useful for the writer to reserve buffer space without serialising twice.
pub fn encoded_size_uncompressed(id: i32, body_len: usize) -> Result<usize, FramingError> {
    let inner = varint_encoded_len(id)
        .checked_add(body_len)
        .ok_or(FramingError::LengthOverflow)?;
    if inner > MAX_PACKET_LENGTH {
        return Err(FramingError::FrameTooLarge {
            got: inner,
            max: MAX_PACKET_LENGTH,
        });
    }
    let inner_i32 = i32::try_from(inner).map_err(|_| FramingError::LengthOverflow)?;
    varint_encoded_len(inner_i32)
        .checked_add(inner)
        .ok_or(FramingError::LengthOverflow)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
