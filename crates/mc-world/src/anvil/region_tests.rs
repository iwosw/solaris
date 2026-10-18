use super::*;
use flate2::write::GzEncoder;
use std::path::{Path, PathBuf};

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(rel)
}

fn first_populated_chunk(path: &Path) -> Option<(u8, Vec<u8>)> {
    let bytes = std::fs::read(path).unwrap();
    for slot in 0..REGION_CHUNK_COUNT {
        let loc_off = slot * 4;
        let loc = u32::from_be_bytes(
            bytes[loc_off..loc_off + 4]
                .try_into()
                .expect("4-byte slice"),
        );
        if loc == 0 {
            continue;
        }
        let sector = loc >> 8;
        let start = sector as usize * SECTOR_SIZE;
        let len =
            u32::from_be_bytes(bytes[start..start + 4].try_into().expect("4-byte slice")) as usize;
        let compression = bytes[start + 4];
        let payload = bytes[start + 5..start + 4 + len].to_vec();
        return Some((compression, payload));
    }
    None
}

fn synthetic_region(compression: u8, payload: &[u8]) -> tempfile::NamedTempFile {
    synthetic_region_chunks(&[(0, compression, payload.to_vec())])
}

fn synthetic_region_chunks(chunks: &[(usize, u8, Vec<u8>)]) -> tempfile::NamedTempFile {
    let mut bytes = vec![0_u8; HEADER_BYTES];
    let mut next_sector = HEADER_SECTORS;
    for (slot, compression, payload) in chunks {
        assert!(*slot < REGION_CHUNK_COUNT);
        let body_len = 5 + payload.len();
        let sectors = body_len.div_ceil(SECTOR_SIZE).max(1);
        assert!(sectors <= usize::from(u8::MAX));
        let location = ((next_sector as u32) << 8) | sectors as u32;
        let loc_off = slot * 4;
        bytes[loc_off..loc_off + 4].copy_from_slice(&location.to_be_bytes());
        let ts_off = SECTOR_SIZE + slot * 4;
        bytes[ts_off..ts_off + 4]
            .copy_from_slice(&(1_700_000_000_u32 + *slot as u32).to_be_bytes());

        let chunk_start = next_sector * SECTOR_SIZE;
        bytes.resize((next_sector + sectors) * SECTOR_SIZE, 0);
        let declared_len = (payload.len() + 1) as u32;
        bytes[chunk_start..chunk_start + 4].copy_from_slice(&declared_len.to_be_bytes());
        bytes[chunk_start + 4] = *compression;
        bytes[chunk_start + 5..chunk_start + 5 + payload.len()].copy_from_slice(payload);
        next_sector += sectors;
    }

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), bytes).unwrap();
    tmp
}

fn overwrite_location(path: &Path, slot: usize, sector: u32, count: u8) {
    assert!(slot < REGION_CHUNK_COUNT);
    assert!(sector < (1 << 24));
    let mut bytes = std::fs::read(path).unwrap();
    let location = (sector << 8) | u32::from(count);
    let offset = slot * 4;
    bytes[offset..offset + 4].copy_from_slice(&location.to_be_bytes());
    std::fs::write(path, bytes).unwrap();
}

fn collect_with_limits(
    path: &Path,
    limits: RegionLimits,
) -> Result<Vec<ChunkPayload>, RegionError> {
    let mut chunks = Vec::new();
    read_region_with_limits(path, limits, |payload| chunks.push(payload))?;
    Ok(chunks)
}

fn test_limits(max_chunk_bytes: usize, max_region_bytes: usize) -> RegionLimits {
    RegionLimits {
        max_file_bytes: MAX_REGION_FILE_BYTES,
        max_chunk_bytes,
        max_region_bytes,
    }
}

fn gzip_payload(raw: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw).unwrap();
    encoder.finish().unwrap()
}

fn zlib_payload(raw: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw).unwrap();
    encoder.finish().unwrap()
}

fn deterministic_incompressible(len: usize) -> Vec<u8> {
    let mut out = vec![0_u8; len];
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    for bytes in out.chunks_mut(8) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.copy_from_slice(&state.to_le_bytes()[..bytes.len()]);
    }
    out
}

fn encoded_zlib_sectors(raw: &[u8]) -> usize {
    zlib_payload(raw)
        .len()
        .saturating_add(5)
        .div_ceil(SECTOR_SIZE)
}

#[test]
fn reused_compressor_keeps_independent_vanilla_slot_bytes() {
    let chunks: Vec<_> = [
        deterministic_incompressible(65_537),
        vec![17; 100_000],
        Vec::new(),
        deterministic_incompressible(8193),
    ]
    .into_iter()
    .enumerate()
    .map(|(slot, uncompressed_nbt)| ChunkPayload {
        local_x: slot as u8,
        local_z: 0,
        timestamp: 1_700_000_000 + slot as u32,
        uncompressed_nbt,
    })
    .collect();
    let expected = synthetic_region_chunks(
        &chunks
            .iter()
            .enumerate()
            .map(|(slot, chunk)| (slot, 2, zlib_payload(&chunk.uncompressed_nbt)))
            .collect::<Vec<_>>(),
    );
    let directory = tempfile::tempdir().unwrap();
    let fresh = directory.path().join("fresh.mca");
    write_region(&fresh, &chunks).unwrap();
    assert_eq!(
        std::fs::read(&fresh).unwrap(),
        std::fs::read(expected.path()).unwrap()
    );

    // A preserved stream between fresh streams must neither advance nor reset
    // their dictionary, and must remain byte-identical itself.
    let records: Vec<_> = chunks
        .iter()
        .enumerate()
        .map(|(slot, chunk)| {
            if slot == 1 {
                RegionChunkOutput::Preserved(RawChunkRecord {
                    local_x: chunk.local_x,
                    local_z: chunk.local_z,
                    timestamp: chunk.timestamp,
                    compression: 2,
                    compressed: zlib_payload(&chunk.uncompressed_nbt),
                    validated_decoded_len: None,
                    validated_content_stamp: None,
                })
            } else {
                RegionChunkOutput::Fresh(chunk.clone())
            }
        })
        .collect();
    let mixed = directory.path().join("mixed.mca");
    write_region_create_new_mixed(&mixed, &records).unwrap();
    assert_eq!(
        std::fs::read(&mixed).unwrap(),
        std::fs::read(expected.path()).unwrap()
    );
    let restored = read_region(&mixed).unwrap();
    for (expected, actual) in chunks.iter().zip(restored) {
        assert_eq!(actual.uncompressed_nbt, expected.uncompressed_nbt);
        assert_eq!(actual.timestamp, expected.timestamp);
    }
}

fn lz4_block_payload(method: u8, block: &[u8], decompressed_len: usize, checksum: u32) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(LZ4_BLOCK_MAGIC);
    payload.push(method);
    payload.extend_from_slice(&(block.len() as u32).to_le_bytes());
    payload.extend_from_slice(&(decompressed_len as u32).to_le_bytes());
    payload.extend_from_slice(&checksum.to_le_bytes());
    payload.extend_from_slice(block);
    payload.extend_from_slice(LZ4_BLOCK_MAGIC);
    payload.push(LZ4_BLOCK_METHOD_RAW);
    payload.extend_from_slice(&0_u32.to_le_bytes());
    payload.extend_from_slice(&0_u32.to_le_bytes());
    payload.extend_from_slice(&0_u32.to_le_bytes());
    payload
}

fn raw_lz4_block_payload(raw: &[u8]) -> Vec<u8> {
    lz4_block_payload(
        LZ4_BLOCK_METHOD_RAW,
        raw,
        raw.len(),
        xxhash32(raw, LZ4_CHECKSUM_SEED),
    )
}

fn compressed_lz4_block_payload(raw: &[u8]) -> Vec<u8> {
    let compressed = lz4_flex::block::compress(raw);
    lz4_block_payload(
        LZ4_BLOCK_METHOD_COMPRESSED,
        &compressed,
        raw.len(),
        xxhash32(raw, LZ4_CHECKSUM_SEED),
    )
}

#[test]
fn round_trip_synthetic_region() {
    let payloads = vec![
        ChunkPayload {
            local_x: 0,
            local_z: 0,
            timestamp: 1_700_000_000,
            uncompressed_nbt: b"hello".to_vec(),
        },
        ChunkPayload {
            local_x: 31,
            local_z: 31,
            timestamp: 1_700_000_500,
            uncompressed_nbt: vec![0xAA; 10_000],
        },
    ];

    let tmp = tempfile::NamedTempFile::new().unwrap();
    write_region(tmp.path(), &payloads).unwrap();

    let read = read_region(tmp.path()).unwrap();
    assert_eq!(read.len(), 2);
    let by_slot: HashMap<(u8, u8), &ChunkPayload> =
        read.iter().map(|c| ((c.local_x, c.local_z), c)).collect();
    for orig in &payloads {
        let got = by_slot[&(orig.local_x, orig.local_z)];
        assert_eq!(got.timestamp, orig.timestamp);
        assert_eq!(got.uncompressed_nbt, orig.uncompressed_nbt);
    }
}

#[test]
fn encoded_chunk_layout_enforces_255_sector_boundary() {
    let (_, padded_len, sectors) = encoded_chunk_layout(MAX_COMPRESSED_CHUNK_BYTES, 3, 4).unwrap();
    assert_eq!(padded_len, MAX_SECTORS_PER_CHUNK * SECTOR_SIZE);
    assert_eq!(sectors, u32::from(u8::MAX));

    for (compressed_len, expected_sectors) in [
        (MAX_COMPRESSED_CHUNK_BYTES + 1, 256),
        (256 * SECTOR_SIZE - 4, 257),
    ] {
        assert!(matches!(
            encoded_chunk_layout(compressed_len, 3, 4),
            Err(RegionError::ChunkTooLarge {
                cx: 3,
                cz: 4,
                sectors,
                max_sectors: MAX_SECTORS_PER_CHUNK,
            }) if sectors == expected_sectors
        ));
    }
}

#[test]
fn write_region_accepts_255_sector_incompressible_chunk() {
    let raw = deterministic_incompressible(1_043_500);
    assert_eq!(encoded_zlib_sectors(&raw), 255);
    let payload = ChunkPayload {
        local_x: 7,
        local_z: 9,
        timestamp: 1_700_000_700,
        uncompressed_nbt: raw.clone(),
    };
    let tmp = tempfile::NamedTempFile::new().unwrap();

    write_region(tmp.path(), &[payload]).unwrap();

    let bytes = std::fs::read(tmp.path()).unwrap();
    let slot = 9 * CHUNKS_PER_REGION_AXIS + 7;
    let loc_off = slot * 4;
    let location = u32::from_be_bytes(
        bytes[loc_off..loc_off + 4]
            .try_into()
            .expect("4-byte location"),
    );
    assert_eq!(location & 0xFF, u32::from(u8::MAX));
    let read = read_region(tmp.path()).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].uncompressed_nbt, raw);
}

#[test]
fn write_region_rejects_invalid_and_duplicate_slots_without_touching_output() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let sentinel = b"owner bytes must remain unchanged";
    let base = ChunkPayload {
        local_x: 1,
        local_z: 2,
        timestamp: 1,
        uncompressed_nbt: b"valid".to_vec(),
    };

    for invalid in [
        ChunkPayload {
            local_x: 32,
            ..base.clone()
        },
        ChunkPayload {
            local_z: 32,
            ..base.clone()
        },
    ] {
        std::fs::write(tmp.path(), sentinel).unwrap();
        assert!(matches!(
            write_region(tmp.path(), &[invalid]),
            Err(RegionError::InvalidChunkCoordinates { .. })
        ));
        assert_eq!(std::fs::read(tmp.path()).unwrap(), sentinel);
    }

    std::fs::write(tmp.path(), sentinel).unwrap();
    assert!(matches!(
        write_region(tmp.path(), &[base.clone(), base.clone()]),
        Err(RegionError::DuplicateChunkSlot { cx: 1, cz: 2 })
    ));
    assert_eq!(std::fs::read(tmp.path()).unwrap(), sentinel);

    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("invalid.mca");
    assert!(matches!(
        write_region_create_new(
            &absent,
            &[ChunkPayload {
                local_x: 32,
                ..base
            }]
        ),
        Err(RegionError::InvalidChunkCoordinates { cx: 32, cz: 2 })
    ));
    assert!(!absent.exists());
}

#[test]
fn write_region_rejects_256_and_257_sector_chunks_without_touching_output() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let sentinel = b"existing region image";
    for (raw_len, expected_sectors) in [(1_045_000, 256), (1_049_000, 257)] {
        let raw = deterministic_incompressible(raw_len);
        assert_eq!(encoded_zlib_sectors(&raw), expected_sectors);
        std::fs::write(tmp.path(), sentinel).unwrap();

        assert!(matches!(
            write_region(
                tmp.path(),
                &[ChunkPayload {
                    local_x: 5,
                    local_z: 6,
                    timestamp: 2,
                    uncompressed_nbt: raw,
                }]
            ),
            Err(RegionError::ChunkTooLarge {
                cx: 5,
                cz: 6,
                sectors,
                max_sectors: MAX_SECTORS_PER_CHUNK,
            }) if sectors == expected_sectors
        ));
        assert_eq!(std::fs::read(tmp.path()).unwrap(), sentinel);
    }
}

#[test]
fn write_preflight_enforces_reader_decode_budgets() {
    let one = ChunkPayload {
        local_x: 0,
        local_z: 0,
        timestamp: 0,
        uncompressed_nbt: vec![0; 5],
    };
    assert!(matches!(
        validate_write_chunks(std::slice::from_ref(&one), test_limits(4, 16)),
        Err(RegionError::DecompressedChunkTooLarge {
            cx: 0,
            cz: 0,
            bytes: 5,
            max: 4,
        })
    ));
    assert!(matches!(
        validate_write_chunks(
            &[
                one,
                ChunkPayload {
                    local_x: 1,
                    local_z: 0,
                    timestamp: 0,
                    uncompressed_nbt: vec![0; 5],
                },
            ],
            test_limits(8, 9),
        ),
        Err(RegionError::DecompressedRegionTooLarge { bytes: 10, max: 9 })
    ));
}

#[test]
fn reads_synthetic_gzip_uncompressed_and_lz4_regions() {
    let raw = b"synthetic chunk payload bytes";
    for (compression, payload) in [
        (CompressionType::Gzip as u8, gzip_payload(raw)),
        (CompressionType::Zlib as u8, zlib_payload(raw)),
        (CompressionType::Uncompressed as u8, raw.to_vec()),
        (CompressionType::Lz4 as u8, raw_lz4_block_payload(raw)),
        (
            CompressionType::Lz4 as u8,
            compressed_lz4_block_payload(raw),
        ),
    ] {
        let region = synthetic_region(compression, &payload);

        let chunks = read_region(region.path()).unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].local_x, 0);
        assert_eq!(chunks[0].local_z, 0);
        assert_eq!(chunks[0].timestamp, 1_700_000_000);
        assert_eq!(chunks[0].uncompressed_nbt, raw);
        let chunk = read_chunk(region.path(), 0, 0).unwrap().unwrap();
        assert_eq!(chunk.timestamp, 1_700_000_000);
        assert_eq!(chunk.uncompressed_nbt, raw);
    }
}

#[test]
fn xxhash32_matches_published_zero_seed_vectors() {
    assert_eq!(xxhash32(b"", 0), 0x02CC_5D05);
    assert_eq!(xxhash32(b"a", 0), 0x550D_7456);
    assert_eq!(xxhash32(b"abc", 0), 0x32D1_53FF);
}

#[test]
fn rejects_raw_and_compressed_lz4_checksum_mismatch() {
    let raw = b"checksum-protected LZ4 payload";
    for mut payload in [
        raw_lz4_block_payload(raw),
        compressed_lz4_block_payload(raw),
    ] {
        payload[17] ^= 0x01;
        let region = synthetic_region(CompressionType::Lz4 as u8, &payload);
        let error = read_region(region.path()).unwrap_err();
        assert!(matches!(
            error,
            RegionError::Decompress { cx: 0, cz: 0, .. }
        ));
        assert!(error.to_string().contains("checksum mismatch"));
    }
}

#[test]
fn rejects_lz4_end_marker_with_nonzero_checksum() {
    let mut payload = raw_lz4_block_payload(b"payload");
    let end_marker = payload.len() - LZ4_BLOCK_HEADER_LEN;
    payload[end_marker + 17..end_marker + 21].copy_from_slice(&1_u32.to_le_bytes());
    let region = synthetic_region(CompressionType::Lz4 as u8, &payload);

    let error = read_region(region.path()).unwrap_err();
    assert!(matches!(
        error,
        RegionError::Decompress { cx: 0, cz: 0, .. }
    ));
    assert!(error.to_string().contains("invalid LZ4 end marker"));
}

#[test]
fn slot_reader_and_visitor_do_not_require_retaining_the_region() {
    let region = synthetic_region_chunks(&[
        (0, CompressionType::Uncompressed as u8, b"zero".to_vec()),
        (33, CompressionType::Uncompressed as u8, b"target".to_vec()),
    ]);

    let target = read_chunk(region.path(), 1, 1).unwrap().unwrap();
    assert_eq!(target.uncompressed_nbt, b"target");
    assert_eq!(
        read_chunk(region.path(), 0, 0)
            .unwrap()
            .unwrap()
            .uncompressed_nbt,
        b"zero"
    );
    assert!(read_chunk(region.path(), 2, 2).unwrap().is_none());
    assert!(matches!(
        read_chunk(region.path(), 32, 0),
        Err(RegionError::InvalidChunkCoordinates { cx: 32, cz: 0 })
    ));

    let mut visited = Vec::new();
    visit_region(region.path(), |payload| {
        visited.push((payload.local_x, payload.local_z, payload.uncompressed_nbt));
    })
    .unwrap();
    assert_eq!(
        visited,
        vec![(0, 0, b"zero".to_vec()), (1, 1, b"target".to_vec())]
    );
}

#[test]
fn slot_reader_isolated_from_unrelated_corrupt_slot() {
    let region = synthetic_region_chunks(&[
        (0, 0x7F, b"corrupt".to_vec()),
        (1, CompressionType::Zlib as u8, b"not zlib".to_vec()),
        (2, 0x80 | CompressionType::Zlib as u8, Vec::new()),
        (33, CompressionType::Uncompressed as u8, b"target".to_vec()),
    ]);

    let target = read_chunk(region.path(), 1, 1).unwrap().unwrap();
    assert_eq!(target.uncompressed_nbt, b"target");
    assert!(matches!(
        read_chunk(region.path(), 0, 0),
        Err(RegionError::UnknownCompression(0x7F))
    ));
    assert!(matches!(
        read_chunk(region.path(), 1, 0),
        Err(RegionError::Decompress { cx: 1, cz: 0, .. })
    ));
    assert!(matches!(
        read_chunk(region.path(), 2, 0),
        Err(RegionError::Oversized)
    ));
    assert_eq!(
        read_chunk(region.path(), 1, 1)
            .unwrap()
            .unwrap()
            .uncompressed_nbt,
        b"target"
    );
    assert!(matches!(
        read_region(region.path()),
        Err(RegionError::UnknownCompression(0x7F))
    ));
}

#[test]
fn location_table_rejects_reserved_zero_count_and_overlap_before_decode() {
    let reserved = synthetic_region(CompressionType::Uncompressed as u8, b"payload");
    overwrite_location(reserved.path(), 0, 1, 1);
    assert!(matches!(
        read_region(reserved.path()),
        Err(RegionError::ReservedSector {
            cx: 0,
            cz: 0,
            sector: 1,
            ..
        })
    ));

    let zero_count = synthetic_region(CompressionType::Uncompressed as u8, b"payload");
    overwrite_location(zero_count.path(), 0, 2, 0);
    assert!(matches!(
        read_region(zero_count.path()),
        Err(RegionError::ZeroSectorCount { cx: 0, cz: 0, .. })
    ));

    let overlap = synthetic_region_chunks(&[
        (0, CompressionType::Uncompressed as u8, b"first".to_vec()),
        (1, CompressionType::Uncompressed as u8, b"second".to_vec()),
    ]);
    overwrite_location(overlap.path(), 1, 2, 1);
    assert!(matches!(
        RegionReader::open(overlap.path()),
        Err(RegionError::OverlappingChunkSectors { .. })
    ));
    let mut visited = 0_usize;
    let error = visit_region(overlap.path(), |_| visited += 1).unwrap_err();
    assert_eq!(visited, 0, "structural validation must precede visitors");
    assert!(matches!(
        error,
        RegionError::OverlappingChunkSectors {
            cx: 1,
            cz: 0,
            other_cx: 0,
            other_cz: 0,
            overlap_start: 2,
            overlap_end: 3,
            ..
        }
    ));
}

#[test]
fn location_table_fuzz_matches_interval_oracle() {
    const FILE_SECTORS: u32 = 16;
    let mut state = 0xD1B5_4A32_D192_ED03_u64;
    for case in 0..512 {
        let mut header = [0_u8; HEADER_BYTES];
        for _ in 0..12 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let slot = (state as usize) % REGION_CHUNK_COUNT;
            let sector = ((state >> 16) as u32) % 20;
            let count = ((state >> 32) as u8) % 4;
            let location = (sector << 8) | u32::from(count);
            let offset = slot * 4;
            header[offset..offset + 4].copy_from_slice(&location.to_be_bytes());
        }

        let mut expected_valid = true;
        let mut intervals = Vec::new();
        for slot in 0..REGION_CHUNK_COUNT {
            let offset = slot * 4;
            let location = u32::from_be_bytes(
                header[offset..offset + 4]
                    .try_into()
                    .expect("4-byte location"),
            );
            let sector = location >> 8;
            let count = (location & 0xFF) as u8;
            if sector == 0 && count == 0 {
                continue;
            }
            let end = sector + u32::from(count);
            if count == 0
                || sector < HEADER_SECTORS as u32
                || end > FILE_SECTORS
                || intervals
                    .iter()
                    .any(|&(start, previous_end)| sector < previous_end && start < end)
            {
                expected_valid = false;
                break;
            }
            intervals.push((sector, end));
        }

        let result = validate_location_table(
            Path::new("fuzz-region.mca"),
            &header,
            u64::from(FILE_SECTORS) * SECTOR_SIZE as u64,
        );
        assert_eq!(
            result.is_ok(),
            expected_valid,
            "location-table fuzz case {case} disagreed with interval oracle"
        );
    }
}

#[test]
fn rejects_region_file_above_sector_geometry_limit_before_body_read() {
    let region = tempfile::NamedTempFile::new().unwrap();
    region.as_file().set_len((HEADER_BYTES + 1) as u64).unwrap();
    let limits = RegionLimits {
        max_file_bytes: HEADER_BYTES as u64,
        max_chunk_bytes: 64,
        max_region_bytes: 64,
    };

    let error = collect_with_limits(region.path(), limits).unwrap_err();
    assert!(matches!(
        error,
        RegionError::RegionTooLarge { bytes, max }
            if bytes == (HEADER_BYTES + 1) as u64 && max == HEADER_BYTES as u64
    ));
}

#[test]
fn rejects_decompression_bombs_for_every_codec_before_output_allocation() {
    let raw = vec![0xA5; 65];
    let declared_lz4_bomb = lz4_block_payload(LZ4_BLOCK_METHOD_COMPRESSED, &[0], raw.len(), 0);
    for (compression, payload) in [
        (CompressionType::Gzip as u8, gzip_payload(&raw)),
        (CompressionType::Zlib as u8, zlib_payload(&raw)),
        (CompressionType::Uncompressed as u8, raw.clone()),
        (CompressionType::Lz4 as u8, raw_lz4_block_payload(&raw)),
        (CompressionType::Lz4 as u8, declared_lz4_bomb),
    ] {
        let region = synthetic_region(compression, &payload);
        let error = collect_with_limits(region.path(), test_limits(64, 128)).unwrap_err();
        assert!(matches!(
            error,
            RegionError::DecompressedChunkTooLarge {
                cx: 0,
                cz: 0,
                bytes: 65,
                max: 64,
            }
        ));
    }
}

#[test]
fn accepts_exact_chunk_limit_for_every_codec() {
    let raw = vec![0x5A; 64];
    for (compression, payload) in [
        (CompressionType::Gzip as u8, gzip_payload(&raw)),
        (CompressionType::Zlib as u8, zlib_payload(&raw)),
        (CompressionType::Uncompressed as u8, raw.clone()),
        (CompressionType::Lz4 as u8, raw_lz4_block_payload(&raw)),
        (
            CompressionType::Lz4 as u8,
            compressed_lz4_block_payload(&raw),
        ),
    ] {
        let region = synthetic_region(compression, &payload);
        let chunks = collect_with_limits(region.path(), test_limits(64, 64)).unwrap();
        assert_eq!(chunks[0].uncompressed_nbt, raw);
    }
}

#[test]
fn rejects_aggregate_many_chunk_payloads_before_second_allocation() {
    let region = synthetic_region_chunks(&[
        (0, CompressionType::Uncompressed as u8, vec![1; 40]),
        (1, CompressionType::Uncompressed as u8, vec![2; 40]),
    ]);
    let mut visited = 0_usize;
    let error = read_region_with_limits(region.path(), test_limits(64, 64), |_| {
        visited += 1;
    })
    .unwrap_err();

    assert_eq!(visited, 1);
    assert!(matches!(
        error,
        RegionError::DecompressedRegionTooLarge { bytes: 80, max: 64 }
    ));
}

#[test]
fn rejects_truncated_and_trailing_compressed_streams() {
    let raw = b"bounded compressed stream";
    let mut gzip = gzip_payload(raw);
    gzip.truncate(gzip.len() / 2);
    let mut zlib = zlib_payload(raw);
    zlib.truncate(zlib.len() / 2);
    let mut lz4 = raw_lz4_block_payload(raw);
    lz4.pop();
    for (compression, payload) in [
        (CompressionType::Gzip as u8, gzip),
        (CompressionType::Zlib as u8, zlib),
        (CompressionType::Lz4 as u8, lz4),
    ] {
        let region = synthetic_region(compression, &payload);
        let result = read_region(region.path());
        assert!(
            matches!(result, Err(RegionError::Decompress { cx: 0, cz: 0, .. })),
            "compression {compression} unexpectedly returned {result:?}"
        );
    }

    let mut gzip_trailing = gzip_payload(raw);
    gzip_trailing.push(0);
    let mut zlib_trailing = zlib_payload(raw);
    zlib_trailing.push(0);
    for (compression, payload) in [
        (CompressionType::Gzip as u8, gzip_trailing),
        (CompressionType::Zlib as u8, zlib_trailing),
    ] {
        let region = synthetic_region(compression, &payload);
        let result = read_region(region.path());
        assert!(
            matches!(result, Err(RegionError::Decompress { cx: 0, cz: 0, .. })),
            "compression {compression} unexpectedly returned {result:?}"
        );
    }
}

#[test]
fn rejects_chunk_stream_truncated_inside_allocated_sector() {
    let region = synthetic_region(CompressionType::Uncompressed as u8, b"payload");
    region.as_file().set_len((HEADER_BYTES + 6) as u64).unwrap();

    assert!(matches!(
        read_region(region.path()),
        Err(RegionError::SectorExtentOutOfRange {
            cx: 0,
            cz: 0,
            sector: 2,
            end_sector: 3,
            ..
        })
    ));
}

#[test]
fn rejects_unknown_and_oversized_compression_flags() {
    let unknown = synthetic_region(0x7F, b"payload");
    match read_region(unknown.path()) {
        Err(RegionError::UnknownCompression(0x7F)) => {}
        other => panic!("expected unknown compression, got {other:?}"),
    }

    let oversized = synthetic_region(0x80 | CompressionType::Zlib as u8, b"payload");
    match read_region(oversized.path()) {
        Err(RegionError::Oversized) => {}
        other => panic!("expected oversized rejection, got {other:?}"),
    }
}

/// Read every chunk out of the real vanilla r.0.0.mca, decompress
/// each, and assert the payload starts with the NBT named-root
/// header (`0x0a` = Compound, followed by a length-prefixed name).
/// Opt-in because the vanilla test world is a local artifact.
#[test]
#[ignore = "requires local .analysis/test-world vanilla region"]
fn reads_real_vanilla_region() {
    let path = workspace_path(".analysis/test-world/region/r.0.0.mca");
    assert!(
        path.is_file(),
        "{} not present; run tools/generate-test-world.sh",
        path.display()
    );
    let chunks = read_region(&path).unwrap();
    assert!(!chunks.is_empty(), "spawn region should have chunks");
    for c in &chunks {
        assert!(c.uncompressed_nbt.len() > 100, "NBT should be non-trivial");
        assert_eq!(c.uncompressed_nbt[0], 0x0A, "root tag must be Compound");
    }
}

/// Round-trip every chunk in the real region: read, then write
/// out to a fresh file, then read that back, then confirm the
/// decompressed payloads match byte-for-byte.
#[test]
#[ignore = "requires local .analysis/test-world vanilla region"]
fn round_trip_real_vanilla_region() {
    let path = workspace_path(".analysis/test-world/region/r.0.0.mca");
    assert!(
        path.is_file(),
        "{} not present; run tools/generate-test-world.sh",
        path.display()
    );
    let original = read_region(&path).unwrap();
    assert!(!original.is_empty(), "vanilla oracle region has no chunks");
    let tmp = tempfile::NamedTempFile::new().unwrap();
    write_region(tmp.path(), &original).unwrap();
    let reread = read_region(tmp.path()).unwrap();
    assert_eq!(reread.len(), original.len());
    let by_slot: HashMap<(u8, u8), &ChunkPayload> =
        reread.iter().map(|c| ((c.local_x, c.local_z), c)).collect();
    for orig in &original {
        let got = by_slot[&(orig.local_x, orig.local_z)];
        assert_eq!(got.timestamp, orig.timestamp);
        assert_eq!(
            got.uncompressed_nbt, orig.uncompressed_nbt,
            "decompressed NBT must round-trip for slot ({},{})",
            orig.local_x, orig.local_z
        );
    }
}

/// Prove the local vanilla oracle was actually written with
/// compression byte 4 before exercising the Anvil reader. Skipped
/// against the opt-in local LZ4 oracle world.
#[test]
#[ignore = "requires local .analysis/test-world-lz4 vanilla region"]
fn reads_real_vanilla_lz4_region() {
    let path = workspace_path(".analysis/test-world-lz4/region/r.0.0.mca");
    assert!(
        path.is_file(),
        "{} not present; run OUT_DIR=.analysis/test-world-lz4 REGION_FILE_COMPRESSION=lz4 tools/generate-test-world.sh",
        path.display()
    );

    let (compression, payload) = first_populated_chunk(&path).unwrap();
    assert_eq!(compression, CompressionType::Lz4 as u8);
    assert_eq!(&payload[..LZ4_BLOCK_MAGIC.len()], LZ4_BLOCK_MAGIC);

    let chunks = read_region(&path).unwrap();
    assert!(!chunks.is_empty(), "spawn region should have chunks");
    for c in &chunks {
        assert!(c.uncompressed_nbt.len() > 100, "NBT should be non-trivial");
        assert_eq!(c.uncompressed_nbt[0], 0x0A, "root tag must be Compound");
    }
}

#[test]
#[ignore = "requires local .analysis/test-world-lz4 vanilla region"]
fn lz4_block_api_matches_real_vanilla_lz4_payload() {
    let path = workspace_path(".analysis/test-world-lz4/region/r.0.0.mca");
    assert!(
        path.is_file(),
        "{} not present; run OUT_DIR=.analysis/test-world-lz4 REGION_FILE_COMPRESSION=lz4 tools/generate-test-world.sh",
        path.display()
    );

    let (compression, payload) = first_populated_chunk(&path).unwrap();
    assert_eq!(compression, CompressionType::Lz4 as u8);
    let decoded_len = match count_lz4_bytes(&payload, MAX_DECOMPRESSED_CHUNK_BYTES).unwrap() {
        BoundedLength::Exact(len) => len,
        BoundedLength::TooLarge { at_least } => {
            panic!("real LZ4 chunk unexpectedly exceeds limit: {at_least}")
        }
    };
    let nbt = decompress_lz4_exact(&payload, decoded_len).unwrap();
    assert_eq!(nbt[0], 0x0A);

    let mut frame = lz4_flex::frame::FrameDecoder::new(payload.as_slice());
    let mut frame_out = Vec::new();
    assert!(frame.read_to_end(&mut frame_out).is_err());
}

#[test]
fn mixed_rewrite_preserves_untouched_slot_bytes_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("r.0.0.mca");
    write_region(
        &source,
        &[
            ChunkPayload {
                local_x: 0,
                local_z: 0,
                timestamp: 111,
                uncompressed_nbt: vec![0x0Au8; 512],
            },
            ChunkPayload {
                local_x: 1,
                local_z: 0,
                timestamp: 222,
                uncompressed_nbt: vec![0x0Bu8; 256],
            },
        ],
    )
    .unwrap();
    let raw_before = read_region_raw(&source).unwrap();
    assert_eq!(raw_before.len(), 2);

    let target = dir.path().join("r.0.1.mca");
    let fresh_nbt = vec![0x0Cu8; 128];
    let kept = raw_before
        .into_iter()
        .find(|record| record.local_x == 0)
        .expect("kept slot present");
    write_region_create_new_mixed(
        &target,
        &[
            RegionChunkOutput::Preserved(kept.clone()),
            RegionChunkOutput::Fresh(ChunkPayload {
                local_x: 1,
                local_z: 0,
                timestamp: 333,
                uncompressed_nbt: fresh_nbt.clone(),
            }),
        ],
    )
    .unwrap();

    let raw_after = read_region_raw(&target).unwrap();
    let kept_after = raw_after
        .iter()
        .find(|record| record.local_x == 0)
        .expect("kept slot rewritten");
    assert_eq!(kept_after.timestamp, kept.timestamp);
    assert_eq!(kept_after.compression, kept.compression);
    assert_eq!(kept_after.compressed, kept.compressed);
    let decoded = read_region(&target).unwrap();
    assert_eq!(decoded.len(), 2);
    let fresh = decoded
        .iter()
        .find(|payload| payload.local_x == 1)
        .expect("fresh slot rewritten");
    assert_eq!(fresh.timestamp, 333);
    assert_eq!(fresh.uncompressed_nbt, fresh_nbt);
}

#[test]
fn mixed_rewrite_rejects_unknown_compression_and_duplicate_slots() {
    let dir = tempfile::tempdir().unwrap();
    let bogus = RawChunkRecord {
        local_x: 0,
        local_z: 0,
        timestamp: 1,
        compression: 9,
        compressed: vec![0u8; 16],
        validated_decoded_len: None,
        validated_content_stamp: None,
    };
    assert!(matches!(
        write_region_create_new_mixed(
            dir.path().join("bad-comp.mca"),
            &[RegionChunkOutput::Preserved(bogus)]
        ),
        Err(RegionError::UnknownCompression(9))
    ));

    let fresh = ChunkPayload {
        local_x: 2,
        local_z: 0,
        timestamp: 1,
        uncompressed_nbt: vec![0x0Au8; 32],
    };
    let dupe = RawChunkRecord {
        local_x: 2,
        local_z: 0,
        timestamp: 1,
        compression: CompressionType::Zlib as u8,
        compressed: zlib_payload(&[0x0Au8; 32]),
        validated_decoded_len: None,
        validated_content_stamp: None,
    };
    assert!(matches!(
        write_region_create_new_mixed(
            dir.path().join("dupe.mca"),
            &[
                RegionChunkOutput::Fresh(fresh),
                RegionChunkOutput::Preserved(dupe),
            ]
        ),
        Err(RegionError::DuplicateChunkSlot { .. })
    ));

    let corrupt = RawChunkRecord {
        local_x: 3,
        local_z: 0,
        timestamp: 1,
        compression: CompressionType::Zlib as u8,
        compressed: vec![0xFFu8; 64],
        validated_decoded_len: None,
        validated_content_stamp: None,
    };
    assert!(matches!(
        write_region_create_new_mixed(
            dir.path().join("corrupt.mca"),
            &[RegionChunkOutput::Preserved(corrupt)]
        ),
        Err(RegionError::Decompress { .. })
    ));
}
#[test]
fn raw_read_rejects_undecodable_payload_like_decoded_read() {
    // An ordinary read seeds only its requested slot. The corrupt preserved
    // neighbor has never been read and must still fail the first raw rewrite.
    let good_nbt = vec![0x0Au8; 256];
    let region = synthetic_region_chunks(&[
        (0, CompressionType::Zlib as u8, zlib_payload(&good_nbt)),
        (1, CompressionType::Zlib as u8, vec![0xFFu8; 64]),
    ]);
    assert_eq!(
        read_chunk(region.path(), 0, 0)
            .unwrap()
            .unwrap()
            .uncompressed_nbt,
        good_nbt
    );
    assert!(matches!(
        read_region_raw(region.path()),
        Err(RegionError::Decompress { cx: 1, cz: 0, .. })
    ));
    assert!(matches!(
        read_region(region.path()),
        Err(RegionError::Decompress { cx: 1, cz: 0, .. })
    ));
}

#[test]
fn validated_preserved_records_still_enforce_chunk_and_mixed_region_budgets() {
    let payload = [0xA5; 32];
    let region = synthetic_region(CompressionType::Zlib as u8, &zlib_payload(&payload));
    let preserved = read_region_raw(region.path()).unwrap().pop().unwrap();
    assert!(matches!(
        validate_write_records(
            &[RegionChunkOutput::Preserved(preserved.clone())],
            test_limits(31, 128)
        ),
        Err(RegionError::DecompressedChunkTooLarge { max: 31, .. })
    ));

    let fresh = ChunkPayload {
        local_x: 1,
        local_z: 0,
        timestamp: 1,
        uncompressed_nbt: vec![0x0A; 33],
    };
    let mixed = [
        RegionChunkOutput::Fresh(fresh),
        RegionChunkOutput::Preserved(preserved),
    ];
    assert!(matches!(
        validate_write_records(&mixed, test_limits(64, 64)),
        Err(RegionError::DecompressedRegionTooLarge { bytes: 65, max: 64 })
    ));
    validate_write_records(&mixed, test_limits(64, 65)).unwrap();
}

#[test]
fn verified_slot_rejects_changed_compressed_bytes_with_unchanged_stored_crc() {
    let raw = deterministic_incompressible(8193);
    let compressed = gzip_payload(&raw);
    let region = synthetic_region(CompressionType::Gzip as u8, &compressed);
    // Ordinary decode success seeds the proof used by the preserving read.
    assert_eq!(
        read_chunk(region.path(), 0, 0)
            .unwrap()
            .unwrap()
            .uncompressed_nbt,
        raw
    );
    let verified = read_region_raw(region.path()).unwrap().pop().unwrap();
    assert_eq!(verified.compressed(), compressed);

    let original = std::fs::read(region.path()).unwrap();
    let mut changed = original.clone();
    let payload_start = HEADER_BYTES + 5;
    // Gzip's first deflate block starts after the fixed ten-byte header.
    // BTYPE=3 is invalid; leave its length and stored CRC/ISIZE untouched.
    changed[payload_start + 10] = (changed[payload_start + 10] & !0x06) | 0x06;
    assert_eq!(&changed[..payload_start], &original[..payload_start]);
    let trailer_start = payload_start + compressed.len() - 8;
    assert_eq!(
        &changed[trailer_start..trailer_start + 8],
        &original[trailer_start..trailer_start + 8]
    );
    std::fs::write(region.path(), changed).unwrap();
    assert!(matches!(
        read_region_raw(region.path()),
        Err(RegionError::Decompress { cx: 0, cz: 0, .. })
    ));
    assert!(matches!(
        read_chunk(region.path(), 0, 0),
        Err(RegionError::Decompress { cx: 0, cz: 0, .. })
    ));
}

#[test]
fn region_validation_memos_are_bounded_and_evicted_slots_revalidate() {
    let directory = tempfile::tempdir().unwrap();
    let raw = deterministic_incompressible(8193);
    let compressed = gzip_payload(&raw);
    let source = synthetic_region(CompressionType::Gzip as u8, &compressed);
    let original = std::fs::read(source.path()).unwrap();
    let first = directory.path().join("r.0.0.mca");
    std::fs::write(&first, &original).unwrap();
    assert_eq!(read_region_raw(&first).unwrap()[0].compressed(), compressed);

    for region in 1..=MAX_REGION_VALIDATION_MEMOS {
        let path = directory.path().join(format!("r.{region}.0.mca"));
        std::fs::write(&path, &original).unwrap();
        assert_eq!(read_region_raw(&path).unwrap()[0].compressed(), compressed);
        assert!(REGION_VALIDATION_MEMOS.lock().unwrap().len() <= MAX_REGION_VALIDATION_MEMOS);
    }
    assert!(!REGION_VALIDATION_MEMOS.lock().unwrap().contains_key(&first));

    let mut corrupt = original;
    let deflate_start = HEADER_BYTES + 5 + 10;
    corrupt[deflate_start] = (corrupt[deflate_start] & !0x06) | 0x06;
    std::fs::write(&first, corrupt).unwrap();
    assert!(matches!(
        read_region_raw(&first),
        Err(RegionError::Decompress { cx: 0, cz: 0, .. })
    ));
}

#[test]
fn region_read_bounds_distinct_slot_proofs_without_losing_validation() {
    let initial = gzip_payload(&0_u32.to_be_bytes());
    let region = synthetic_region(CompressionType::Gzip as u8, &initial);
    let memo = region_validation_memo(region.path());
    let mut bytes = std::fs::read(region.path()).unwrap();
    for version in 0..=MAX_SLOT_VALIDATION_PROOFS {
        let raw = (version as u32).to_be_bytes();
        let compressed = gzip_payload(&raw);
        bytes[HEADER_BYTES..HEADER_BYTES + 4]
            .copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        bytes[HEADER_BYTES + 5..HEADER_BYTES + 5 + compressed.len()].copy_from_slice(&compressed);
        std::fs::write(region.path(), &bytes).unwrap();
        assert_eq!(
            read_chunk(region.path(), 0, 0)
                .unwrap()
                .unwrap()
                .uncompressed_nbt,
            raw
        );
        assert!(memo.lock().unwrap().len() <= MAX_SLOT_VALIDATION_PROOFS);
    }

    let deflate_start = HEADER_BYTES + 5 + 10;
    bytes[deflate_start] = (bytes[deflate_start] & !0x06) | 0x06;
    std::fs::write(region.path(), bytes).unwrap();
    assert!(matches!(
        read_chunk(region.path(), 0, 0),
        Err(RegionError::Decompress { cx: 0, cz: 0, .. })
    ));
}

#[test]
fn memoized_raw_slots_keep_chunk_and_aggregate_budget_fences() {
    let raw = [0xA5; 32];
    let neighbor = [0x5A; 32];
    for (compression, compressed, compressed_neighbor) in [
        (
            CompressionType::Gzip as u8,
            gzip_payload(&raw),
            gzip_payload(&neighbor),
        ),
        (
            CompressionType::Zlib as u8,
            zlib_payload(&raw),
            zlib_payload(&neighbor),
        ),
        (
            CompressionType::Uncompressed as u8,
            raw.to_vec(),
            neighbor.to_vec(),
        ),
        (
            CompressionType::Lz4 as u8,
            raw_lz4_block_payload(&raw),
            raw_lz4_block_payload(&neighbor),
        ),
    ] {
        let region = synthetic_region_chunks(&[
            (0, compression, compressed.clone()),
            (1, compression, compressed_neighbor.clone()),
        ]);
        // Seed both proofs under a larger budget, then lower each fence.
        read_region_raw_with_limits(region.path(), test_limits(64, 128)).unwrap();
        assert!(matches!(
            read_region_raw_with_limits(region.path(), test_limits(31, 128)),
            Err(RegionError::DecompressedChunkTooLarge {
                cx: 0,
                cz: 0,
                max: 31,
                ..
            })
        ));
        assert!(matches!(
            read_region_raw_with_limits(region.path(), test_limits(32, 63)),
            Err(RegionError::DecompressedRegionTooLarge { bytes: 64, max: 63 })
        ));
        let preserved = read_region_raw_with_limits(region.path(), test_limits(32, 64)).unwrap();
        assert_eq!(preserved[0].compressed(), compressed);
        assert_eq!(preserved[1].compressed(), compressed_neighbor);
        let decoded = collect_with_limits(region.path(), test_limits(32, 64)).unwrap();
        assert_eq!(decoded[0].uncompressed_nbt, raw);
        assert_eq!(decoded[1].uncompressed_nbt, neighbor);
    }
}

#[test]
fn memoized_payload_never_bypasses_location_and_codec_validation() {
    let region = synthetic_region(CompressionType::Zlib as u8, &zlib_payload(&[7; 32]));
    read_region_raw(region.path()).unwrap();
    let original = std::fs::read(region.path()).unwrap();
    overwrite_location(region.path(), 0, 1, 1);
    assert!(matches!(
        read_region_raw(region.path()),
        Err(RegionError::ReservedSector { cx: 0, cz: 0, .. })
    ));

    let mut changed = original;
    changed[HEADER_BYTES + 4] = 9;
    std::fs::write(region.path(), changed).unwrap();
    assert!(matches!(
        read_region_raw(region.path()),
        Err(RegionError::UnknownCompression(9))
    ));
}

#[test]
fn vanilla_lazy_empty_region_is_empty_but_partial_headers_are_invalid() {
    let file = tempfile::NamedTempFile::new().unwrap();
    assert!(read_region(file.path()).unwrap().is_empty());
    assert!(read_region_raw(file.path()).unwrap().is_empty());
    for bytes in [1, 4096, HEADER_BYTES - 1] {
        file.as_file().set_len(bytes as u64).unwrap();
        assert!(matches!(
            read_region(file.path()),
            Err(RegionError::TooShort { needed: HEADER_BYTES, got }) if got == bytes
        ));
    }
}
