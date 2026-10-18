use super::*;
use crate::boundary::BlockPos;
use crate::settlements::allocator::tests::{bed, block, test_state, villager};

#[test]
fn names_are_uuid_stable_and_use_all_uuid_bytes() {
    assert_eq!(villager_name(uuid::Uuid::nil()), "Иван");
    for value in 0..256_u128 {
        let id = uuid::Uuid::from_u128(value);
        assert_eq!(villager_name(id), villager_name(id));
        assert!(!villager_name(id).is_empty());
    }
    assert_ne!(
        villager_name(uuid::Uuid::nil()),
        villager_name(uuid::Uuid::from_u128(1 << 120))
    );
}

#[test]
fn authenticated_owner_name_is_cached_round_trips_and_does_not_dirty_idle_ticks() {
    use crate::boundary::{ConnectionId, EntityId};
    let mut state = test_state();
    let owner = uuid::Uuid::from_u128(42);
    let id = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "Oak".into(),
            owner.to_string(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    state
        .connections
        .insert(ConnectionId(7), EntityId(700), owner, "Alex".into());
    crate::settlements::refresh_owner_names(&mut state);
    let record = state.settlements.get(id).unwrap();
    assert_eq!(
        record.owner,
        owner.to_string(),
        "name must never become authorization identity"
    );
    assert_eq!(record.owner_name.as_deref(), Some("Alex"));
    assert!(summary(&state, record).contains("глава: Alex"));
    let encoded = serde_json::to_value(record).unwrap();
    let restored: SettlementRecord = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(restored.owner_name.as_deref(), Some("Alex"));
    let mut old_save = encoded;
    old_save.as_object_mut().unwrap().remove("owner_name");
    assert!(
        serde_json::from_value::<SettlementRecord>(old_save)
            .unwrap()
            .owner_name
            .is_none()
    );
    state.settlements.take_images();
    crate::settlements::refresh_owner_names(&mut state);
    assert!(state.settlements.take_images().is_empty());
    state
        .connections
        .insert(ConnectionId(7), EntityId(700), owner, "AlexNew".into());
    crate::settlements::refresh_owner_names(&mut state);
    assert_eq!(state.settlements.take_images().len(), 1);
    state.connections.remove_by_connection(ConnectionId(7));
    assert!(summary(&state, state.settlements.get(id).unwrap()).contains("глава: AlexNew"));
}

#[test]
fn summary_counts_living_members_and_actual_jobs() {
    let mut state = test_state();
    let id = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "Oak".into(),
            "Alex".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    let farmer = villager(&mut state, [8.5, 201.0, 8.5]);
    let idle = villager(&mut state, [9.5, 201.0, 8.5]);
    let farmer_id = state_id(&state, farmer);
    let idle_id = state_id(&state, idle);
    crate::settlements::bind(&mut state, id, farmer_id).unwrap();
    crate::settlements::bind(&mut state, id, idle_id).unwrap();
    let v = state.entities.row_mut(farmer).villager.as_mut().unwrap();
    v.data.profession = Profession::Farmer;
    v.job_site = Some(BlockPos { x: 4, y: 201, z: 4 });
    let record = state.settlements.get(id).unwrap();
    assert_eq!(
        summary(&state, record),
        "«Oak» · глава: Alex · жителей 2 · работают 1 · безработных 1"
    );
    state.entities.body_mut(idle).alive = false;
    assert!(
        summary(&state, state.settlements.get(id).unwrap())
            .ends_with("жителей 1 · работают 1 · безработных 0")
    );
}

fn state_id(state: &TickState, slot: crate::entities::EntitySlot) -> crate::boundary::EntityId {
    state.entities.body(slot).id
}

#[test]
fn home_job_and_bell_labels_share_entrance_and_snapshot_is_reused() {
    let mut state = test_state();
    let first = bed(&mut state, 8, 8);
    bed(&mut state, 8, 10);
    let door = block(
        &state,
        "minecraft:oak_door",
        &[("half", "lower"), ("facing", "north")],
    );
    state.world.set_block(6, 201, 9, door);
    state
        .world
        .set_block(6, 203, 9, state.registry.states.stone); // lintel
    state
        .world
        .set_block(6, 203, 10, state.registry.states.stone); // eave
    state
        .world
        .set_block(6, 203, 8, state.registry.states.stone);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    let bell = block(&state, "minecraft:bell", &[]);
    state.world.set_block(12, 201, 12, bell);
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.home = Some(first);
    v.job_site = Some(BlockPos { x: 4, y: 201, z: 4 });
    v.data.profession = Profession::Farmer;
    let mut cache = std::mem::take(&mut state.villager_pois);
    cache.refresh_poi_revisions(&state);
    // The canonical bounded POI refresh is driven by the allocator tick.
    state.villager_pois = cache;
    crate::settlements::allocator::tick(&mut state, &mut Default::default());
    let labels = snapshot(&mut state);
    let home = labels
        .iter()
        .find(|label| label.text.contains("Дом"))
        .unwrap();
    assert_eq!(home.anchor, [6, 201, 9]);
    assert_eq!(
        home.position,
        [6.5, 203.6, 11.25],
        "entrance label must sit outside the door plane, away from the home"
    );
    assert_eq!(
        state.world.block_at(
            home.position[0].floor() as i32,
            home.position[1].floor() as i32,
            home.position[2].floor() as i32
        ),
        Some(state.registry.states.air),
        "label must not be embedded in the opaque lintel/eave"
    );
    assert_eq!(
        label_position(&state, [6, 201, 9], [8, 201, 10]),
        [6.5, 203.6, 7.75],
        "the opposite exterior also clears a one-block eave"
    );
    assert!(home.text.contains("Дом · жильцов 1/2"));
    assert!(home.text.contains("Ферма · фермер"));
    assert!(
        labels
            .iter()
            .any(|label| label.text == "Деревня (без хозяина) · /settlement create <name>")
    );
    let entity_count = state.entities.len();
    let again = snapshot(&mut state);
    assert!(Arc::ptr_eq(&labels, &again));
    state.tick += 1;
    state.entities.set_pos(slot, [9.5, 201.0, 8.5]);
    assert!(
        Arc::ptr_eq(&labels, &snapshot(&mut state)),
        "ordinary movement must not rebuild labels"
    );
    assert_eq!(
        state.entities.len(),
        entity_count,
        "labels must never become gameplay entities"
    );
    state.entities.row_mut(slot).villager.as_mut().unwrap().home = None;
    let changed = snapshot(&mut state);
    assert!(!Arc::ptr_eq(&labels, &changed));
    assert!(
        changed
            .iter()
            .any(|label| label.text.contains("Дом · жильцов 0/2"))
    );
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "Oak".into(),
            "Alex".into(),
            [12, 201, 12],
            48,
        )
        .unwrap();
    let resident = state_id(&state, slot);
    crate::settlements::bind(&mut state, settlement, resident).unwrap();
    let expected = summary(&state, state.settlements.get(settlement).unwrap());
    assert!(
        snapshot(&mut state)
            .iter()
            .any(|label| label.anchor == [12, 201, 12] && label.text == expected)
    );
    let before_door_removal = snapshot(&mut state);
    state.world.set_block(6, 201, 9, state.registry.states.air);
    state.world.fold_overlay();
    let after_door_removal = snapshot(&mut state);
    assert!(!Arc::ptr_eq(&before_door_removal, &after_door_removal));
    assert!(
        after_door_removal
            .iter()
            .all(|label| label.anchor != [6, 201, 9])
    );
}
