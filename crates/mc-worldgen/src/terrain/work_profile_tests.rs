//! Hardware-counter workloads. Run with perf stat, not as wall-clock benchmarks.
use super::*;

#[test]
#[ignore = "explicit release instruction-count workload"]
fn density_work() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    for seed in [712816, 5617830, 42] {
        let density = crate::vanilla_density::VanillaDensity::new(seed, &root).unwrap();
        let mut values = vec![0.0; density.height() as usize * 256];
        for z in -3..3 {
            for x in -3..3 {
                density.fill_density_chunk(x * 16, z * 16, &mut values);
                std::hint::black_box(&values);
            }
        }
    }
    println!("WORK density chunks=108");
}

#[test]
#[ignore = "explicit release instruction-count workload"]
fn lighting_work() {
    let report = mc_data::blocks::solaris_required_blocks_report();
    let table = mc_data::block_light::BlockLightTable::conservative_from_blocks_report(&report);
    let mut workspace = mc_world::light::LightWorkspace::new();
    for seed in [712816, 5617830, 42] {
        let generator = generator(seed);
        let chunks: Vec<_> = (-3..3)
            .flat_map(|z| (-3..3).map(move |x| ChunkPos { x, z }))
            .map(|pos| generator.generate(pos))
            .collect();
        for _ in 0..20 {
            for z in 1..5 {
                for x in 1..5 {
                    let neighbours = std::array::from_fn(|dz| {
                        std::array::from_fn(|dx| Some(&chunks[(z + dz - 1) * 6 + x + dx - 1]))
                    });
                    std::hint::black_box(mc_world::light::compute_chunk_light_in(
                        &mut workspace,
                        neighbours,
                        &table,
                    ));
                }
            }
        }
    }
    println!("WORK lighting chunks=960 setup_generated_chunks=108");
}

#[test]
#[ignore = "explicit full chunk and light byte fixture"]
fn chunk_light_bytes() {
    use std::io::Write;
    let path = std::env::var_os("GENPERF_SNAPSHOT").expect("GENPERF_SNAPSHOT output path");
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let report = mc_data::blocks::solaris_required_blocks_report();
    let table = mc_data::block_light::BlockLightTable::conservative_from_blocks_report(&report);
    let mut workspace = mc_world::light::LightWorkspace::new();
    for seed in [712816_i64, 5617830, 42] {
        let generator = generator(seed);
        let chunks: Vec<_> = (-3..3)
            .flat_map(|z| (-3..3).map(move |x| ChunkPos { x, z }))
            .map(|pos| generator.generate(pos))
            .collect();
        for chunk in &chunks {
            let payload =
                mc_world::anvil::chunk_nbt::chunk_to_payload(chunk, &generator.blocks, 0).unwrap();
            out.write_all(&(payload.uncompressed_nbt.len() as u64).to_le_bytes())
                .unwrap();
            out.write_all(&payload.uncompressed_nbt).unwrap();
            for z in 0..16 {
                for x in 0..16 {
                    out.write_all(&chunk.highest_opaque.get(x, z).to_le_bytes())
                        .unwrap();
                }
            }
        }
        for z in 1..5 {
            for x in 1..5 {
                let neighbours = std::array::from_fn(|dz| {
                    std::array::from_fn(|dx| Some(&chunks[(z + dz - 1) * 6 + x + dx - 1]))
                });
                let light =
                    mc_world::light::compute_chunk_light_in(&mut workspace, neighbours, &table);
                for y in 0..384 {
                    for lz in 0..16 {
                        for lx in 0..16 {
                            out.write_all(&[light.sky.get(lx, y, lz), light.block.get(lx, y, lz)])
                                .unwrap();
                        }
                    }
                }
            }
        }
    }
    out.flush().unwrap();
    println!("SNAPSHOT chunks=108 lights=48");
}
