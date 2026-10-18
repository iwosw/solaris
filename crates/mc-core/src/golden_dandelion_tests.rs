use super::*;
use crate::boundary::{ConnectionId, EntityEvent, PlayerInput};
use crate::crafter::tests::{fixture, stack};
use crate::tick::run_tick;
use std::collections::BTreeMap;
fn spawn(state: &mut TickState, name: &str) -> EntityId {
    let id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    let health = state.registry.blueprint(id).unwrap().params.max_health;
    state.spawn_mob(id, [8.5, 201.0, 9.5], health)
}
fn use_on(target: EntityId) -> PlayerInput {
    PlayerInput::InteractEntity {
        connection: ConnectionId(1),
        target,
        hand: Hand::Main,
        location: [0.0; 3],
    }
}
#[test]
fn flower_locks_baby_consumes_one_and_unlocks_after_forty_ticks() {
    let (mut state, actor) = fixture(None);
    let cow = spawn(&mut state, "cow");
    crate::entity_life::make_child(&mut state, cow);
    let flower = stack(&state, "golden_dandelion", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = flower;
    let (output, _, _) = run_tick(&mut state, &[use_on(cow)]);
    let slot = state.entities.slot_of(cow).unwrap();
    assert!(state.entities.row(slot).entity_life.age_lock.locked);
    assert!(state.entities.row(slot).persistent);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
    assert!(
        output
            .changes
            .entity_events
            .contains(&EntityEvent::GoldenDandelion {
                entity: cow,
                locked: true
            })
    );
    let cooldown = state.entities.row(slot).entity_life.age_lock.cooldown_until;
    let age = state.entities.row(slot).entity_life.child_ticks;
    run_tick(&mut state, &[use_on(cow)]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
    while state.tick < cooldown {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.entities.row(slot).entity_life.child_ticks, age);
    assert!(state.entities.row(slot).appearance.baby);
    let (output, _, _) = run_tick(&mut state, &[use_on(cow)]);
    let row = state.entities.row(slot);
    assert!(!row.entity_life.age_lock.locked);
    assert!(
        row.appearance
            .extra
            .contains(&(17, EntityDataValue::Boolean(false)))
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(
        output
            .changes
            .entity_events
            .contains(&EntityEvent::GoldenDandelion {
                entity: cow,
                locked: false
            })
    );
    run_tick(&mut state, &[]);
    assert!(state.entities.row(slot).entity_life.child_ticks < crate::entity_life::CHILD_TICKS);
}
#[test]
fn flower_rejects_adults_and_excluded_baby_types() {
    let (mut state, actor) = fixture(None);
    let flower = stack(&state, "golden_dandelion", 4);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = flower;
    for name in [
        "cow",
        "villager",
        "skeleton_horse",
        "zombie_horse",
        "zombie",
    ] {
        let mob = spawn(&mut state, name);
        if name != "cow" {
            crate::entity_life::make_child(&mut state, mob);
        }
        assert!(!interact(&mut state, actor, mob, Hand::Main));
    }
    assert_eq!(state.players[&actor].inventory.slots[0].count, 4);
}
#[test]
fn tadpole_lock_resets_nonnegative_age_and_sniffer_uses_its_own_start_age() {
    let (mut state, actor) = fixture(None);
    let flowers = stack(&state, "golden_dandelion", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = flowers;
    let tadpole = spawn(&mut state, "tadpole");
    let slot = state.entities.slot_of(tadpole).unwrap();
    state
        .entities
        .row_mut(slot)
        .entity_life
        .age_lock
        .tadpole_age = 123;
    assert!(interact(&mut state, actor, tadpole, Hand::Main));
    crate::entity_life::advance(&mut state);
    assert_eq!(state.entities.row(slot).entity_life.age_lock.tadpole_age, 0);
    assert!(state.entities.row(slot).entity_life.age_lock.locked);
    let native = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:tadpole".into(),
        age: 77,
        extra_fields: crate::entity_life::to_fields(&state.entities.row(slot).entity_life, 0),
        ..Default::default()
    };
    let restored = crate::journal::EntityStateImage::from_record(&state.registry, &native).unwrap();
    assert_eq!(restored.entity_life.age_lock.tadpole_age, 77);
    assert!(restored.entity_life.age_lock.locked);
    let sniffer = spawn(&mut state, "sniffer");
    crate::entity_life::make_child(&mut state, sniffer);
    assert!(interact(&mut state, actor, sniffer, Hand::Main));
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(sniffer).unwrap())
            .entity_life
            .child_ticks,
        48_000
    );
}

#[test]
fn golden_lock_native_entity_checkpoint_and_wal_image_restore_exactly() {
    use crate::journal::{EntityStateImage, JournalEntry, Replay, replay_batches};
    let (mut state, actor) = fixture(None);
    let cow = spawn(&mut state, "cow");
    crate::entity_life::make_child(&mut state, cow);
    let flower = stack(&state, "golden_dandelion", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = flower;
    let (output, _, _) = run_tick(&mut state, &[use_on(cow)]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let slot = state.entities.slot_of(cow).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let image = EntityStateImage::from_row(body, row);
    let native = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:cow".into(),
        age: -(row.entity_life.child_ticks as i32),
        extra_fields: crate::entity_life::to_fields(&row.entity_life, 0),
        ..Default::default()
    };
    let dir = std::env::temp_dir().join(format!("solaris-golden-{}", uuid::Uuid::new_v4()));
    mc_world::anvil::entity_regions::write_entity_regions(
        &dir,
        &state.registry.items,
        &BTreeMap::from([(
            (0, 0),
            mc_world::anvil::EntityChunk {
                x: 0,
                z: 0,
                entities: vec![native],
            },
        )]),
        state.tick as u32,
    )
    .unwrap();
    let chunks =
        mc_world::anvil::entity_regions::read_entity_regions(&dir, &state.registry.items).unwrap();
    let restored =
        EntityStateImage::from_record(&state.registry, &chunks[&(0, 0)].entities[0]).unwrap();
    assert_eq!(restored.entity_life.age_lock, image.entity_life.age_lock);
    assert_eq!(
        restored.entity_life.child_ticks,
        image.entity_life.child_ticks
    );
    assert!(entries.iter().any(|entry| matches!(
        entry,
        JournalEntry::EntityState { .. } | JournalEntry::Entity { .. }
    )));
    let batches = vec![(state.tick, state.tick, entries)];
    let once = replay_batches(Replay::default(), &batches);
    let twice = replay_batches(replay_batches(Replay::default(), &batches), &batches);
    assert_eq!(
        once.entities[&cow.0].state.entity_life.age_lock,
        twice.entities[&cow.0].state.entity_life.age_lock
    );
    assert!(twice.entities[&cow.0].state.entity_life.age_lock.locked);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn corrupt_age_lock_records_have_exact_diagnostics() {
    assert_eq!(
        from_fields(&[("AgeLocked".into(), Tag::Int(1))])
            .unwrap_err()
            .to_string(),
        "storage: AgeLocked: expected boolean byte"
    );
    assert!(
        from_fields(&[
            ("AgeLocked".into(), Tag::Byte(1)),
            ("AgeLocked".into(), Tag::Byte(0))
        ])
        .unwrap_err()
        .to_string()
        .contains("AgeLocked: duplicate field")
    );
    assert!(
        from_fields(&[("solaris:AgeLockCooldownUntil".into(), Tag::Long(-1))])
            .unwrap_err()
            .to_string()
            .contains("solaris:AgeLockCooldownUntil: expected nonnegative long")
    );
}
