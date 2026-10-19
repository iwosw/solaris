use super::*;
use crate::villager::VillagerState;
use std::sync::Arc;

fn state() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn settlement(state: &mut TickState, name: &str) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            name.into(),
            "owner".into(),
            [0, 64, 0],
            48,
        )
        .unwrap()
}

fn resident(state: &mut TickState, pos: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).villager = Some(Box::new(VillagerState::default()));
    id
}

#[test]
fn binding_preserves_exact_original_entity_and_has_one_membership_column() {
    let mut state = state();
    let town = settlement(&mut state, "town");
    let id = resident(&mut state, [1.0, 64.0, 1.0]);
    let slot = state.entities.slot_of(id).unwrap();
    let uuid = state.entities.row(slot).uuid;
    let previous = state.entities.row(slot).villager.clone().unwrap();
    bind(&mut state, town, id).unwrap();
    bind(&mut state, town, id).unwrap();
    assert_eq!(state.entities.len(), 1);
    assert_eq!(state.entities.row(slot).uuid, uuid);
    let current = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(current.settlement, Some(town));
    assert!(!crate::villager::persistence::journal_eq(
        &previous, current
    ));
    assert_eq!(member_count(&state, town), 1);
    assert!(
        !serde_json::to_string(state.settlements.get(town).unwrap())
            .unwrap()
            .contains("resident")
    );
    unbind(&mut state, id).unwrap();
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        None
    );
    assert_eq!(member_count(&state, town), 0);
    assert_eq!(state.entities.row(slot).uuid, uuid);
}

#[test]
fn binding_refuses_foreign_owner_membership_dead_trader_zombie_and_far_entities() {
    let mut state = state();
    let first = settlement(&mut state, "first");
    let second = settlement(&mut state, "second");
    let id = resident(&mut state, [0.0, 64.0, 0.0]);
    bind(&mut state, first, id).unwrap();
    assert!(bind(&mut state, second, id).is_err());
    unbind(&mut state, id).unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .wandering = true;
    assert!(bind(&mut state, first, id).is_err());
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .wandering = false;
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .conversion_time = Some(-1);
    assert!(bind(&mut state, first, id).is_err());
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .conversion_time = None;
    state.entities.body_mut(slot).alive = false;
    assert!(bind(&mut state, first, id).is_err());
    let far = resident(&mut state, [1000.0, 64.0, 0.0]);
    assert!(bind(&mut state, first, far).is_err());
    assert!(bind(&mut state, SettlementId(999), far).is_err());
    assert_eq!(member_count(&state, first), 0);
}

#[test]
fn counts_rebuild_from_loaded_columns_and_exclude_dead_residents() {
    let mut state = state();
    let town = settlement(&mut state, "town");
    let living = resident(&mut state, [0.0, 64.0, 0.0]);
    let dead = resident(&mut state, [1.0, 64.0, 0.0]);
    bind(&mut state, town, living).unwrap();
    bind(&mut state, town, dead).unwrap();
    let slot = state.entities.slot_of(dead).unwrap();
    state.entities.body_mut(slot).alive = false;
    let mut restored = Registry::default();
    for image in state.settlements.images() {
        restored.apply_image(image);
    }
    state.settlements = restored;
    assert_eq!(member_count(&state, town), 0);
    rebuild_counts(&mut state);
    assert_eq!(member_count(&state, town), 1);
    validate_memberships(&state).unwrap();
}

#[test]
fn dimension_tail_deletion_clears_foreign_member_without_reusing_identity() {
    let mut root = state();
    let town = settlement(&mut root, "town");
    let mut foreign = state();
    foreign.dimension = crate::dimensions::Dimension::Nether;
    let id = resident(&mut foreign, [0.0, 64.0, 0.0]);
    let slot = foreign.entities.slot_of(id).unwrap();
    foreign
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(town);
    root.other_dimensions
        .insert(foreign.dimension, Box::new(foreign));
    validate_memberships(&root).unwrap();
    let worlds = std::mem::take(&mut root.other_dimensions);
    assert!(remove(&mut root, town));
    root.other_dimensions = worlds;
    finish(&mut root);
    let foreign = &root.other_dimensions[&crate::dimensions::Dimension::Nether];
    assert_eq!(
        foreign
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        None
    );
    validate_memberships(&root).unwrap();
    assert!(settlement(&mut root, "new").0 > town.0);
}

#[test]
fn dangling_membership_is_an_explicit_recovery_error() {
    let mut state = state();
    let id = resident(&mut state, [0.0, 64.0, 0.0]);
    let slot = state.entities.slot_of(id).unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(SettlementId(99));
    let error = validate_memberships(&state).unwrap_err().to_string();
    assert!(error.contains("missing settlement 99"), "{error}");
}

#[test]
fn membership_survives_native_nbt_and_journal_roundtrip_without_a_second_roster() {
    let state = state();
    let original = VillagerState {
        settlement: Some(SettlementId(u64::MAX)),
        ..VillagerState::default()
    };
    let fields =
        crate::villager::persistence::villager_to_fields(&state.registry.items, &original, 0)
            .unwrap();
    let restored =
        crate::villager::persistence::villager_from_fields(&state.registry.items, &fields, 0)
            .unwrap();
    assert_eq!(restored.settlement, original.settlement);
    let image = crate::villager::persistence::journal_image(&original);
    assert_eq!(image.settlement, original.settlement);
    let json = serde_json::to_vec(&image).unwrap();
    let restored: VillagerState = serde_json::from_slice(&json).unwrap();
    assert_eq!(restored.settlement, original.settlement);
    let invalid = VillagerState {
        settlement: Some(SettlementId(0)),
        ..VillagerState::default()
    };
    assert!(crate::villager::persistence::validate_state(&invalid).is_err());
}

#[test]
fn duplicate_record_identity_across_dimensions_is_an_explicit_recovery_error() {
    let mut root = state();
    let id = settlement(&mut root, "overworld");
    let mut foreign = state();
    foreign.dimension = crate::dimensions::Dimension::Nether;
    assert_eq!(settlement(&mut foreign, "nether"), id);
    root.other_dimensions
        .insert(foreign.dimension, Box::new(foreign));
    let error = validate_memberships(&root).unwrap_err().to_string();
    assert!(
        error.contains("duplicate global settlement identity"),
        "{error}"
    );
}

#[test]
fn incremental_change_death_and_removal_match_rebuild() {
    let mut state = state();
    let town = settlement(&mut state, "town");
    let other = settlement(&mut state, "other");
    let keeper = resident(&mut state, [0.0, 64.0, 0.0]);
    let mover = resident(&mut state, [1.0, 64.0, 0.0]);
    let doomed = resident(&mut state, [2.0, 64.0, 0.0]);
    bind(&mut state, town, keeper).unwrap();
    bind(&mut state, town, mover).unwrap();
    bind(&mut state, town, doomed).unwrap();
    assert_eq!(member_count(&state, town), 3);

    // Move one resident to the other settlement behind the counters' back:
    // incremental refresh must drop the old share and add the new one.
    let slot = state.entities.slot_of(mover).unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(other);
    refresh_counts(&mut state, [mover]);
    assert_eq!(member_count(&state, town), 2);
    assert_eq!(member_count(&state, other), 1);
    let mut oracle = state.settlements.member_count(town);
    rebuild_counts(&mut state);
    assert_eq!(state.settlements.member_count(town), oracle);
    assert_eq!(member_count(&state, other), 1);

    // Death removes occupancy the same way a full rebuild does.
    let slot = state.entities.slot_of(doomed).unwrap();
    state.entities.body_mut(slot).alive = false;
    refresh_counts(&mut state, [doomed]);
    assert_eq!(member_count(&state, town), 1);
    oracle = state.settlements.member_count(town);
    rebuild_counts(&mut state);
    assert_eq!(state.settlements.member_count(town), oracle);

    // Native row removal (swap-remove) drops the cached share without a row.
    let slot = state.entities.slot_of(keeper).unwrap();
    state.remove_entity(slot);
    refresh_counts(&mut state, [keeper]);
    assert_eq!(member_count(&state, town), 0);
    oracle = state.settlements.member_count(town);
    rebuild_counts(&mut state);
    assert_eq!(state.settlements.member_count(town), oracle);
}

#[test]
fn radius_grows_with_population_persists_and_never_shrinks() {
    let mut state = state();
    let town = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "growing".into(),
            "owner".into(),
            [0, 64, 0],
            8,
        )
        .unwrap();
    let mut people = Vec::new();
    for _ in 0..5 {
        let id = resident(&mut state, [1.0, 64.0, 1.0]);
        bind(&mut state, town, id).unwrap();
        people.push(id);
    }
    state.settlements.take_images();
    assert_eq!(state.settlements.get(town).unwrap().radius, 20);
    let id = resident(&mut state, [1.0, 64.0, 1.0]);
    bind(&mut state, town, id).unwrap();
    let images = state.settlements.take_images();
    let record = images[0].value.as_ref().expect("grown record is journaled");
    assert_eq!(record.radius, 24);
    assert!(validate_record(record).is_ok());
    unbind(&mut state, people[0]).unwrap();
    assert_eq!(
        state.settlements.get(town).unwrap().radius,
        24,
        "the radius is monotonic"
    );
    for _ in 0..70 {
        let id = resident(&mut state, [1.0, 64.0, 1.0]);
        bind(&mut state, town, id).unwrap();
    }
    assert_eq!(state.settlements.get(town).unwrap().radius, 256);
}
