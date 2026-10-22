use super::*;
use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
fn fixture(range: f64) -> (TickState, EntityId, EntityId, [f64; 3]) {
    let (mut state, player, pos) = crate::hostile_tests::setup(range);
    state.difficulty = crate::survival::Difficulty::Normal;
    state.tick = 0;
    let ty = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:warden").unwrap())
        .unwrap();
    let mob = state.spawn_mob(ty, pos, 500.0);
    state.hostile.spawned.push(mob);
    (state, mob, player, pos)
}
fn step(state: &mut TickState, mob: EntityId, player: EntityId, tick: u64) -> Vec<Intent> {
    state.tick = tick;
    let targets = state
        .entities
        .slot_of(player)
        .map(|s| vec![(player, state.entities.pos(s))])
        .unwrap_or_default();
    let mut intents = Vec::new();
    crate::special_mobs::run(
        state,
        &mut intents,
        mob,
        crate::blueprint::GoalKind::SonicBoom,
        &targets,
    );
    intents
}
fn value(state: &TickState, mob: EntityId) -> &Warden {
    match state
        .entities
        .row(state.entities.slot_of(mob).unwrap())
        .special_mob
        .as_deref()
    {
        Some(Retained::Warden(v)) => v,
        _ => panic!("warden"),
    }
}
#[test]
fn emerging_wire_data_then_standing_and_digging_removal() {
    let (mut state, mob, player, _) = fixture(30.0);
    emerging(&mut state, mob);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(state.entities.row(slot).appearance.pose, 13);
    assert_eq!(state.entities.row(slot).appearance.spawn_data, 1);
    assert_eq!(state.entities.body(slot).height, 1.0);
    if let Some(Retained::Warden(value)) = state.entities.row_mut(slot).special_mob.as_deref_mut() {
        value.sniff_at = 5000;
    }
    step(&mut state, mob, player, 134);
    assert_eq!(value(&state, mob).pose, 0);
    assert_eq!(state.entities.row(slot).appearance.spawn_data, 0);
    step(&mut state, mob, player, 1201);
    assert_eq!(value(&state, mob).pose, 14);
    step(&mut state, mob, player, 1301);
    assert!(state.entities.slot_of_any(mob).is_none());
}
#[test]
fn vibrations_anger_roar_darkness_then_melee() {
    let (mut state, mob, player, pos) = fixture(2.0);
    for tick in [0, 40, 80] {
        state.tick = tick;
        vibration(&mut state, pos, Some(player));
    }
    assert_eq!(value(&state, mob).anger.values().copied().max(), Some(105));
    step(&mut state, mob, player, 80);
    assert_eq!(value(&state, mob).pose, 11);
    assert!(
        state.players[&player]
            .active_effects
            .has(EffectId::Darkness)
    );
    let hits = step(&mut state, mob, player, 164);
    assert!(hits.iter().any(|i|matches!(i.kind,crate::intent::IntentKind::DamageEntity{target,amount:30.0,kind:crate::survival::DamageKind::Melee,..} if target==player)));
}
#[test]
fn sniff_adds_anger_and_calm_warden_never_attacks() {
    let (mut state, mob, player, _) = fixture(4.0);
    assert!(step(&mut state, mob, player, 0).is_empty());
    assert!(step(&mut state, mob, player, 100).is_empty());
    assert_eq!(value(&state, mob).pose, 12);
    assert_eq!(value(&state, mob).anger.values().copied().max(), Some(35));
}
#[test]
fn anger_checkpoint_and_replay_restore_uuid_and_pose() {
    let (mut state, mob, player, pos) = fixture(8.0);
    state.tick = 40;
    vibration(&mut state, pos, Some(player));
    emerging(&mut state, mob);
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let slot = state.entities.slot_of(mob).unwrap();
    let row = state.entities.row(slot);
    let expected = value(&state, mob).clone();
    let root = std::env::temp_dir().join(format!("solaris-warden-{}", uuid::Uuid::new_v4()));
    let (_, regions, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let checkpoint = regions
        .values()
        .flat_map(|entities| entities.entities.iter())
        .find(|e| e.uuid == row.uuid)
        .unwrap();
    assert_eq!(
        crate::special_mobs::persistence::from_record(checkpoint).unwrap(),
        Some(Retained::Warden(expected.clone()))
    );
    std::fs::create_dir_all(&root).unwrap();
    let mut writer = JournalWriter::open(&root, 1).unwrap();
    writer.append(1, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    let replay = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&root.join("journal"), 0).unwrap(),
    );
    let record = &replay.entities[&mob.0];
    state.remove_entity(slot);
    crate::core::restore_entity(&mut state, mob.0, record).unwrap();
    assert_eq!(state.entities.warden_ids(), &[mob]);
    assert_eq!(value(&state, mob), &expected);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(mob).unwrap())
            .appearance
            .pose,
        13
    );
    drop(writer);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn vanilla_anger_codec_rejects_duplicate_and_out_of_range_suspects() {
    let uuid = uuid::Uuid::from_u128(7);
    let mut warden = Warden::default();
    warden.anger.insert(uuid, 100);
    let tag = anger_tag(&warden);
    assert_eq!(read_anger(Some(&tag)).unwrap().anger, warden.anger);
    let Tag::Compound(mut fields) = tag else {
        panic!()
    };
    let Tag::List(list) = &mut fields[0].1 else {
        panic!()
    };
    list.elements.push(list.elements[0].clone());
    assert_eq!(
        read_anger(Some(&Tag::Compound(fields)))
            .unwrap_err()
            .to_string(),
        "storage: Warden.anger: suspects.uuid: duplicate UUID"
    );
    let mut invalid = Warden::default();
    invalid.anger.insert(uuid, 151);
    assert_eq!(
        validate(&invalid).unwrap_err(),
        "Warden.anger: expected 0..=150"
    );
}

#[test]
fn repeated_emits_skip_nonwarden_population_and_distant_warden_bodies() {
    let (mut state, player, pos) = crate::hostile_tests::setup(8.0);
    let cow = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    for n in 0..2048 {
        state.spawn_mob(
            cow,
            [
                pos[0] + f64::from(n % 16),
                pos[1],
                pos[2] + f64::from(n / 16),
            ],
            10.0,
        );
    }
    assert!(state.entities.warden_ids().is_empty());
    for distant in [false, true] {
        if distant {
            let ty = state
                .registry
                .entity_types
                .id_of(&mc_data::Identifier::parse("minecraft:warden").unwrap())
                .unwrap();
            state.spawn_mob(ty, [pos[0] + 100.0, pos[1], pos[2]], 500.0);
        }
        VIBRATION_WORK.with(|work| work.set((0, 0, 0)));
        for tick in 0..1000 {
            state.tick = tick;
            crate::vibration::emit(
                &mut state,
                pos,
                crate::vibration::GameEvent::Step,
                Some(player),
            );
        }
        VIBRATION_WORK.with(|work| {
            assert_eq!(
                work.get(),
                (1000, 0, 0),
                "distant={distant}: no body scan or occlusion raycast"
            )
        });
    }
}

#[test]
fn sparse_warden_index_tracks_transfer_and_swap_removal() {
    let (mut source, mob, _, pos) = fixture(8.0);
    let (mut destination, _, _) = crate::hostile_tests::setup(8.0);
    let cow = source
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let moved = source.spawn_mob(cow, pos, 10.0);
    assert_eq!(source.entities.warden_ids(), &[mob]);
    let slot = source.entities.slot_of(mob).unwrap();
    let destination_slot =
        source
            .entities
            .transfer_to(slot, &mut destination.entities, pos, [0.0, 0.0]);
    assert!(source.entities.warden_ids().is_empty());
    assert!(source.entities.slot_of(moved).is_some());
    assert_eq!(destination.entities.warden_ids(), &[mob]);
    assert_eq!(destination.entities.body(destination_slot).id, mob);
    destination.remove_entity(destination_slot);
    assert!(destination.entities.warden_ids().is_empty());
}

#[test]
fn sparse_warden_index_retains_native_identity_before_vibration() {
    let (mut state, original, player, pos) = fixture(8.0);
    let slot = state.entities.slot_of(original).unwrap();
    let uuid = state.entities.row(slot).uuid;
    let (_, regions, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let entity = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == uuid)
        .unwrap();
    let payload = Tag::Compound(
        mc_data::entity_nbt::entity_to_fields(&state.registry.items, entity).unwrap(),
    );
    let mut replay = Replay::default();
    crate::dimensions::recovery::hydrate_entity(&state.registry, &mut replay, &payload, state.tick)
        .unwrap();
    state.remove_entity(slot);
    assert!(state.entities.warden_ids().is_empty());
    crate::core::restore_entity(&mut state, original.0, &replay.entities[&original.0]).unwrap();
    crate::dimensions::validate_identity(&mut state).unwrap();
    assert_eq!(state.entities.warden_ids(), &[original]);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(original).unwrap())
            .uuid,
        uuid
    );
    crate::vibration::emit(
        &mut state,
        pos,
        crate::vibration::GameEvent::Step,
        Some(player),
    );
    let uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    assert_eq!(value(&state, original).anger.get(&uuid), Some(&35));
}
