//! Reproducible native chunk residency/storage probe; run only in an authorized quiet window.
use mc_world::native::NativeStore;
use mc_world::{BlockRegistry, Chunk, ChunkGenerator, ChunkPos};
use mc_worldgen::TerrainGenerator;
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde_json::json;
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Instant};

fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")
                .map(|s| s.split_whitespace().next().unwrap().parse::<u64>().unwrap() * 1024)
        })
        .unwrap()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 {
        return Err(
            "usage: chunk_storage_probe <content> <fixture-dir> <output-dir> <count>".into(),
        );
    }
    let count: usize = args[4].parse()?;
    assert!(count > 0);
    let blocks = Arc::new(BlockRegistry::from_report(
        &mc_data::blocks::solaris_required_blocks_report(),
    )?);
    let items = mc_data::items::solaris_required_items();
    let loot = mc_data::loot::chest_26_1_2::ChestLootCatalog::new();
    let fixture = Path::new(&args[2]);
    std::fs::create_dir_all(fixture)?;
    let path = fixture.join("state.redb");
    if !path.exists() {
        let mut generator = TerrainGenerator::new(42, blocks.clone());
        generator.load_vanilla_worldgen(Path::new(&args[1]))?;
        let store = NativeStore::create(&path, [42; 32])?;
        for start in (0..count).step_by(32) {
            let chunks: Vec<_> = (start..count.min(start + 32))
                .map(|i| generator.generate(position(i)))
                .collect();
            store.fill_generated(0, &chunks, &blocks, &items)?;
            eprintln!("generated {} / {count}", start + chunks.len());
        }
    }
    let store = NativeStore::open(&path, [42; 32])?;
    let before = rss();
    let mut chunks = Vec::with_capacity(count);
    let mut load_us = Vec::with_capacity(count);
    for i in 0..count {
        let start = Instant::now();
        let chunk = store
            .resolve_base(0, position(i), &blocks, &items, &loot, || Ok(None))?
            .ok_or("fixture chunk missing")?;
        load_us.push(start.elapsed().as_micros() as u64);
        chunks.push(chunk);
    }
    let light_table =
        mc_data::block_light::load(Path::new(&args[1]).join("reports/block_light.json"))?;
    {
        let mut workspace = mc_world::light::LightWorkspace::new();
        for chunk in &mut chunks {
            let light = mc_world::light::compute_chunk_light_in(
                &mut workspace,
                [
                    [None, None, None],
                    [None, Some(chunk), None],
                    [None, None, None],
                ],
                &light_table,
            );
            chunk.set_baked_light(&light);
        }
    }
    let loaded = rss();
    let mut bytes: BTreeMap<&str, usize> = BTreeMap::new();
    let mut unused = 0;
    let mut single_candidates = 0;
    let mut narrower_candidates = 0;
    for chunk in &chunks {
        *bytes.entry("chunk_inline").or_default() += size_of::<Chunk>();
        *bytes.entry("sections_inline").or_default() +=
            chunk.sections.capacity() * size_of::<mc_world::ChunkSection>();
        *bytes.entry("biomes_inline").or_default() +=
            chunk.biomes.capacity() * size_of::<mc_world::chunk::BiomeSection>();
        *bytes.entry("light_inline").or_default() +=
            chunk.section_lights.capacity() * size_of::<mc_world::chunk::SectionLight>();
        for section in &chunk.sections {
            if let (Some(palette), Some(indices)) = (section.palette(), section.indices()) {
                *bytes.entry("palettes_logical").or_default() += size_of_val(palette);
                *bytes.entry("indices_logical").or_default() += size_of_val(indices.words());
                *bytes.entry("section_arc_inline").or_default() += 2 * size_of::<usize>()
                    + size_of::<Vec<mc_world::BlockStateId>>()
                    + size_of::<mc_world::section::PackedBitArray>();
                let mut used = vec![false; palette.len()];
                for cell in 0..4096 {
                    used[indices.get(cell) as usize] = true;
                }
                let live = used.iter().filter(|v| **v).count();
                unused += palette.len() - live;
                single_candidates += usize::from(live == 1);
                narrower_candidates += usize::from(
                    live > 1 && (live - 1).ilog2() + 1 < u32::from(indices.bits_per_entry()),
                );
            }
        }
        for biome in &chunk.biomes {
            *bytes.entry("biome_identifiers_logical").or_default() += biome
                .palette()
                .iter()
                .map(|id| id.as_str().len())
                .sum::<usize>();
            if let mc_world::chunk::BiomeSection::Indirect { palette, indices } = biome {
                *bytes.entry("biome_palettes_capacity").or_default() +=
                    palette.capacity() * size_of::<mc_data::Identifier>();
                *bytes.entry("biome_indices_logical").or_default() += size_of_val(indices.words());
            }
        }
        for map in chunk
            .heightmaps
            .values()
            .chain(std::iter::once(&chunk.highest_opaque))
        {
            *bytes.entry("heightmap_words").or_default() += size_of_val(map.words());
        }
        for layer in chunk
            .section_lights
            .iter()
            .chain(&chunk.boundary_section_lights)
        {
            for light in [&layer.sky, &layer.block].into_iter().flatten() {
                *bytes.entry("light_heap").or_default() += light.allocated_bytes();
            }
        }
        for raw in chunk.block_entities.values() {
            *bytes.entry("opaque_block_entity_capacity").or_default() += raw.capacity();
        }
        *bytes.entry("typed_block_entity_slots").or_default() += chunk.chests.capacity()
            * size_of::<(mc_world::BlockPos, mc_world::chunk::ChestBlockEntity)>()
            + chunk.furnaces.capacity()
                * size_of::<(mc_world::BlockPos, mc_world::chunk::FurnaceBlockEntity)>()
            + chunk.hoppers.capacity()
                * size_of::<(mc_world::BlockPos, mc_world::chunk::HopperBlockEntity)>();
        *bytes.entry("extras_heap").or_default() += chunk.extras.capacity()
            * size_of::<(String, mc_nbt::Tag)>()
            + chunk
                .extras
                .iter()
                .map(|(k, v)| k.capacity() + tag_heap(v))
                .sum::<usize>();
    }
    let output = Path::new(&args[3]);
    std::fs::create_dir(output)?;
    let dest = output.join("state.redb");
    let saved = NativeStore::create(&dest, [42; 32])?;
    let start = Instant::now();
    saved.fill_generated(0, &chunks, &blocks, &items)?;
    let save_us = start.elapsed().as_micros();
    drop(saved);
    let db = redb::Database::builder().open_read_only(&dest)?;
    let tx = db.begin_read()?;
    let mut rows = BTreeMap::new();
    for name in ["imported_chunks", "chunk_metadata", "section_overlays"] {
        let table = tx.open_table(TableDefinition::<&[u8], &[u8]>::new(name))?;
        let (mut records, mut payload) = (0, 0);
        for row in table.iter()? {
            let (_, value) = row?;
            records += 1;
            payload += value.value().len();
        }
        rows.insert(name, json!({"records":records,"payload_bytes":payload}));
    }
    load_us.sort_unstable();
    let result = json!({"seed":42,"count":count,"terrain":"TerrainGenerator with load_vanilla_worldgen; no village decor","rss_before_bytes":before,"rss_loaded_bytes":loaded,"rss_delta_bytes":loaded.saturating_sub(before),"attribution_bytes":bytes,"attribution_scope":"logical packed buffers and exposed capacities; excludes allocator overhead, map buckets and nested item components; not total resident bytes","unused_palette_entries":unused,"single_candidates":single_candidates,"narrower_candidates":narrower_candidates,"load_us_p50":load_us[count/2],"load_us_p95":load_us[count*95/100],"save_encode_commit_us":save_us,"disk_bytes":std::fs::metadata(&dest)?.len(),"rows":rows,"wal_bytes":0,"wal_scope":"offline fill_generated does not produce a tick WAL; no gameplay/tick claim"});
    std::fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    println!("{result}");
    Ok(())
}
fn position(i: usize) -> ChunkPos {
    ChunkPos {
        x: (i % 33) as i32 - 16,
        z: (i / 33) as i32 - 16,
    }
}
fn tag_heap(tag: &mc_nbt::Tag) -> usize {
    use mc_nbt::Tag;
    match tag {
        Tag::ByteArray(v) => v.capacity(),
        Tag::IntArray(v) => v.capacity() * 4,
        Tag::LongArray(v) => v.capacity() * 8,
        Tag::String(v) => v.capacity(),
        Tag::List(v) => {
            v.elements.capacity() * size_of::<Tag>()
                + v.elements.iter().map(tag_heap).sum::<usize>()
        }
        Tag::Compound(v) => {
            v.capacity() * size_of::<(String, Tag)>()
                + v.iter()
                    .map(|(k, v)| k.capacity() + tag_heap(v))
                    .sum::<usize>()
        }
        _ => 0,
    }
}
