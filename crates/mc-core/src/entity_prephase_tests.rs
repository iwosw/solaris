//! Entity-phase pre-phase and serial-apply checks: species ids behind the
//! per-row name comparisons, plus the native-100-like release micro.

use super::*;

/// The id helpers replace per-row contract-name and blueprint-name compares;
/// they must agree with those for every protocol id.
#[test]
fn species_id_helpers_match_contract_and_blueprint_names() {
    let registry = CoreRegistry::load(None).expect("registry");
    for type_id in 0..mc_data::entity_types::ENTITY_TYPE_COUNT as u32 {
        let name = crate::piglin::name(type_id);
        assert_eq!(
            crate::piglin::is_piglin_type(type_id),
            name == "minecraft:piglin"
        );
        assert_eq!(
            crate::piglin::is_zoglin_type(type_id),
            name == "minecraft:zoglin"
        );
        assert_eq!(
            crate::piglin::is_hoglin_type(type_id),
            name == "minecraft:hoglin"
        );
        let blueprint = registry.blueprint(type_id).map_or("", |b| b.name.as_str());
        assert_eq!(
            crate::piglin::is_iron_golem_type(type_id),
            blueprint == "minecraft:iron_golem"
        );
        assert_eq!(
            crate::piglin::hunts_villagers_type(type_id),
            matches!(blueprint, "minecraft:zombie" | "minecraft:zombie_villager")
        );
    }
}

/// Native-100-like population through the real `run_tick`; phases are read from
/// MC_CORE_PHASE_LOG output by the caller.
#[test]
#[ignore = "release micro: native-100-like 105k mixed mobs"]
fn native100_like_entity_phase_micro() {
    use crate::boundary::{ConnectionId, PlayerInput};
    const SIDE: i32 = 384;
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(Arc::clone(&registry), world, 2, 512);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.day_time = 14_000;
    for x in 0..SIDE / 16 {
        for z in 0..SIDE / 16 {
            state
                .tickets
                .insert(format!("n100-{x}-{z}"), (ChunkPos { x, z }, 0));
        }
    }
    let joins: Vec<_> = (1..=100u64)
        .map(|connection| PlayerInput::Join {
            connection: ConnectionId(connection),
            uuid: uuid::Uuid::from_u128(u128::from(connection)),
            name: format!("bot{connection}"),
            permission_level: 0,
        })
        .collect();
    crate::tick::run_tick(&mut state, &joins);
    let mut rng = 0x4852_4F44_4531u64;
    let mut next = move || {
        rng = rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) >> 11
    };
    let mut position = move || {
        let unit = |bits: u64| bits as f64 / (1u64 << 53) as f64;
        [
            unit(next()) * f64::from(SIDE),
            64.0,
            unit(next()) * f64::from(SIDE),
        ]
    };
    for connection in 1..=100u64 {
        let id = state
            .connections
            .entity_of(ConnectionId(connection))
            .unwrap();
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.set_pos(slot, position());
        state.entities.row_mut(slot).protected_until = u64::MAX;
        if std::env::var_os("N100_SURVIVAL").is_none() {
            state.players.get_mut(&id).unwrap().game_mode = crate::commands::GameMode::Creative;
        }
    }
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    for name in [
        "minecraft:chicken",
        "minecraft:cow",
        "minecraft:creeper",
        "minecraft:pig",
        "minecraft:sheep",
        "minecraft:skeleton",
        "minecraft:spider",
        "minecraft:zombie",
    ] {
        let type_id = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap();
        for _ in 0..13_000 {
            let id = state.spawn_mob(type_id, position(), 20.0);
            let slot = state.entities.slot_of(id).unwrap();
            state.entities.row_mut(slot).persistent = true;
        }
    }
    let villager = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    for _ in 0..64 {
        let id = state.spawn_mob(villager, position(), 20.0);
        state
            .entities
            .row_mut(state.entities.slot_of(id).unwrap())
            .persistent = true;
    }
    for _ in 0..60 {
        let start = std::time::Instant::now();
        crate::tick::run_tick(&mut state, &[]);
        eprintln!(
            "NATIVE100 total_ms={:.2}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}
