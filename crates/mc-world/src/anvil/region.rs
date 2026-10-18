//! Anvil `.mca` region-file binary layout.
//!
//! Per minecraft.wiki "Region file format":
//!
//! - bytes `0..4096` — locations table: 1024 entries, each 4 bytes
//!   big-endian, holding `(offset_in_sectors << 8) | sector_count`.
//!   Sector size is 4096. `0u32` means the chunk slot is empty.
//! - bytes `4096..8192` — timestamps table: 1024 `u32` BE values
//!   (epoch seconds of last save).
//! - bytes `8192..` — chunk data, 4096-aligned. Each chunk is
//!   `[len: i32 BE][compression: u8][payload …]` where `len` counts
//!   the compression byte plus the payload (so `payload_len = len - 1`).
//!
//! Compression types: 1 = gzip, 2 = zlib, 3 = uncompressed, 4 = LZ4.
//! The high bit (`0x80`) on the compression byte marks an *oversized*
//! chunk whose payload lives in a sibling `c.X.Z.mcc` file; we don't
//! emit that variant on write but read returns an error rather than
//! silently dropping it.
//!
//! Slot index for chunk-local `(cx, cz)` in `0..32`:
//!     idx = (cz as usize) * 32 + (cx as usize)
//! Locations table entry sits at `idx * 4`.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::hash::BuildHasher;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use flate2::bufread::GzDecoder;
use flate2::write::ZlibEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};
use thiserror::Error;

/// 32 × 32 chunks per region.
pub const CHUNKS_PER_REGION_AXIS: usize = 32;
const REGION_CHUNK_COUNT: usize = CHUNKS_PER_REGION_AXIS * CHUNKS_PER_REGION_AXIS;
const SECTOR_SIZE: usize = 4096;
const HEADER_SECTORS: usize = 2; // locations + timestamps
const HEADER_BYTES: usize = HEADER_SECTORS * SECTOR_SIZE;
const MAX_SECTORS_PER_CHUNK: usize = u8::MAX as usize;
const MAX_COMPRESSED_CHUNK_BYTES: usize = MAX_SECTORS_PER_CHUNK * SECTOR_SIZE - 5;
const MAX_REGION_FILE_BYTES: u64 =
    (HEADER_SECTORS as u64 + REGION_CHUNK_COUNT as u64 * u8::MAX as u64) * SECTOR_SIZE as u64;
const MAX_DECOMPRESSED_CHUNK_BYTES: usize = 64 * 1024 * 1024;
const MAX_DECOMPRESSED_REGION_BYTES: usize = 256 * 1024 * 1024;

const LZ4_BLOCK_MAGIC: &[u8; 8] = b"LZ4Block";
const LZ4_BLOCK_HEADER_LEN: usize = 21;
const LZ4_BLOCK_METHOD_RAW: u8 = 0x10;
const LZ4_BLOCK_METHOD_COMPRESSED: u8 = 0x20;
const LZ4_CHECKSUM_SEED: u32 = 0x9747_B28C;
const XXH32_PRIME1: u32 = 0x9E37_79B1;
const XXH32_PRIME2: u32 = 0x85EB_CA77;
const XXH32_PRIME3: u32 = 0xC2B2_AE3D;
const XXH32_PRIME4: u32 = 0x27D4_EB2F;
const XXH32_PRIME5: u32 = 0x1656_67B1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompressionType {
    Gzip = 1,
    Zlib = 2,
    Uncompressed = 3,
    Lz4 = 4,
}

impl CompressionType {
    fn from_byte(b: u8) -> Result<Self, RegionError> {
        match b {
            1 => Ok(Self::Gzip),
            2 => Ok(Self::Zlib),
            3 => Ok(Self::Uncompressed),
            4 => Ok(Self::Lz4),
            other => Err(RegionError::UnknownCompression(other)),
        }
    }
}

/// One chunk slot's full content.
#[derive(Debug, Clone)]
pub struct ChunkPayload {
    /// Local chunk x within the region, `0..32`.
    pub local_x: u8,
    /// Local chunk z within the region, `0..32`.
    pub local_z: u8,
    /// Epoch seconds from the timestamps table.
    pub timestamp: u32,
    /// Uncompressed Java-standard NBT bytes (a single named root
    /// compound, as `mc_nbt::read_named` accepts).
    pub uncompressed_nbt: Vec<u8>,
}

/// One chunk slot's verbatim on-disk record: compression byte, compressed
/// bytes and timestamp, exactly as stored. Region rewrites can copy these
/// bytes for untouched slots instead of decoding and recompressing them.
/// [`read_region_raw`] verifies each record decodes within budget (without
/// retaining the decoded bytes), so raw and decoded reads reject the same
/// corrupt inputs; only the allocating decode-and-retain step is skipped.
/// The stored codec and payload are immutable: a preserving rewrite reuses
/// their verified decoded length, while checking its new aggregate budget.
#[derive(Debug, Clone)]
pub struct RawChunkRecord {
    /// Local chunk x within the region, `0..32`.
    pub local_x: u8,
    /// Local chunk z within the region, `0..32`.
    pub local_z: u8,
    /// Epoch seconds from the timestamps table.
    pub timestamp: u32,
    /// Stored compression byte (`CompressionType` discriminant, high bit clear).
    compression: u8,
    /// Stored compressed payload bytes (without the 5-byte chunk header).
    compressed: Vec<u8>,
    validated_decoded_len: Option<usize>,
    validated_content_stamp: Option<u64>,
}

impl RawChunkRecord {
    #[must_use]
    pub fn compression(&self) -> u8 {
        self.compression
    }

    #[must_use]
    pub fn compressed(&self) -> &[u8] {
        &self.compressed
    }
}

/// One slot for a region rewrite: a freshly encoded payload or a verbatim
/// preserved record.
#[derive(Debug, Clone)]
pub enum RegionChunkOutput {
    /// Encode (zlib) and store with the payload's timestamp.
    Fresh(ChunkPayload),
    /// Copy stored bytes verbatim with the record's timestamp.
    Preserved(RawChunkRecord),
}

#[derive(Debug, Error)]
pub enum RegionError {
    #[error("region io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("region file too short: needs at least {needed} bytes, got {got}")]
    TooShort { needed: usize, got: usize },
    #[error("region file is {bytes} bytes, exceeding limit {max}")]
    RegionTooLarge { bytes: u64, max: u64 },
    #[error("region image size arithmetic overflow")]
    RegionSizeOverflow,
    #[error("chunk coordinates ({cx},{cz}) are outside the local region range 0..32")]
    InvalidChunkCoordinates { cx: u8, cz: u8 },
    #[error("chunk slot ({cx},{cz}) appears more than once in the write set")]
    DuplicateChunkSlot { cx: u8, cz: u8 },
    #[error(
        "chunk ({cx},{cz}) requires {sectors} sectors, exceeding the Anvil limit {max_sectors}"
    )]
    ChunkTooLarge {
        cx: u8,
        cz: u8,
        sectors: usize,
        max_sectors: usize,
    },
    #[error("region {path} chunk slot ({cx},{cz}) points into reserved header sector {sector}")]
    ReservedSector {
        path: PathBuf,
        cx: u8,
        cz: u8,
        sector: u32,
    },
    #[error(
        "region {path} chunk slot ({cx},{cz}) extent [{sector},{end_sector}) exceeds file length {file_bytes} bytes"
    )]
    SectorExtentOutOfRange {
        path: PathBuf,
        cx: u8,
        cz: u8,
        sector: u32,
        end_sector: u32,
        file_bytes: u64,
    },
    #[error(
        "region {path} chunk slot ({cx},{cz}) overlaps slot ({other_cx},{other_cz}) in sectors [{overlap_start},{overlap_end})"
    )]
    OverlappingChunkSectors {
        path: PathBuf,
        cx: u8,
        cz: u8,
        other_cx: u8,
        other_cz: u8,
        overlap_start: u32,
        overlap_end: u32,
    },
    #[error("region {path} chunk slot ({cx},{cz}) has nonzero offset but zero sector count")]
    ZeroSectorCount { path: PathBuf, cx: u8, cz: u8 },
    #[error(
        "chunk slot ({cx},{cz}) declared length {len} runs past its allocated {bytes_available} bytes"
    )]
    LengthOverrun {
        cx: u8,
        cz: u8,
        len: u32,
        bytes_available: usize,
    },
    #[error("unknown compression byte {0:#04x}")]
    UnknownCompression(u8),
    #[error("oversized chunks (.mcc sidecars) are not supported")]
    Oversized,
    #[error("decompression failed for chunk ({cx},{cz}): {source}")]
    Decompress {
        cx: u8,
        cz: u8,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "chunk ({cx},{cz}) decompressed payload is at least {bytes} bytes, exceeding limit {max}"
    )]
    DecompressedChunkTooLarge {
        cx: u8,
        cz: u8,
        bytes: usize,
        max: usize,
    },
    #[error("region decompressed payload total is {bytes} bytes, exceeding limit {max}")]
    DecompressedRegionTooLarge { bytes: usize, max: usize },
}

#[derive(Debug, Clone, Copy)]
struct RegionLimits {
    max_file_bytes: u64,
    max_chunk_bytes: usize,
    max_region_bytes: usize,
}

const DEFAULT_REGION_LIMITS: RegionLimits = RegionLimits {
    max_file_bytes: MAX_REGION_FILE_BYTES,
    max_chunk_bytes: MAX_DECOMPRESSED_CHUNK_BYTES,
    max_region_bytes: MAX_DECOMPRESSED_REGION_BYTES,
};

#[derive(Debug, Clone, Copy)]
struct ChunkLocation {
    sector: u32,
    count: u8,
}

/// Metadata only: every lookup still reads and stamps the stored payload and
/// rechecks its location, codec, sector extent and the current decode budgets.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SlotValidationProof {
    sector: u32,
    count: u8,
    compressed_len: usize,
    compression: u8,
    stamp: u64,
}

impl SlotValidationProof {
    fn new(location: ChunkLocation, compression: u8, compressed: &[u8]) -> Self {
        Self {
            sector: location.sector,
            count: location.count,
            compressed_len: compressed.len(),
            compression,
            stamp: compressed_content_stamp(compressed),
        }
    }
}

fn compressed_content_stamp(compressed: &[u8]) -> u64 {
    static STAMPER: LazyLock<std::collections::hash_map::RandomState> =
        LazyLock::new(std::collections::hash_map::RandomState::new);
    STAMPER.hash_one(compressed)
}

// Keep distinct in-flight byte snapshots' metadata rather than overwriting a
// newer proof when an older reader finishes. Synced writes replace obsolete
// generations with the complete output set; no payload bytes are cached.
type RegionValidationMemo = HashMap<(usize, SlotValidationProof), usize>;
type SharedRegionValidationMemo = Arc<Mutex<RegionValidationMemo>>;
const MAX_REGION_VALIDATION_MEMOS: usize = 256;
const MAX_SLOT_VALIDATION_PROOFS: usize = REGION_CHUNK_COUNT;

fn remember_slot_validation(
    memo: &mut RegionValidationMemo,
    slot: usize,
    proof: SlotValidationProof,
    decoded_len: usize,
) {
    let key = (slot, proof);
    if let Some(previous) = memo.get_mut(&key) {
        *previous = decoded_len;
        return;
    }
    // A miss only repeats validation. Bound obsolete in-flight generations
    // as well as the process registry, without retaining payload bytes.
    if memo.len() == MAX_SLOT_VALIDATION_PROOFS {
        memo.clear();
    }
    memo.insert(key, decoded_len);
}

static REGION_VALIDATION_MEMOS: LazyLock<Mutex<HashMap<PathBuf, SharedRegionValidationMemo>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn region_validation_memo(path: &Path) -> SharedRegionValidationMemo {
    let mut memos = REGION_VALIDATION_MEMOS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(memo) = memos.get(path) {
        return Arc::clone(memo);
    }
    if memos.len() == MAX_REGION_VALIDATION_MEMOS {
        memos.clear();
    }
    let memo = Arc::new(Mutex::new(HashMap::new()));
    memos.insert(path.to_path_buf(), Arc::clone(&memo));
    memo
}

/// A validated region index and open file, with no retained decoded payloads.
/// Reads seek directly to one slot; callers must discard the reader after
/// replacing the region file so the index and file handle stay current.
pub(crate) struct RegionReader {
    path: PathBuf,
    file: Mutex<File>,
    header: [u8; HEADER_BYTES],
    locations: [Option<ChunkLocation>; REGION_CHUNK_COUNT],
    validation_memo: SharedRegionValidationMemo,
}

impl RegionReader {
    pub(crate) fn open(path: &Path) -> Result<Self, RegionError> {
        Self::open_with_limits(path, DEFAULT_REGION_LIMITS)
    }

    fn open_with_limits(path: &Path, limits: RegionLimits) -> Result<Self, RegionError> {
        let mut file = File::open(path).map_err(|source| RegionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let file_len = file
            .metadata()
            .map_err(|source| RegionError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .len();
        if file_len > limits.max_file_bytes {
            return Err(RegionError::RegionTooLarge {
                bytes: file_len,
                max: limits.max_file_bytes,
            });
        }
        // Vanilla lazily creates region files before the first chunk write;
        // a zero-byte file is empty, unlike a partially written header.
        if file_len != 0 && file_len < HEADER_BYTES as u64 {
            return Err(RegionError::TooShort {
                needed: HEADER_BYTES,
                got: file_len as usize,
            });
        }

        let mut header = [0_u8; HEADER_BYTES];
        if file_len != 0 {
            file.read_exact(&mut header)
                .map_err(|source| RegionError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
        }
        let locations = validate_location_table(path, &header, file_len)?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
            header,
            locations,
            validation_memo: region_validation_memo(path),
        })
    }

    fn read_slot(
        &self,
        slot: usize,
        decoded_so_far: usize,
        limits: RegionLimits,
    ) -> Result<Option<ChunkPayload>, RegionError> {
        let Some(record) = self.read_slot_raw(slot)? else {
            return Ok(None);
        };
        let uncompressed_nbt = decompress_bounded(
            CompressionType::from_byte(record.compression)?,
            &record.compressed,
            record.local_x,
            record.local_z,
            decoded_so_far,
            limits,
        )?;
        let proof = SlotValidationProof::new(
            self.locations[slot].expect("read populated slot"),
            record.compression,
            &record.compressed,
        );
        remember_slot_validation(
            &mut self
                .validation_memo
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            slot,
            proof,
            uncompressed_nbt.len(),
        );
        Ok(Some(ChunkPayload {
            local_x: record.local_x,
            local_z: record.local_z,
            timestamp: record.timestamp,
            uncompressed_nbt,
        }))
    }

    /// Read one slot's stored bytes without decoding them. Location-table,
    /// compression-type and size validation match [`Self::read_slot`]; the
    /// caller applies [`count_slot_payload`] when it needs the same
    /// decode-budget fence without retaining decoded bytes.
    fn read_slot_raw(&self, slot: usize) -> Result<Option<RawChunkRecord>, RegionError> {
        let cx = (slot % CHUNKS_PER_REGION_AXIS) as u8;
        let cz = (slot / CHUNKS_PER_REGION_AXIS) as u8;
        let Some(location) = self.locations[slot] else {
            return Ok(None);
        };
        let start = u64::from(location.sector) * SECTOR_SIZE as u64;
        let bytes_available = usize::from(location.count) * SECTOR_SIZE;

        // A shared reader serializes file-position changes, not decompression.
        // Every read seeks anew, so a previous failed read cannot misalign it.
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        file.seek(SeekFrom::Start(start))
            .map_err(|source| RegionError::Io {
                path: self.path.clone(),
                source,
            })?;
        let mut chunk_header = [0_u8; 5];
        file.read_exact(&mut chunk_header)
            .map_err(|source| RegionError::Io {
                path: self.path.clone(),
                source,
            })?;
        let len = u32::from_be_bytes(chunk_header[..4].try_into().expect("4-byte slice"));
        let comp_byte = chunk_header[4];
        if comp_byte & 0x80 != 0 {
            return Err(RegionError::Oversized);
        }
        // Reject unknown compression ids here so preserved records can never
        // smuggle an unreadable byte through a rewrite.
        CompressionType::from_byte(comp_byte)?;
        let payload_len = usize::try_from(len)
            .expect("u32 chunk length fits usize")
            .checked_sub(1)
            .ok_or(RegionError::LengthOverrun {
                cx,
                cz,
                len,
                bytes_available,
            })?;
        let encoded_len = 5_usize
            .checked_add(payload_len)
            .ok_or(RegionError::LengthOverrun {
                cx,
                cz,
                len,
                bytes_available,
            })?;
        if encoded_len > bytes_available {
            return Err(RegionError::LengthOverrun {
                cx,
                cz,
                len,
                bytes_available,
            });
        }

        let mut compressed =
            allocate_exact_bytes(payload_len, "compressed Anvil chunk").map_err(|source| {
                RegionError::Io {
                    path: self.path.clone(),
                    source,
                }
            })?;
        file.read_exact(&mut compressed)
            .map_err(|source| RegionError::Io {
                path: self.path.clone(),
                source,
            })?;
        drop(file);

        let ts_off = SECTOR_SIZE + slot * 4;
        let timestamp = u32::from_be_bytes(
            self.header[ts_off..ts_off + 4]
                .try_into()
                .expect("4-byte slice"),
        );
        Ok(Some(RawChunkRecord {
            local_x: cx,
            local_z: cz,
            timestamp,
            compression: comp_byte,
            compressed,
            validated_decoded_len: None,
            validated_content_stamp: None,
        }))
    }
}

fn validate_location_table(
    path: &Path,
    header: &[u8; HEADER_BYTES],
    file_len: u64,
) -> Result<[Option<ChunkLocation>; REGION_CHUNK_COUNT], RegionError> {
    let mut locations: [Option<ChunkLocation>; REGION_CHUNK_COUNT] = [None; REGION_CHUNK_COUNT];
    for slot in 0..REGION_CHUNK_COUNT {
        let cx = (slot % CHUNKS_PER_REGION_AXIS) as u8;
        let cz = (slot / CHUNKS_PER_REGION_AXIS) as u8;
        let loc_off = slot * 4;
        let loc = u32::from_be_bytes(
            header[loc_off..loc_off + 4]
                .try_into()
                .expect("4-byte location entry"),
        );
        let sector = loc >> 8;
        let count = (loc & 0xFF) as u8;
        if sector == 0 && count == 0 {
            continue;
        }
        if count == 0 {
            return Err(RegionError::ZeroSectorCount {
                path: path.to_path_buf(),
                cx,
                cz,
            });
        }
        if sector < HEADER_SECTORS as u32 {
            return Err(RegionError::ReservedSector {
                path: path.to_path_buf(),
                cx,
                cz,
                sector,
            });
        }

        let end_sector = sector
            .checked_add(u32::from(count))
            .ok_or(RegionError::RegionSizeOverflow)?;
        let extent_bytes = u64::from(end_sector)
            .checked_mul(SECTOR_SIZE as u64)
            .ok_or(RegionError::RegionSizeOverflow)?;
        if extent_bytes > file_len {
            return Err(RegionError::SectorExtentOutOfRange {
                path: path.to_path_buf(),
                cx,
                cz,
                sector,
                end_sector,
                file_bytes: file_len,
            });
        }

        for (other_slot, other) in locations[..slot].iter().enumerate() {
            let Some(other) = other else {
                continue;
            };
            let other_end = other.sector + u32::from(other.count);
            let overlap_start = sector.max(other.sector);
            let overlap_end = end_sector.min(other_end);
            if overlap_start < overlap_end {
                return Err(RegionError::OverlappingChunkSectors {
                    path: path.to_path_buf(),
                    cx,
                    cz,
                    other_cx: (other_slot % CHUNKS_PER_REGION_AXIS) as u8,
                    other_cz: (other_slot / CHUNKS_PER_REGION_AXIS) as u8,
                    overlap_start,
                    overlap_end,
                });
            }
        }

        locations[slot] = Some(ChunkLocation { sector, count });
    }
    Ok(locations)
}

/// Read every populated chunk out of a region file. Empty slots are
/// silently skipped. Returns chunks in slot-order
/// `(cz * 32 + cx)`.
pub fn read_region(path: impl AsRef<Path>) -> Result<Vec<ChunkPayload>, RegionError> {
    let path = path.as_ref();
    let mut out = Vec::new();
    out.try_reserve_exact(REGION_CHUNK_COUNT)
        .map_err(|error| RegionError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(format!("reserve region chunk list: {error}")),
        })?;
    read_region_with_limits(path, DEFAULT_REGION_LIMITS, |payload| out.push(payload))?;
    Ok(out)
}

/// Visit every populated chunk without retaining all decoded payloads.
/// The same per-chunk and aggregate decode budgets as [`read_region`]
/// are enforced before each output allocation.
pub fn visit_region(
    path: impl AsRef<Path>,
    visitor: impl FnMut(ChunkPayload),
) -> Result<(), RegionError> {
    read_region_with_limits(path.as_ref(), DEFAULT_REGION_LIMITS, visitor)
}

/// Read one chunk slot without inflating the rest of the region.
pub fn read_chunk(
    path: impl AsRef<Path>,
    local_x: u8,
    local_z: u8,
) -> Result<Option<ChunkPayload>, RegionError> {
    let slot = chunk_slot(local_x, local_z)?;
    RegionReader::open(path.as_ref())?.read_slot(slot, 0, DEFAULT_REGION_LIMITS)
}

fn chunk_slot(local_x: u8, local_z: u8) -> Result<usize, RegionError> {
    if usize::from(local_x) >= CHUNKS_PER_REGION_AXIS
        || usize::from(local_z) >= CHUNKS_PER_REGION_AXIS
    {
        return Err(RegionError::InvalidChunkCoordinates {
            cx: local_x,
            cz: local_z,
        });
    }
    Ok(usize::from(local_z) * CHUNKS_PER_REGION_AXIS + usize::from(local_x))
}

fn read_region_with_limits(
    path: &Path,
    limits: RegionLimits,
    mut visitor: impl FnMut(ChunkPayload),
) -> Result<(), RegionError> {
    let reader = RegionReader::open_with_limits(path, limits)?;
    let mut decoded_total = 0_usize;
    for slot in 0..REGION_CHUNK_COUNT {
        if let Some(payload) = reader.read_slot(slot, decoded_total, limits)? {
            decoded_total = decoded_total
                .checked_add(payload.uncompressed_nbt.len())
                .expect("aggregate limit prevents decoded byte overflow");
            visitor(payload);
        }
    }
    Ok(())
}

/// Read and validate every populated chunk's stored bytes, in slot-order
/// `(cz * 32 + cx)`. Enforces the same location, compression and decode budgets
/// as [`read_region`], without retaining decoded NBT. Immutable records carry
/// their verified decoded lengths into a preserving rewrite.
pub fn read_region_raw(path: impl AsRef<Path>) -> Result<Vec<RawChunkRecord>, RegionError> {
    read_region_raw_with_limits(path.as_ref(), DEFAULT_REGION_LIMITS)
}

fn read_region_raw_with_limits(
    path: &Path,
    limits: RegionLimits,
) -> Result<Vec<RawChunkRecord>, RegionError> {
    let mut out = Vec::new();
    out.try_reserve_exact(REGION_CHUNK_COUNT)
        .map_err(|error| RegionError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(format!("reserve region chunk list: {error}")),
        })?;
    let reader = RegionReader::open_with_limits(path, limits)?;
    let mut decoded_total = 0_usize;
    for slot in 0..REGION_CHUNK_COUNT {
        if let Some(mut record) = reader.read_slot_raw(slot)? {
            let comp = CompressionType::from_byte(record.compression)?;
            let proof = SlotValidationProof::new(
                reader.locations[slot].expect("read populated slot"),
                record.compression,
                &record.compressed,
            );
            // Only this region is locked during a cache miss's bounded count.
            // Concurrent preserving reads therefore validate unchanged bytes
            // once, while unrelated regions and all file reads remain free.
            let mut memo = reader
                .validation_memo
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let cached_len = memo
                .get(&(slot, proof))
                .copied()
                .filter(|&len| len <= limits.max_chunk_bytes);
            let decoded_len = if let Some(len) = cached_len {
                let aggregate = decoded_total.saturating_add(len);
                if aggregate > limits.max_region_bytes {
                    return Err(RegionError::DecompressedRegionTooLarge {
                        bytes: aggregate,
                        max: limits.max_region_bytes,
                    });
                }
                len
            } else {
                let len = count_slot_payload(
                    comp,
                    &record.compressed,
                    record.local_x,
                    record.local_z,
                    decoded_total,
                    limits,
                )?;
                remember_slot_validation(&mut memo, slot, proof, len);
                len
            };
            drop(memo);
            decoded_total = decoded_total
                .checked_add(decoded_len)
                .expect("aggregate limit prevents decoded byte overflow");
            record.validated_decoded_len = Some(decoded_len);
            record.validated_content_stamp = Some(proof.stamp);
            out.push(record);
        }
    }
    Ok(out)
}

/// Write a fresh region file at `path`, packing each chunk with zlib.
/// Slots not represented in `chunks` are left empty (`(0, 0)` in the
/// locations table).
pub fn write_region(path: impl AsRef<Path>, chunks: &[ChunkPayload]) -> Result<(), RegionError> {
    write_region_with_options(path.as_ref(), chunks, false)
}

/// Write a fresh region file at `path`, failing if it already exists.
pub fn write_region_create_new(
    path: impl AsRef<Path>,
    chunks: &[ChunkPayload],
) -> Result<(), RegionError> {
    write_region_with_options(path.as_ref(), chunks, true)
}

fn write_region_with_options(
    path: &Path,
    chunks: &[ChunkPayload],
    create_new: bool,
) -> Result<(), RegionError> {
    validate_write_chunks(chunks, DEFAULT_REGION_LIMITS)?;

    let mut planned = Vec::with_capacity(chunks.len());
    let mut zlib = None;

    for c in chunks {
        let slot = usize::from(c.local_z) * CHUNKS_PER_REGION_AXIS + usize::from(c.local_x);
        let compressed = encode_fresh_slot(path, &c.uncompressed_nbt, &mut zlib)?;
        let (len, padded_len, sectors) =
            encoded_chunk_layout(compressed.len(), c.local_x, c.local_z)?;
        planned.push(PlannedSlot {
            slot,
            timestamp: c.timestamp,
            compression: CompressionType::Zlib as u8,
            compressed: std::borrow::Cow::Owned(compressed),
            decoded_len: c.uncompressed_nbt.len(),
            len,
            padded_len,
            sectors,
            content_stamp: None,
        });
    }

    write_assembled_region(path, &planned, create_new)
}

/// Write a fresh region file at `path`, failing if it already exists, copying
/// [`RegionChunkOutput::Preserved`] slots verbatim and encoding `Fresh` ones
/// with zlib. Slots not represented are left empty. Byte-identical preserved
/// records keep their timestamp, compression byte and compressed bytes, so a
/// rewrite that only replaces dirty slots never recompresses the rest.
pub fn write_region_create_new_mixed(
    path: impl AsRef<Path>,
    records: &[RegionChunkOutput],
) -> Result<(), RegionError> {
    let path = path.as_ref();
    let decoded_lengths = validate_write_records(records, DEFAULT_REGION_LIMITS)?;

    let mut planned = Vec::with_capacity(records.len());
    let mut zlib = None;

    for record in records {
        let (local_x, local_z, timestamp, comp_byte, compressed) = match record {
            RegionChunkOutput::Fresh(payload) => (
                payload.local_x,
                payload.local_z,
                payload.timestamp,
                CompressionType::Zlib as u8,
                std::borrow::Cow::Owned(encode_fresh_slot(
                    path,
                    &payload.uncompressed_nbt,
                    &mut zlib,
                )?),
            ),
            RegionChunkOutput::Preserved(record) => (
                record.local_x,
                record.local_z,
                record.timestamp,
                record.compression,
                std::borrow::Cow::Borrowed(record.compressed.as_slice()),
            ),
        };
        let slot = usize::from(local_z) * CHUNKS_PER_REGION_AXIS + usize::from(local_x);
        let (len, padded_len, sectors) = encoded_chunk_layout(compressed.len(), local_x, local_z)?;
        planned.push(PlannedSlot {
            slot,
            timestamp,
            compression: comp_byte,
            compressed,
            decoded_len: decoded_lengths[slot],
            len,
            padded_len,
            sectors,
            content_stamp: match record {
                RegionChunkOutput::Fresh(_) => None,
                RegionChunkOutput::Preserved(record) => record.validated_content_stamp,
            },
        });
    }

    write_assembled_region(path, &planned, true)
}

fn encode_fresh_slot(
    path: &Path,
    uncompressed_nbt: &[u8],
    zlib: &mut Option<ZlibEncoder<Vec<u8>>>,
) -> Result<Vec<u8>, RegionError> {
    // Each slot is an independent zlib stream. Reset keeps the compressor's
    // large workspace, but finishes the old stream before starting the next.
    let zlib = zlib.get_or_insert_with(|| ZlibEncoder::new(Vec::new(), Compression::default()));
    zlib.write_all(uncompressed_nbt)
        .map_err(|e| RegionError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    zlib.reset(Vec::new()).map_err(|e| RegionError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Fresh streams are encoded once during planning; preserved streams remain
/// borrowed until their single append into the final image.
struct PlannedSlot<'a> {
    slot: usize,
    timestamp: u32,
    compression: u8,
    compressed: std::borrow::Cow<'a, [u8]>,
    decoded_len: usize,
    len: u32,
    padded_len: usize,
    sectors: u32,
    content_stamp: Option<u64>,
}

fn write_assembled_region(
    path: &Path,
    planned: &[PlannedSlot<'_>],
    create_new: bool,
) -> Result<(), RegionError> {
    let out_len = planned.iter().try_fold(HEADER_BYTES, |total, record| {
        total
            .checked_add(record.padded_len)
            .ok_or(RegionError::RegionSizeOverflow)
    })?;
    if out_len as u64 > MAX_REGION_FILE_BYTES {
        return Err(RegionError::RegionTooLarge {
            bytes: out_len as u64,
            max: MAX_REGION_FILE_BYTES,
        });
    }
    // Reserve the final exact size before appending any retained bytes. There
    // is no geometric-growth prefix copy and no separate body-to-image copy.
    let mut out = Vec::new();
    out.try_reserve_exact(out_len)
        .map_err(|error| RegionError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(format!(
                "reserve region image buffer by {out_len} bytes: {error}"
            )),
        })?;
    out.resize(HEADER_BYTES, 0);
    let mut proofs = HashMap::with_capacity(planned.len());
    for record in planned {
        let sector = (out.len() / SECTOR_SIZE) as u32;
        debug_assert!(sector < (1 << 24));
        let location = (sector << 8) | record.sectors;
        let loc_off = record.slot * 4;
        out[loc_off..loc_off + 4].copy_from_slice(&location.to_be_bytes());
        let ts_off = SECTOR_SIZE + loc_off;
        out[ts_off..ts_off + 4].copy_from_slice(&record.timestamp.to_be_bytes());
        let end = out.len() + record.padded_len;
        out.extend_from_slice(&record.len.to_be_bytes());
        out.push(record.compression);
        out.extend_from_slice(&record.compressed);
        out.resize(end, 0);
        let proof = SlotValidationProof {
            sector,
            count: record.sectors as u8,
            compressed_len: record.compressed.len(),
            compression: record.compression,
            stamp: record
                .content_stamp
                .unwrap_or_else(|| compressed_content_stamp(&record.compressed)),
        };
        proofs.insert((record.slot, proof), record.decoded_len);
    }
    debug_assert_eq!(out.len(), out_len);

    let mut file = if create_new {
        OpenOptions::new().write(true).create_new(true).open(path)
    } else {
        File::create(path)
    }
    .map_err(|e| RegionError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    file.write_all(&out).map_err(|e| RegionError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    file.sync_all().map_err(|e| RegionError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    // Failed writes/syncs must never publish proofs. A temporary image's
    // proofs remain under its own path until its caller installs it.
    *region_validation_memo(path)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = proofs;
    Ok(())
}

fn validate_write_chunks(chunks: &[ChunkPayload], limits: RegionLimits) -> Result<(), RegionError> {
    let mut seen = [false; REGION_CHUNK_COUNT];
    let mut decoded_total = 0_usize;
    for chunk in chunks {
        if usize::from(chunk.local_x) >= CHUNKS_PER_REGION_AXIS
            || usize::from(chunk.local_z) >= CHUNKS_PER_REGION_AXIS
        {
            return Err(RegionError::InvalidChunkCoordinates {
                cx: chunk.local_x,
                cz: chunk.local_z,
            });
        }
        let slot = usize::from(chunk.local_z) * CHUNKS_PER_REGION_AXIS + usize::from(chunk.local_x);
        if std::mem::replace(&mut seen[slot], true) {
            return Err(RegionError::DuplicateChunkSlot {
                cx: chunk.local_x,
                cz: chunk.local_z,
            });
        }
        if chunk.uncompressed_nbt.len() > limits.max_chunk_bytes {
            return Err(RegionError::DecompressedChunkTooLarge {
                cx: chunk.local_x,
                cz: chunk.local_z,
                bytes: chunk.uncompressed_nbt.len(),
                max: limits.max_chunk_bytes,
            });
        }
        decoded_total = decoded_total.saturating_add(chunk.uncompressed_nbt.len());
        if decoded_total > limits.max_region_bytes {
            return Err(RegionError::DecompressedRegionTooLarge {
                bytes: decoded_total,
                max: limits.max_region_bytes,
            });
        }
    }
    Ok(())
}

fn validate_write_records(
    records: &[RegionChunkOutput],
    limits: RegionLimits,
) -> Result<[usize; REGION_CHUNK_COUNT], RegionError> {
    let mut seen = [false; REGION_CHUNK_COUNT];
    let mut decoded_lengths = [0; REGION_CHUNK_COUNT];
    let mut decoded_total = 0_usize;
    for record in records {
        let (local_x, local_z) = match record {
            RegionChunkOutput::Fresh(payload) => (payload.local_x, payload.local_z),
            RegionChunkOutput::Preserved(record) => (record.local_x, record.local_z),
        };
        if usize::from(local_x) >= CHUNKS_PER_REGION_AXIS
            || usize::from(local_z) >= CHUNKS_PER_REGION_AXIS
        {
            return Err(RegionError::InvalidChunkCoordinates {
                cx: local_x,
                cz: local_z,
            });
        }
        let slot = usize::from(local_z) * CHUNKS_PER_REGION_AXIS + usize::from(local_x);
        if std::mem::replace(&mut seen[slot], true) {
            return Err(RegionError::DuplicateChunkSlot {
                cx: local_x,
                cz: local_z,
            });
        }
        match record {
            RegionChunkOutput::Fresh(payload) => {
                if payload.uncompressed_nbt.len() > limits.max_chunk_bytes {
                    return Err(RegionError::DecompressedChunkTooLarge {
                        cx: local_x,
                        cz: local_z,
                        bytes: payload.uncompressed_nbt.len(),
                        max: limits.max_chunk_bytes,
                    });
                }
                decoded_lengths[slot] = payload.uncompressed_nbt.len();
                decoded_total = decoded_total.saturating_add(payload.uncompressed_nbt.len());
                if decoded_total > limits.max_region_bytes {
                    return Err(RegionError::DecompressedRegionTooLarge {
                        bytes: decoded_total,
                        max: limits.max_region_bytes,
                    });
                }
            }
            RegionChunkOutput::Preserved(record) => {
                if record.compression & 0x80 != 0 {
                    return Err(RegionError::Oversized);
                }
                let comp = CompressionType::from_byte(record.compression)?;
                // Sector/size limits for the stored bytes.
                encoded_chunk_layout(record.compressed.len(), local_x, local_z)?;
                // Raw reads already validated these immutable bytes. Recheck
                // smaller chunk limits and the new mixed aggregate, not the
                // same compressed stream a second time.
                let decoded_len = match record.validated_decoded_len {
                    Some(len) if len <= limits.max_chunk_bytes => len,
                    _ => count_slot_payload(
                        comp,
                        &record.compressed,
                        local_x,
                        local_z,
                        decoded_total,
                        limits,
                    )?,
                };
                decoded_lengths[slot] = decoded_len;
                decoded_total = decoded_total.saturating_add(decoded_len);
                if decoded_total > limits.max_region_bytes {
                    return Err(RegionError::DecompressedRegionTooLarge {
                        bytes: decoded_total,
                        max: limits.max_region_bytes,
                    });
                }
            }
        }
    }
    Ok(decoded_lengths)
}

fn encoded_chunk_layout(
    compressed_len: usize,
    cx: u8,
    cz: u8,
) -> Result<(u32, usize, u32), RegionError> {
    let sectors = compressed_len.saturating_add(5).div_ceil(SECTOR_SIZE);
    if compressed_len > MAX_COMPRESSED_CHUNK_BYTES {
        return Err(RegionError::ChunkTooLarge {
            cx,
            cz,
            sectors,
            max_sectors: MAX_SECTORS_PER_CHUNK,
        });
    }
    let len = u32::try_from(compressed_len + 1)
        .expect("a chunk within the 255-sector limit has a u32 payload length");
    let padded_len = sectors * SECTOR_SIZE;
    let sectors = u32::try_from(sectors).expect("a bounded sector count fits u32");
    Ok((len, padded_len, sectors))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundedLength {
    Exact(usize),
    TooLarge { at_least: usize },
}

fn decompress_bounded(
    comp: CompressionType,
    payload: &[u8],
    cx: u8,
    cz: u8,
    decoded_so_far: usize,
    limits: RegionLimits,
) -> Result<Vec<u8>, RegionError> {
    let decoded_len = count_slot_payload(comp, payload, cx, cz, decoded_so_far, limits)?;
    decompress_exact(comp, payload, decoded_len).map_err(|source| RegionError::Decompress {
        cx,
        cz,
        source,
    })
}

/// Verify a stored payload decodes within budget without retaining the
/// decoded bytes. Same fence as the counting half of [`decompress_bounded`];
/// shared by decoded reads and raw (preserving) reads so both reject the
/// same corrupt inputs.
fn count_slot_payload(
    comp: CompressionType,
    payload: &[u8],
    cx: u8,
    cz: u8,
    decoded_so_far: usize,
    limits: RegionLimits,
) -> Result<usize, RegionError> {
    let decoded_len = match count_decoded_bytes(comp, payload, limits.max_chunk_bytes)
        .map_err(|source| RegionError::Decompress { cx, cz, source })?
    {
        BoundedLength::Exact(len) => len,
        BoundedLength::TooLarge { at_least } => {
            return Err(RegionError::DecompressedChunkTooLarge {
                cx,
                cz,
                bytes: at_least,
                max: limits.max_chunk_bytes,
            });
        }
    };
    let aggregate = decoded_so_far.saturating_add(decoded_len);
    if aggregate > limits.max_region_bytes {
        return Err(RegionError::DecompressedRegionTooLarge {
            bytes: aggregate,
            max: limits.max_region_bytes,
        });
    }
    Ok(decoded_len)
}

fn count_decoded_bytes(
    comp: CompressionType,
    payload: &[u8],
    max: usize,
) -> Result<BoundedLength, std::io::Error> {
    match comp {
        CompressionType::Uncompressed => Ok(if payload.len() > max {
            BoundedLength::TooLarge {
                at_least: payload.len(),
            }
        } else {
            BoundedLength::Exact(payload.len())
        }),
        CompressionType::Zlib => count_zlib_bytes(payload, max),
        CompressionType::Gzip => count_gzip_bytes(payload, max),
        CompressionType::Lz4 => count_lz4_bytes(payload, max),
    }
}

fn count_zlib_bytes(payload: &[u8], max: usize) -> Result<BoundedLength, std::io::Error> {
    let mut decoder = Decompress::new(true);
    let mut input_pos = 0_usize;
    let mut total = 0_usize;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let remaining = max.saturating_sub(total);
        let output_cap = remaining.saturating_add(1).min(buffer.len());
        let before_input = decoder.total_in();
        let before_output = decoder.total_out();
        let flush = if input_pos == payload.len() {
            FlushDecompress::Finish
        } else {
            FlushDecompress::None
        };
        let status = decoder
            .decompress(&payload[input_pos..], &mut buffer[..output_cap], flush)
            .map_err(decompress_io_error)?;
        let consumed = usize::try_from(decoder.total_in() - before_input)
            .expect("bounded zlib input progress fits usize");
        let produced = usize::try_from(decoder.total_out() - before_output)
            .expect("bounded zlib output progress fits usize");
        input_pos += consumed;
        total = total.checked_add(produced).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zlib decompressed length overflow",
            )
        })?;
        if total > max {
            return Ok(BoundedLength::TooLarge { at_least: total });
        }
        if status == Status::StreamEnd {
            ensure_compressed_consumed(input_pos, payload.len(), "zlib stream")?;
            return Ok(BoundedLength::Exact(total));
        }
        if consumed == 0 && produced == 0 {
            return Err(std::io::Error::new(
                if input_pos == payload.len() {
                    std::io::ErrorKind::UnexpectedEof
                } else {
                    std::io::ErrorKind::InvalidData
                },
                if input_pos == payload.len() {
                    "truncated zlib stream"
                } else {
                    "zlib decoder made no progress"
                },
            ));
        }
    }
}

fn count_gzip_bytes(payload: &[u8], max: usize) -> Result<BoundedLength, std::io::Error> {
    let mut decoder = GzDecoder::new(Cursor::new(payload));
    let len = count_bounded_bytes(&mut decoder, max)?;
    if matches!(len, BoundedLength::Exact(_)) {
        ensure_compressed_consumed(
            decoder.into_inner().position() as usize,
            payload.len(),
            "gzip member",
        )?;
    }
    Ok(len)
}

fn count_bounded_bytes(
    reader: &mut impl Read,
    max: usize,
) -> Result<BoundedLength, std::io::Error> {
    let mut total = 0_usize;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let remaining = max.saturating_sub(total);
        let read_cap = remaining.saturating_add(1).min(buffer.len());
        let read = reader.read(&mut buffer[..read_cap])?;
        if read == 0 {
            return Ok(BoundedLength::Exact(total));
        }
        total = total.checked_add(read).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "decompressed Anvil length overflow",
            )
        })?;
        if total > max {
            return Ok(BoundedLength::TooLarge { at_least: total });
        }
    }
}

fn decompress_exact(
    comp: CompressionType,
    payload: &[u8],
    decoded_len: usize,
) -> Result<Vec<u8>, std::io::Error> {
    match comp {
        CompressionType::Uncompressed => {
            let mut out = allocate_exact_bytes(decoded_len, "uncompressed Anvil chunk")?;
            out.copy_from_slice(payload);
            Ok(out)
        }
        CompressionType::Zlib => decompress_zlib_exact(payload, decoded_len),
        CompressionType::Gzip => decompress_gzip_exact(payload, decoded_len),
        CompressionType::Lz4 => decompress_lz4_exact(payload, decoded_len),
    }
}

fn decompress_zlib_exact(payload: &[u8], decoded_len: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut out = allocate_exact_bytes(decoded_len, "decompressed zlib Anvil chunk")?;
    let mut decoder = Decompress::new(true);
    let mut input_pos = 0_usize;
    let mut output_pos = 0_usize;
    let mut extra = [0_u8; 1];
    loop {
        let before_input = decoder.total_in();
        let before_output = decoder.total_out();
        let flush = if input_pos == payload.len() {
            FlushDecompress::Finish
        } else {
            FlushDecompress::None
        };
        let status = if output_pos < out.len() {
            decoder.decompress(&payload[input_pos..], &mut out[output_pos..], flush)
        } else {
            decoder.decompress(&payload[input_pos..], &mut extra, flush)
        }
        .map_err(decompress_io_error)?;
        let consumed = usize::try_from(decoder.total_in() - before_input)
            .expect("bounded zlib input progress fits usize");
        let produced = usize::try_from(decoder.total_out() - before_output)
            .expect("bounded zlib output progress fits usize");
        input_pos += consumed;
        if output_pos == out.len() && produced != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zlib output changed between bounded passes",
            ));
        }
        output_pos = output_pos.checked_add(produced).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zlib output length overflow",
            )
        })?;
        if output_pos > out.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zlib output changed between bounded passes",
            ));
        }
        if status == Status::StreamEnd {
            ensure_compressed_consumed(input_pos, payload.len(), "zlib stream")?;
            if output_pos != decoded_len {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "zlib output changed between bounded passes",
                ));
            }
            return Ok(out);
        }
        if consumed == 0 && produced == 0 {
            return Err(std::io::Error::new(
                if input_pos == payload.len() {
                    std::io::ErrorKind::UnexpectedEof
                } else {
                    std::io::ErrorKind::InvalidData
                },
                if input_pos == payload.len() {
                    "truncated zlib stream"
                } else {
                    "zlib decoder made no progress"
                },
            ));
        }
    }
}

fn decompress_gzip_exact(payload: &[u8], decoded_len: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut out = allocate_exact_bytes(decoded_len, "decompressed gzip Anvil chunk")?;
    let mut decoder = GzDecoder::new(Cursor::new(payload));
    decoder.read_exact(&mut out)?;
    ensure_decoder_eof(&mut decoder, "gzip output changed between bounded passes")?;
    ensure_compressed_consumed(
        decoder.into_inner().position() as usize,
        payload.len(),
        "gzip member",
    )?;
    Ok(out)
}

fn ensure_decoder_eof(
    decoder: &mut impl Read,
    message: &'static str,
) -> Result<(), std::io::Error> {
    let mut extra = [0_u8; 1];
    if decoder.read(&mut extra)? != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        ));
    }
    Ok(())
}

fn ensure_compressed_consumed(
    consumed: usize,
    expected: usize,
    kind: &'static str,
) -> Result<(), std::io::Error> {
    if consumed != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "trailing bytes after {kind}: {}",
                expected.saturating_sub(consumed)
            ),
        ));
    }
    Ok(())
}

fn decompress_io_error(error: flate2::DecompressError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}

fn allocate_exact_bytes(len: usize, context: &'static str) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|error| {
        std::io::Error::other(format!("reserve {context} buffer of {len} bytes: {error}"))
    })?;
    bytes.resize(len, 0);
    Ok(bytes)
}

#[derive(Debug, Clone, Copy)]
struct Lz4BlockHeader {
    token: u8,
    compressed_len: usize,
    decompressed_len: usize,
    checksum: u32,
}

fn count_lz4_bytes(payload: &[u8], max: usize) -> Result<BoundedLength, std::io::Error> {
    let mut pos = 0_usize;
    let mut total = 0_usize;
    loop {
        let header = read_lz4_block_header(payload, &mut pos)?;
        if header.compressed_len == 0 && header.decompressed_len == 0 {
            validate_lz4_end_marker(header)?;
            if pos != payload.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "trailing bytes after LZ4 end marker",
                ));
            }
            return Ok(BoundedLength::Exact(total));
        }
        validate_lz4_method(header.token, header.compressed_len, header.decompressed_len)?;
        total = total.checked_add(header.decompressed_len).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "LZ4 decompressed length overflow",
            )
        })?;
        if total > max {
            return Ok(BoundedLength::TooLarge { at_least: total });
        }
        if payload.len() - pos < header.compressed_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated LZ4 block payload",
            ));
        }
        if header.token & 0xF0 == LZ4_BLOCK_METHOD_RAW {
            verify_lz4_checksum(&payload[pos..pos + header.compressed_len], header.checksum)?;
        } else if header.token & 0xF0 == LZ4_BLOCK_METHOD_COMPRESSED {
            // Compressed blocks carry no self-describing checksum: decode
            // into scratch and verify, mirroring decompress_lz4_exact, so
            // counting rejects the same corrupt inputs decoding rejects.
            let block = &payload[pos..pos + header.compressed_len];
            let mut scratch =
                allocate_exact_bytes(header.decompressed_len, "LZ4 verification block")?;
            let written = lz4_flex::block::decompress_into(block, &mut scratch)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            if written != header.decompressed_len {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "LZ4 block decompressed length mismatch",
                ));
            }
            verify_lz4_checksum(&scratch, header.checksum)?;
        }
        pos += header.compressed_len;
    }
}

fn decompress_lz4_exact(payload: &[u8], decoded_len: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut out = allocate_exact_bytes(decoded_len, "decompressed LZ4 Anvil chunk")?;
    let mut pos = 0_usize;
    let mut out_pos = 0_usize;
    loop {
        let header = read_lz4_block_header(payload, &mut pos)?;
        if header.compressed_len == 0 && header.decompressed_len == 0 {
            validate_lz4_end_marker(header)?;
            if pos != payload.len() || out_pos != decoded_len {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "LZ4 output changed between bounded passes",
                ));
            }
            return Ok(out);
        }
        validate_lz4_method(header.token, header.compressed_len, header.decompressed_len)?;
        if payload.len() - pos < header.compressed_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated LZ4 block payload",
            ));
        }
        let out_end = out_pos
            .checked_add(header.decompressed_len)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "LZ4 output length overflow",
                )
            })?;
        if out_end > out.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "LZ4 output changed between bounded passes",
            ));
        }
        let block = &payload[pos..pos + header.compressed_len];
        match header.token & 0xF0 {
            LZ4_BLOCK_METHOD_RAW => out[out_pos..out_end].copy_from_slice(block),
            LZ4_BLOCK_METHOD_COMPRESSED => {
                let written = lz4_flex::block::decompress_into(block, &mut out[out_pos..out_end])
                    .map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                })?;
                if written != header.decompressed_len {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "LZ4 block decompressed length mismatch",
                    ));
                }
            }
            _ => unreachable!("LZ4 method validated before decode"),
        }
        verify_lz4_checksum(&out[out_pos..out_end], header.checksum)?;
        pos += header.compressed_len;
        out_pos = out_end;
    }
}

fn read_lz4_block_header(
    payload: &[u8],
    pos: &mut usize,
) -> Result<Lz4BlockHeader, std::io::Error> {
    if payload.len() - *pos < LZ4_BLOCK_HEADER_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "truncated LZ4 block header",
        ));
    }
    if &payload[*pos..*pos + LZ4_BLOCK_MAGIC.len()] != LZ4_BLOCK_MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing LZ4Block magic",
        ));
    }
    let token = payload[*pos + 8];
    let compressed_len = u32::from_le_bytes(
        payload[*pos + 9..*pos + 13]
            .try_into()
            .expect("4-byte slice"),
    ) as usize;
    let decompressed_len = u32::from_le_bytes(
        payload[*pos + 13..*pos + 17]
            .try_into()
            .expect("4-byte slice"),
    ) as usize;
    let checksum = u32::from_le_bytes(
        payload[*pos + 17..*pos + 21]
            .try_into()
            .expect("4-byte slice"),
    );
    *pos += LZ4_BLOCK_HEADER_LEN;
    Ok(Lz4BlockHeader {
        token,
        compressed_len,
        decompressed_len,
        checksum,
    })
}

fn validate_lz4_end_marker(header: Lz4BlockHeader) -> Result<(), std::io::Error> {
    if header.token & 0xF0 != LZ4_BLOCK_METHOD_RAW || header.checksum != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid LZ4 end marker",
        ));
    }
    Ok(())
}

fn validate_lz4_method(
    token: u8,
    compressed_len: usize,
    decompressed_len: usize,
) -> Result<(), std::io::Error> {
    match token & 0xF0 {
        LZ4_BLOCK_METHOD_RAW if compressed_len == decompressed_len => Ok(()),
        LZ4_BLOCK_METHOD_RAW => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "raw LZ4 block length mismatch",
        )),
        LZ4_BLOCK_METHOD_COMPRESSED => Ok(()),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown LZ4 block token {token:#04x}"),
        )),
    }
}

fn verify_lz4_checksum(bytes: &[u8], expected: u32) -> Result<(), std::io::Error> {
    let actual = xxhash32(bytes, LZ4_CHECKSUM_SEED);
    if actual != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("LZ4 block checksum mismatch: expected {expected:#010x}, got {actual:#010x}"),
        ));
    }
    Ok(())
}

fn xxhash32(bytes: &[u8], seed: u32) -> u32 {
    let mut pos = 0_usize;
    let mut hash = if bytes.len() >= 16 {
        let mut v1 = seed.wrapping_add(XXH32_PRIME1).wrapping_add(XXH32_PRIME2);
        let mut v2 = seed.wrapping_add(XXH32_PRIME2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(XXH32_PRIME1);
        while bytes.len() - pos >= 16 {
            v1 = xxhash32_round(v1, read_u32_le(bytes, pos));
            v2 = xxhash32_round(v2, read_u32_le(bytes, pos + 4));
            v3 = xxhash32_round(v3, read_u32_le(bytes, pos + 8));
            v4 = xxhash32_round(v4, read_u32_le(bytes, pos + 12));
            pos += 16;
        }
        v1.rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18))
    } else {
        seed.wrapping_add(XXH32_PRIME5)
    };

    hash = hash.wrapping_add(bytes.len() as u32);
    while bytes.len() - pos >= 4 {
        hash = hash
            .wrapping_add(read_u32_le(bytes, pos).wrapping_mul(XXH32_PRIME3))
            .rotate_left(17)
            .wrapping_mul(XXH32_PRIME4);
        pos += 4;
    }
    while pos < bytes.len() {
        hash = hash
            .wrapping_add(u32::from(bytes[pos]).wrapping_mul(XXH32_PRIME5))
            .rotate_left(11)
            .wrapping_mul(XXH32_PRIME1);
        pos += 1;
    }

    hash ^= hash >> 15;
    hash = hash.wrapping_mul(XXH32_PRIME2);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(XXH32_PRIME3);
    hash ^ (hash >> 16)
}

fn xxhash32_round(accumulator: u32, input: u32) -> u32 {
    accumulator
        .wrapping_add(input.wrapping_mul(XXH32_PRIME2))
        .rotate_left(13)
        .wrapping_mul(XXH32_PRIME1)
}

fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("caller checks four input bytes"),
    )
}

#[cfg(test)]
#[path = "region_tests.rs"]
mod tests;
