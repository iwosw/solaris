use super::*;
use crate::boundary::{BlockPos, ChunkPos};

fn cohort_joins(first: u64, last: u64) -> Vec<PlayerInput> {
    (first..=last)
        .map(|id| PlayerInput::Join {
            connection: ConnectionId(id),
            uuid: uuid::Uuid::from_u128(u128::from(id)),
            name: format!("Cohort{id}"),
            permission_level: 0,
        })
        .collect()
}

/// Same 25-quarter population as MIX-01, with flat prelit terrain so this
/// receipt isolates cohort finalization rather than worldgen or edge fanout.
#[test]
#[ignore = "manual 75-player cohort / 10k-villager performance receipt"]
fn cohort_arrival_benchmark() {
    let registry = Arc::new(full_registry());
    let world = flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    run_tick(&mut state, &cohort_joins(1, 25));
    for quarter in 0..25 {
        let x = 384 + (quarter % 5) * 128;
        let z = 384 + (quarter / 5) * 128;
        let id = state
            .connections
            .entity_of(ConnectionId(quarter as u64 + 1))
            .unwrap();
        let slot = state.entities.slot_of(id).unwrap();
        state
            .entities
            .set_pos(slot, [f64::from(x) + 88.5, 64.0, f64::from(z) + 56.5]);
        state.tickets.insert(
            format!("cohort-bench-{quarter}"),
            (
                ChunkPos {
                    x: x / 16 + 3,
                    z: z / 16 + 3,
                },
                4,
            ),
        );
    }
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let block = |name: &str, props: &[(&str, &str)]| {
        let id = mc_data::Identifier::parse(name).unwrap();
        let default = state.registry.blocks.block(&id).unwrap().default;
        let mut full = state
            .registry
            .blocks
            .by_id(default)
            .unwrap()
            .properties
            .clone();
        for (key, value) in props {
            full.iter_mut().find(|(name, _)| name == key).unwrap().1 = (*value).to_owned();
        }
        state
            .registry
            .blocks
            .by_name_and_props(&id, &full)
            .unwrap()
            .0
    };
    let composter = block("minecraft:composter", &[]);
    let foot = block("minecraft:red_bed", &[("part", "foot"), ("facing", "east")]);
    let head = block("minecraft:red_bed", &[("part", "head"), ("facing", "east")]);
    let villager_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    for quarter in 0..25 {
        let ox = 384 + (quarter % 5) * 128;
        let oz = 384 + (quarter / 5) * 128;
        for index in 0..400 {
            let x = ox + (index % 20) * 4;
            let z = oz + (index / 20) * 4;
            state.world.set_block(x + 1, 64, z + 2, foot);
            state.world.set_block(x + 2, 64, z + 2, head);
            state.world.set_block(x, 64, z, composter);
            let id = state.spawn_mob(
                villager_type,
                [f64::from(x) + 1.5, 64.0, f64::from(z) + 1.5],
                20.0,
            );
            let slot = state.entities.slot_of(id).unwrap();
            let villager = state.entities.row_mut(slot).villager.as_mut().unwrap();
            villager.home = Some(BlockPos {
                x: x + 2,
                y: 64,
                z: z + 2,
            });
            villager.job_site = Some(BlockPos { x, y: 64, z });
            villager.data.profession = crate::villager::VillagerProfession::Farmer;
        }
        for index in 0..200 {
            state.world.set_block(
                ox + (index % 20) * 4,
                64,
                oz + 84 + (index / 20) * 4,
                composter,
            );
        }
    }
    let stone = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    for _ in 0..1457 {
        crate::tick::spawn_item_entity(
            &mut state,
            [8.5, 64.0, 8.5],
            mc_data::ItemStack::new(stone, 1),
        );
    }
    state.world.fold_overlay();
    state.day_time = 2000;
    for _ in 0..1000 {
        let (output, _, _) = run_tick(&mut state, &[]);
        std::hint::black_box(crate::core::journal_entries_for_tick(&mut state, &output));
    }
    // Keep spawn terrain resident before the cohort arrives; no chunk load is
    // part of the comparison. Existing villagers remain in their 25 quarters.
    state
        .tickets
        .insert("cohort-spawn".into(), (ChunkPos { x: 0, z: 0 }, 3));
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let joins = cohort_joins(26, 100);
    let mut samples = Vec::new();
    for iteration in 0..40 {
        let start = std::time::Instant::now();
        let (output, _, _) = run_tick(&mut state, if iteration == 0 { &joins } else { &[] });
        let core_us = start.elapsed().as_micros();
        let journal_start = std::time::Instant::now();
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        let journal_us = journal_start.elapsed().as_micros();
        samples.push(serde_json::json!({
            "tick": state.tick,
            "players": state.players.len(),
            "core_us": core_us,
            "journal_us": journal_us,
            "total_us": core_us + journal_us,
            "journal_entries": entries.len(),
        }));
        std::hint::black_box((output, entries));
    }
    assert_eq!(state.players.len(), 100);
    let maximum = samples
        .iter()
        .map(|sample| sample["total_us"].as_u64().unwrap())
        .max()
        .unwrap();
    let receipt = serde_json::json!({
        "scene": "75-player cohort, 25 existing players, 10k villagers, 1457 clearing drops",
        "warmup_ticks": 1000,
        "scope": "complete in-process core tick plus journal-image capture; excludes edge fanout and writer I/O",
        "max_us": maximum,
        "samples": samples,
    });
    if let Some(path) = std::env::var_os("COHORT_RECEIPT") {
        let path = std::path::PathBuf::from(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
    println!(
        "cohort_arrival max_us={maximum} first={}",
        receipt["samples"][0]
    );
    assert!(
        maximum <= 60_000,
        "cohort window exceeds 60 ms: {maximum} us"
    );
}
