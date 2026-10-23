use super::*;
use crate::boundary::{BlockPos, ConnectionId, EntityId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::survival::Difficulty;
use mc_data::{Identifier, ItemStack};
use std::sync::{Arc, LazyLock};

pub(crate) fn fixture(root: Option<&std::path::Path>) -> (TickState, EntityId, EntityId) {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let mut world = if let Some(root) = root {
        crate::world::ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&crate::CoreConfig::new(
                root.to_owned(),
                42,
            ))
            .unwrap(),
            0,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 8],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        crate::world::ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 8],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    world.ensure_blocking(
        &[
            (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
            (crate::boundary::ChunkPos { x: 1, z: 0 }, 0),
        ]
        .into_iter()
        .collect(),
    );
    for x in 0..32 {
        for z in 0..16 {
            world.set_block(x, 200, z, registry.states.stone);
            for y in 201..206 {
                world.set_block(x, y, z, registry.states.air);
            }
        }
    }
    world.fold_overlay();
    let mut state = TickState::new(registry, world, 2, 512);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    crate::tick::run_dimension_tick(
        &mut state,
        [&PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: Uuid::from_u128(77),
            name: "Hero".into(),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    let player_slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(player_slot, [8.5, 201.0, 8.5]);
    let villager = mob(&mut state, "villager", [8.5, 201.0, 8.5]);
    for (x, part) in [(4, "foot"), (5, "head")] {
        let id = block(&state, "red_bed", &[("part", part), ("facing", "east")]);
        state.world.set_block(x, 201, 8, id);
    }
    let bell = block(&state, "bell", &[]);
    state.world.set_block(8, 201, 10, bell);
    state.world.fold_overlay();
    state.tick = 20 - u64::from(villager.0.unsigned_abs() % 20);
    state.tick_budget.reset(state.tick);
    crate::settlements::allocator::tick(&mut state, &mut ChangeSet::default());
    crate::villager::brain::tick(&mut state, &mut ChangeSet::default());
    let v = state
        .entities
        .row(state.entities.slot_of(villager).unwrap())
        .villager
        .as_ref()
        .unwrap();
    assert_eq!(v.home, Some(BlockPos { x: 5, y: 201, z: 8 }));
    assert_eq!(
        v.meeting_point,
        Some(BlockPos {
            x: 8,
            y: 201,
            z: 10
        })
    );
    (state, player, villager)
}
fn block(state: &TickState, name: &str, properties: &[(&str, &str)]) -> u32 {
    let id = Identifier::parse(format!("minecraft:{name}")).unwrap();
    let default = state.registry.blocks.block(&id).unwrap().default;
    let mut full = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for &(key, value) in properties {
        full.iter_mut().find(|(k, _)| k == key).unwrap().1 = value.into();
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &full)
        .unwrap()
        .0
}
fn mob(state: &mut TickState, name: &str, position: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    let health = state.registry.blueprint(type_id).unwrap().params.max_health;
    state.spawn_mob(type_id, position, health)
}
fn begin(state: &mut TickState, player: EntityId, amplifier: u8) {
    state
        .players
        .get_mut(&player)
        .unwrap()
        .give_effect(ActiveEffect::new(EffectId::BadOmen, 120000, amplifier));
    raid_omens(state);
    assert!(!state.players[&player].active_effects.has(EffectId::BadOmen));
    assert_eq!(
        state.players[&player]
            .active_effects
            .get(EffectId::RaidOmen)
            .unwrap()
            .duration,
        600
    );
    assert!(state.raids.raids.is_empty());
    state.tick += 599;
    raid_omens(state);
    assert!(state.raids.raids.is_empty());
    state.tick += 1;
    raid_omens(state);
    assert!(
        !state.players[&player]
            .active_effects
            .has(EffectId::RaidOmen)
    );
    assert_eq!(state.raids.raids.len(), 1);
}
fn step(state: &mut TickState) -> ChangeSet {
    let mut changes = ChangeSet::default();
    run(state, &mut changes);
    state.tick += 1;
    changes
}
fn kill_wave(state: &mut TickState) {
    let uuids: Vec<_> = state
        .raids
        .raids
        .values()
        .flat_map(|r| r.members.iter().copied())
        .collect();
    for uuid in uuids {
        let slot = state.entities.slot_of_uuid(uuid).unwrap();
        state.remove_entity(slot);
    }
}
#[test]
fn raids_bad_omen_village_waves_victory_hero_discount_and_gift() {
    let (mut state, player, _) = fixture(None);
    begin(&mut state, player, 0);
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(mc_data::statistics::CustomStat::RaidTrigger),
        1
    );
    for expected in 1..=3 {
        let mut spawns = Vec::new();
        for _ in 0..300 {
            spawns.extend(step(&mut state).entities_spawned);
        }
        let raid = state.raids.raids.values().next().unwrap();
        assert_eq!(raid.groups_spawned, expected);
        assert!(!raid.members.is_empty());
        assert_eq!(spawns.len(), raid.members.len());
        let fields = entity_fields(&state, state.entities.slot_of(spawns[0]).unwrap());
        assert!(fields.contains(&("RaidId".into(), mc_nbt::Tag::Int(2))));
        assert!(fields.contains(&("Wave".into(), mc_nbt::Tag::Int(expected))));
        kill_wave(&mut state);
    }
    for _ in 0..40 {
        step(&mut state);
    }
    assert_eq!(state.raids.raids[&2].status, "victory");
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(mc_data::statistics::CustomStat::RaidWin),
        1
    );
    let hero = state.players[&player]
        .active_effects
        .get(EffectId::HeroOfTheVillage)
        .unwrap();
    assert_eq!((hero.duration, hero.amplifier), (48000, 0));
    let emerald = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    let bread = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:bread").unwrap())
        .unwrap();
    let offer = crate::villager::VillagerTradeOffer {
        cost_a: ItemStack::new(emerald, 20),
        cost_b: None,
        result: ItemStack::new(bread, 1),
        uses: 0,
        max_uses: 12,
        reward_exp: true,
        xp: 1,
        special_price: 0,
        demand: 0,
        price_multiplier: 0.05,
    };
    assert_eq!(
        offer.customer_price(&state.registry.item_rules(), 0, Some(hero.amplifier)),
        14
    );
    state.raids.raids.clear();
    state.tick = 6000;
    assert_eq!(
        step(&mut state).entities_spawned.len(),
        1,
        "hero gifts continue after raid bar expires"
    );
}
#[test]
fn raids_difficulty_counts_and_omen_bonus_wave() {
    let (mut state, player, _) = fixture(None);
    for (difficulty, count) in [
        (Difficulty::Easy, 3),
        (Difficulty::Normal, 5),
        (Difficulty::Hard, 7),
    ] {
        state.raids = Raids::default();
        state.difficulty = difficulty;
        begin(&mut state, player, 1);
        assert_eq!(state.raids.raids[&2].group_count, count);
        for expected in 1..=count + 1 {
            for _ in 0..300 {
                step(&mut state);
            }
            assert_eq!(state.raids.raids[&2].groups_spawned, expected);
            kill_wave(&mut state);
        }
        for _ in 0..40 {
            step(&mut state);
        }
        assert_eq!(state.raids.raids[&2].status, "victory");
    }
}

#[test]
fn inactive_raid_freezes_wave_clock_and_hides_bar_until_village_reactivates() {
    let (mut state, player, _) = fixture(None);
    begin(&mut state, player, 0);
    let before = state.raids.raids[&2].clone();
    let player_slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(player_slot, [10_000.0, 201.0, 10_000.0]);
    state.refresh_active_chunks(state.tick);
    assert!(!state.world.is_active(0, 0));
    for _ in 0..400 {
        assert!(step(&mut state).entities_spawned.is_empty());
    }
    let paused = &state.raids.raids[&2];
    assert_eq!(paused.status, "ongoing");
    assert_eq!(paused.ticks_active, before.ticks_active);
    assert_eq!(paused.cooldown_ticks, before.cooldown_ticks);
    assert_eq!(paused.groups_spawned, 0);
    assert!(bars(&state).is_empty());
    state.entities.set_pos(player_slot, [8.5, 201.0, 8.5]);
    state.refresh_active_chunks(state.tick);
    assert!(state.world.is_active(0, 0));
    step(&mut state);
    assert_eq!(state.raids.raids[&2].cooldown_ticks, before.cooldown_ticks - 1);
    assert_eq!(bars(&state).len(), 1);
}
#[test]
fn raids_villager_death_loses_and_milk_cancels_omen() {
    let (mut state, player, villager) = fixture(None);
    state
        .players
        .get_mut(&player)
        .unwrap()
        .give_effect(ActiveEffect::new(EffectId::BadOmen, 120000, 0));
    raid_omens(&mut state);
    state
        .players
        .get_mut(&player)
        .unwrap()
        .clear_effects(Some(EffectId::RaidOmen));
    raid_omens(&mut state);
    assert!(state.raids.pending.is_empty());
    assert!(state.raids.raids.is_empty());
    begin(&mut state, player, 0);
    state.remove_entity(state.entities.slot_of(villager).unwrap());
    step(&mut state);
    assert_eq!(state.raids.raids[&2].status, "loss");
    assert!(
        !state.players[&player]
            .active_effects
            .has(EffectId::HeroOfTheVillage)
    );
}
#[test]
fn raids_native_codec_path_wal_roundtrip_and_corrupt_records_fail_closed() {
    let (mut state, player, _) = fixture(None);
    begin(&mut state, player, 0);
    for _ in 0..300 {
        step(&mut state);
    }
    let root = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(root.path().to_owned(), 42);
    let _store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let manifest = crate::journal::CheckpointManifest {
        raids: Some(state.raids.clone()),
        ..crate::native_checkpoint::tests::read_manifest(&config).unwrap()
    };
    crate::native_checkpoint::tests::write_manifest(&config, &manifest).unwrap();
    assert_eq!(
        crate::native_checkpoint::tests::read_manifest(&config)
            .unwrap()
            .raids,
        Some(state.raids.clone()),
    );
    let nbt = state.raids.to_nbt();
    let mc_nbt::Tag::Compound(envelope) = &nbt else {
        panic!()
    };
    let mc_nbt::Tag::Compound(data) = &envelope.iter().find(|(k, _)| k == "data").unwrap().1 else {
        panic!()
    };
    let mc_nbt::Tag::List(raids) = &data.iter().find(|(k, _)| k == "raids").unwrap().1 else {
        panic!()
    };
    let mc_nbt::Tag::Compound(raid) = &raids.elements[0] else {
        panic!()
    };
    assert_eq!(
        raid.iter()
            .map(|(k, _)| k.as_str())
            .filter(|k| !k.starts_with("solaris:"))
            .collect::<BTreeSet<_>>(),
        [
            "id",
            "started",
            "active",
            "ticks_active",
            "raid_omen_level",
            "groups_spawned",
            "cooldown_ticks",
            "post_raid_ticks",
            "total_health",
            "group_count",
            "status",
            "center",
            "heroes_of_the_village"
        ]
        .into_iter()
        .collect()
    );
    assert!(
        matches!(&raid.iter().find(|(k,_)|k=="center").unwrap().1,mc_nbt::Tag::IntArray(v) if v.len()==3)
    );
    for (name, tag) in raid {
        match name.as_str() {
            "id" | "raid_omen_level" | "groups_spawned" | "cooldown_ticks" | "post_raid_ticks"
            | "group_count" => assert!(matches!(tag, mc_nbt::Tag::Int(_)), "{name}"),
            "started" | "active" => assert!(matches!(tag, mc_nbt::Tag::Byte(0 | 1)), "{name}"),
            "ticks_active" => assert!(matches!(tag, mc_nbt::Tag::Long(_))),
            "total_health" => assert!(matches!(tag, mc_nbt::Tag::Float(_))),
            "heroes_of_the_village" => assert!(
                matches!(tag, mc_nbt::Tag::List(list) if list.element_type == mc_nbt::tag_type::INT_ARRAY)
            ),
            _ => {}
        }
    }
    let mut duplicate = nbt.clone();
    let mc_nbt::Tag::Compound(envelope) = &mut duplicate else {
        panic!()
    };
    let mc_nbt::Tag::Compound(data) =
        &mut envelope.iter_mut().find(|(k, _)| k == "data").unwrap().1
    else {
        panic!()
    };
    let mc_nbt::Tag::List(raids) = &mut data.iter_mut().find(|(k, _)| k == "raids").unwrap().1
    else {
        panic!()
    };
    let mc_nbt::Tag::Compound(fields) = &mut raids.elements[0] else {
        panic!()
    };
    fields.push(("center".into(), mc_nbt::Tag::IntArray(vec![0; 3])));
    assert_eq!(
        Raids::from_nbt(&duplicate).unwrap_err(),
        "raid-index 0: duplicate raids field center"
    );
    let mut invalid = state.raids.clone();
    let missing = Uuid::from_u128(0xBAD);
    invalid.raids.get_mut(&2).unwrap().members.insert(missing);
    let good = std::mem::replace(&mut state.raids, invalid);
    restore(&mut state).unwrap();
    assert!(!state.raids.raids[&2].members.contains(&missing));
    assert_eq!(
        state.raids, good,
        "ordinary absent member does not corrupt the raid"
    );
    state.raids = good;
    let mut writer = crate::journal::JournalWriter::open(root.path(), 1).unwrap();
    kill_wave(&mut state);
    step(&mut state);
    let tail = state.raids.clone();
    writer
        .append(
            2,
            2,
            &[crate::journal::JournalEntry::Raids {
                value: tail.clone(),
            }],
        )
        .unwrap();
    writer.flush().unwrap();
    let batches =
        crate::journal::read_complete_batches_after(&root.path().join("journal"), 1).unwrap();
    let base = crate::journal::Replay {
        raids: crate::native_checkpoint::tests::read_manifest(&config)
            .unwrap()
            .raids,
        ..Default::default()
    };
    let replay = crate::journal::replay_batches(base, &batches);
    assert_eq!(replay.raids, Some(tail.clone()));
    let replay = crate::journal::replay_batches(replay, &batches);
    assert_eq!(replay.raids, Some(tail));
    let mut corrupt = state.raids.clone();
    corrupt.raids.get_mut(&2).unwrap().group_count = 4;
    let json = serde_json::to_vec(&crate::journal::JournalEntry::Raids { value: corrupt }).unwrap();
    assert!(
        serde_json::from_slice::<crate::journal::JournalEntry>(&json)
            .unwrap_err()
            .to_string()
            .contains("raid 2: invalid retained fields")
    );
}
#[test]
fn raids_melee_and_spell_goals_target_villagers() {
    for name in [
        "vindicator",
        "ravager",
        "evoker",
        "pillager",
        "witch",
        "illusioner",
    ] {
        let (mut state, player, villager) = fixture(None);
        state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
        let victim = state.entities.slot_of(villager).unwrap();
        state.entities.set_pos(victim, [8.5, 201.0, 8.5]);
        let attacker = mob(&mut state, name, [9.5, 201.0, 8.5]);
        let mut intents = Vec::new();
        for _ in 0..140 {
            crate::special_mobs::raiders::run(&mut state, &mut intents);
            state.tick += 1;
        }
        match name {
            "vindicator"|"ravager"=>assert!(intents.iter().any(|i|matches!(i.kind,crate::intent::IntentKind::DamageEntity{target,..} if target==villager)),"{name}"),
            "illusioner"=>assert!(state.entities.row(victim).active_effects.has(EffectId::Blindness)),
            "evoker"=>assert!(state.entities.slots().any(|s|matches!(state.entities.row(s).special_mob.as_deref(),Some(crate::special_mobs::Retained::Vex{..})))),
            _=>assert!(state.entities.slots().any(|s| state.entities.row(s).projectile.is_some()),"{name}"),
        }
        assert!(state.entities.slot_of(attacker).is_some());
    }
}

#[test]
fn raids_hero_gifts_only_on_cadence_and_once_per_nearby_villager() {
    let (mut state, player, _) = fixture(None);
    mob(&mut state, "villager", [100.5, 201.0, 8.5]);
    state.tick = 5998;
    crate::tick::run_dimension_tick(
        &mut state,
        [&PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: Uuid::from_u128(88),
            name: "OtherHero".into(),
            permission_level: 0,
        }],
    );
    let other = state.connections.entity_of(ConnectionId(2)).unwrap();
    for id in [player, other] {
        state
            .entities
            .set_pos(state.entities.slot_of(id).unwrap(), [8.5, 201.0, 8.5]);
        state
            .players
            .get_mut(&id)
            .unwrap()
            .give_effect(ActiveEffect::new(EffectId::HeroOfTheVillage, 48000, 0));
    }
    assert!(
        step(&mut state).entities_spawned.is_empty(),
        "off-cadence gifts are not evaluated"
    );
    assert_eq!(state.tick, 6000);
    assert_eq!(
        step(&mut state).entities_spawned.len(),
        1,
        "distant villagers and overlapping heroes cannot duplicate gifts"
    );
}

#[test]
fn raid_trigger_only_counts_accepted_omens_before_first_wave() {
    use mc_data::statistics::CustomStat;
    let (mut state, player, _) = fixture(None);
    let uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    start(&mut state, [1000, 201, 1000], uuid, 1);
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(CustomStat::RaidTrigger),
        0
    );
    begin(&mut state, player, 0);
    let center = state.raids.raids[&2].center;
    start(&mut state, center, uuid, 1);
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(CustomStat::RaidTrigger),
        2
    );
    state.raids.raids.get_mut(&2).unwrap().groups_spawned = 1;
    start(&mut state, center, uuid, 1);
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(CustomStat::RaidTrigger),
        2
    );
}
