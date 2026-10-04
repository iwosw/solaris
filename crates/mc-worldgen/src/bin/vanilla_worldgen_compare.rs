//! Compare native overworld generation to the ignored headless vanilla oracle.
use mc_data::Identifier;
use mc_world::{BlockRegistry, ChunkGenerator, ChunkPos};
use mc_worldgen::TerrainGenerator;
use mc_worldgen::vanilla_climate::VanillaClimate;
use mc_worldgen::vanilla_features::{BlockSemantics, CacheTags};
use mc_worldgen::village::plan_source::VillagePlanSource;
use mc_worldgen::village::{VillageDecor, load_village_closure};
use serde_json::{Value, json};
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err(
            "usage: vanilla_worldgen_compare <content-root> <oracle.json> <report.json>".into(),
        );
    }
    let root = PathBuf::from(&args[1]);
    let oracle: Value = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let blocks = Arc::new(BlockRegistry::from_report(
        &mc_data::blocks::solaris_required_blocks_report(),
    )?);
    let data = Arc::new(mc_data::load(&root)?);
    let tags = Arc::new(mc_data::tags::load(&root, &data)?);
    let cache_tags = Arc::new(CacheTags::new(blocks.clone(), data, tags));
    let closure = Arc::new(load_village_closure(
        &root,
        &Identifier::parse("minecraft:villages")?,
        &blocks,
    )?);
    let decor = Arc::new(VillageDecor::compile(
        &root,
        &closure,
        &BlockSemantics::new(&blocks, cache_tags.as_ref()),
    )?);
    let mut results = Vec::new();
    for fixture in oracle["seeds"]
        .as_array()
        .ok_or("oracle seeds array missing")?
    {
        let seed = fixture["seed"].as_i64().ok_or("oracle seed missing")?;
        let climate = VanillaClimate::new(seed as u64, &root)?;
        let mut sampler = climate.sampler();
        let mut native = TerrainGenerator::new(seed, blocks.clone());
        native.load_vanilla_worldgen(&root)?;
        let legacy = TerrainGenerator::new(seed, blocks.clone());
        let mut matched = 0usize;
        let mut baseline_matched = 0usize;
        let mut climate_matched = 0usize;
        let mut height_errors = Vec::new();
        let mut mismatch_points = Vec::new();
        let points = fixture["points"]
            .as_array()
            .ok_or("oracle points missing")?;
        for point in points {
            let x = point["x"].as_i64().ok_or("point x")? as i32;
            let y = point["y"].as_i64().ok_or("point y")? as i32;
            let z = point["z"].as_i64().ok_or("point z")? as i32;
            let sample = sampler.sample(x, y, z);
            let expected = point["biome"].as_str().ok_or("point biome")?;
            matched += usize::from(sample.biome == expected);
            baseline_matched += usize::from(legacy.base_biome(x, z).as_str() == expected);
            let expected_climate: Vec<i64> = point["climate"]
                .as_array()
                .ok_or("point climate")?
                .iter()
                .map(|v| v.as_i64().expect("quantized climate"))
                .collect();
            climate_matched += usize::from(sample.quantized.as_slice() == expected_climate);
            let height = native.surface_height(x, z);
            let error = height - point["height"].as_i64().ok_or("point height")? as i32;
            height_errors.push(error.abs());
            if sample.biome != expected && mismatch_points.len() < 20 {
                mismatch_points.push(json!({"x":x,"y":y,"z":z,"vanilla":expected,"solaris":sample.biome,"climate":sample.quantized,"expected_climate":expected_climate}));
            }
        }
        height_errors.sort_unstable();
        native = native.with_village_plans(Arc::new(VillagePlanSource::new(
            closure.clone(),
            decor.clone(),
            seed,
            blocks.clone(),
            cache_tags.clone(),
            cache_tags.clone(),
        )?));
        let sites = native.village_sites_in_region((-68, -68), (101, 101));
        let actual: Vec<_> = sites
            .iter()
            .map(|s| json!({"chunk_x":s.start_chunk.0,"chunk_z":s.start_chunk.1}))
            .collect();
        let expected: Vec<_> = fixture["structures"]
            .as_array()
            .ok_or("oracle structures missing")?
            .iter()
            .filter(|s| s["set"] == "minecraft:villages")
            .cloned()
            .collect();
        let matching: Vec<_> = expected
            .iter()
            .filter(|s| {
                actual
                    .iter()
                    .any(|a| a["chunk_x"] == s["chunk_x"] && a["chunk_z"] == s["chunk_z"])
            })
            .cloned()
            .collect();
        let missing: Vec<_> = expected
            .iter()
            .filter(|s| {
                !actual
                    .iter()
                    .any(|a| a["chunk_x"] == s["chunk_x"] && a["chunk_z"] == s["chunk_z"])
            })
            .cloned()
            .collect();
        let extra: Vec<_> = actual
            .iter()
            .filter(|a| {
                !expected
                    .iter()
                    .any(|s| a["chunk_x"] == s["chunk_x"] && a["chunk_z"] == s["chunk_z"])
            })
            .cloned()
            .collect();
        let smoke = if let Some(site) = sites.first() {
            let chunk = native.generate(ChunkPos {
                x: site.start_chunk.0,
                z: site.start_chunk.1,
            });
            json!({"chunk_x":site.start_chunk.0,"chunk_z":site.start_chunk.1,"status":chunk.status,"villagers":chunk.generated_entities().len(),"chests":chunk.chests.len()})
        } else {
            json!(null)
        };
        results.push(json!({"seed":seed,"samples":points.len(),"biome_matches":matched,"biome_match_percent":100.0*matched as f64/points.len() as f64,"baseline_biome_matches":baseline_matched,"baseline_match_percent":100.0*baseline_matched as f64/points.len() as f64,"exact_climate_matches":climate_matched,"height_mean_absolute_error":height_errors.iter().map(|v|*v as f64).sum::<f64>()/points.len() as f64,"height_median_absolute_error":height_errors[points.len()/2],"height_p95_absolute_error":height_errors[points.len()*95/100],"height_max_absolute_error":height_errors.last(),"mismatch_points":mismatch_points,"village_matching":matching,"village_missing":missing,"village_extra":extra,"village_smoke":smoke,"other_vanilla_starts":fixture["structures"].as_array().unwrap().iter().filter(|s|s["set"]!="minecraft:villages").collect::<Vec<_>>() }));
    }
    std::fs::write(
        &args[3],
        serde_json::to_string_pretty(&json!({"version":oracle["version"],"seeds":results}))? + "\n",
    )?;
    println!("wrote {}", args[3]);
    Ok(())
}
