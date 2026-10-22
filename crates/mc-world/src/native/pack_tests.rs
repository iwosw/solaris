use mc_data::Identifier;
use mc_data::blocks::solaris_required_blocks_report;
use mc_data::items::solaris_required_items;
use mc_data::loot::chest_26_1_2::ChestLootCatalog;
use redb::ReadableDatabase;

use super::pack::{PACKED, is_packed, pack, unpack};
use super::*;
use crate::BlockStateId;

const LIMIT: usize = 16 * 1024 * 1024;

fn compressible(len: usize) -> Vec<u8> {
    let mut bytes = vec![0x0A];
    bytes.extend((0..len).map(|index| (index % 7) as u8));
    bytes
}

fn state(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}

fn baseline_chunk(blocks: &BlockRegistry, position: ChunkPos) -> Chunk {
    let mut chunk = Chunk::empty(
        position,
        state(blocks, "minecraft:air"),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for z in 0..16 {
        for x in 0..16 {
            chunk
                .set_block(x, 63, z, state(blocks, "minecraft:stone"))
                .unwrap();
        }
    }
    chunk
}

fn stored_row(path: &std::path::Path, key: &[u8]) -> Vec<u8> {
    let database = redb::Database::open(path).unwrap();
    let transaction = database.begin_read().unwrap();
    let table = transaction.open_table(IMPORTED).unwrap();
    table.get(key).unwrap().unwrap().value().to_vec()
}

fn replace_row(path: &std::path::Path, key: &[u8], bytes: &[u8]) {
    let database = redb::Database::open(path).unwrap();
    let transaction = database.begin_write().unwrap();
    {
        let mut table = transaction.open_table(IMPORTED).unwrap();
        table.insert(key, bytes).unwrap();
    }
    transaction.commit().unwrap();
}

#[test]
fn large_rows_round_trip_and_shrink_while_small_rows_stay_plain() {
    let raw = compressible(64 * 1024);
    let packed = pack(raw.clone()).unwrap();
    assert!(is_packed(&packed));
    assert!(packed.len() < raw.len() / 10, "{} bytes", packed.len());
    assert_eq!(unpack(&packed, LIMIT).unwrap().as_ref(), raw.as_slice());
    // Deterministic: equal input is byte-equal, so stored rows stay comparable.
    assert_eq!(pack(raw.clone()).unwrap(), packed);

    let small = compressible(64);
    assert_eq!(pack(small.clone()).unwrap(), small);
    assert_eq!(unpack(&small, LIMIT).unwrap().as_ref(), small.as_slice());
    // Incompressible data is never inflated.
    let mut noise = vec![0x0A];
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    noise.extend((0..4096).map(|_| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 24) as u8
    }));
    assert_eq!(pack(noise.clone()).unwrap(), noise);
}

#[test]
fn packed_rows_fail_closed_on_any_corruption() {
    let raw = compressible(64 * 1024);
    let packed = pack(raw).unwrap();
    assert_eq!(packed[0], PACKED);

    let mut flipped = packed.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0x40;
    assert!(unpack(&flipped, LIMIT).is_err(), "bit flip");

    assert!(
        unpack(&packed[..packed.len() - 3], LIMIT).is_err(),
        "truncated"
    );
    assert!(unpack(&packed[..3], LIMIT).is_err(), "truncated header");

    let mut trailing = packed.clone();
    trailing.extend_from_slice(&[1, 2, 3, 4]);
    assert!(unpack(&trailing, LIMIT).is_err(), "trailing bytes");

    let mut short = packed.clone();
    short[1..5].copy_from_slice(&1024_u32.to_be_bytes());
    assert!(unpack(&short, LIMIT).is_err(), "declared length too small");
    let mut long = packed.clone();
    long[1..5].copy_from_slice(&(128 * 1024_u32).to_be_bytes());
    assert!(unpack(&long, LIMIT).is_err(), "declared length too large");
    let mut huge = packed.clone();
    huge[1..5].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(unpack(&huge, LIMIT).is_err(), "over budget");
    assert!(unpack(&packed, 1024).is_err(), "caller budget");
}

#[test]
fn stored_baseline_is_packed_decodes_and_a_flipped_byte_is_refused() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let position = ChunkPos { x: 5, z: -9 };
    let chunk = baseline_chunk(&blocks, position);
    {
        let store = NativeStore::create(&path, [3; 32]).unwrap();
        store.import_chunk(0, &chunk, &blocks, &items).unwrap();
    }
    let key = chunk_key(0, position);
    let row = stored_row(&path, &key);
    assert!(is_packed(&row), "baseline rows are stored packed");
    let plain = unpack(&row, LIMIT).unwrap().into_owned();
    assert!(
        row.len() * 4 < plain.len(),
        "packed {} of {} bytes",
        row.len(),
        plain.len()
    );

    let chest_loot = ChestLootCatalog::new();
    let load = |path: &std::path::Path| {
        NativeStore::open(path, [3; 32])?.resolve_base(
            0,
            position,
            &blocks,
            &items,
            &chest_loot,
            || Ok(None),
        )
    };
    let restored = load(&path).unwrap().expect("stored baseline");
    assert_eq!(
        restored.get_block(3, 63, 4),
        Some(state(&blocks, "minecraft:stone"))
    );
    assert_eq!(
        restored.get_block(3, 64, 4),
        Some(state(&blocks, "minecraft:air"))
    );

    // The same chunk stored as plain named NBT (the pre-packing layout) still loads.
    replace_row(&path, &key, &plain);
    assert!(load(&path).unwrap().is_some());

    let mut corrupt = row.clone();
    let middle = corrupt.len() / 2;
    corrupt[middle] ^= 0x01;
    replace_row(&path, &key, &corrupt);
    assert!(
        load(&path).is_err(),
        "corrupt packed baseline must not load"
    );
}
