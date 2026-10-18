use super::*;
use crate::boundary::{BlockPos, ChangeSet, ChunkPos};
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
        [0, 200, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn town(state: &mut TickState, name: &str, anchor: [i32; 3], radius: u32) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            name.into(),
            "owner".into(),
            anchor,
            radius,
        )
        .unwrap()
}

fn resident(state: &mut TickState, settlement: SettlementId, pos: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState::default()));
    crate::settlements::bind(state, settlement, id).unwrap();
    id
}

fn villager(state: &TickState, id: EntityId) -> &crate::villager::VillagerState {
    state
        .entities
        .row(state.entities.slot_of(id).unwrap())
        .villager
        .as_ref()
        .unwrap()
}

#[test]
fn formation_is_atomic_distinct_and_membership_is_only_in_columns() {
    let mut state = state();
    let town = town(&mut state, "first", [8, 201, 8], 48);
    let a = resident(&mut state, town, [8.0, 201.0, 8.0]);
    let b = resident(&mut state, town, [9.0, 201.0, 8.0]);
    state.settlements.take_images();
    assert!(form(&mut state, town, &[a, a], HouseholdOrigin::Adoption).is_err());
    assert!(state.settlements.take_images().is_empty());
    assert!(villager(&state, a).household.is_none());
    let family = form(&mut state, town, &[a, b], HouseholdOrigin::Adoption).unwrap();
    assert_eq!(
        villager(&state, a).household.as_deref(),
        Some(family.as_str())
    );
    assert_eq!(state.settlements.household_count(town, &family), 2);
    assert_eq!(members(&state, town, &family), vec![a, b]);
    let images = state.settlements.take_images();
    assert_eq!(images.len(), 1);
    let json = serde_json::to_value(images[0].value.as_ref().unwrap()).unwrap();
    assert!(json["households"][&family].get("members").is_none());
    assert!(form(&mut state, town, &[a], HouseholdOrigin::Split).is_err());
    detach(&mut state, a);
    assert_eq!(members(&state, town, &family), vec![b]);
    detach(&mut state, b);
    crate::settlements::finish(&mut state);
    assert!(
        !state
            .settlements
            .get(town)
            .unwrap()
            .households
            .contains_key(&family)
    );
    let next = form(&mut state, town, &[a], HouseholdOrigin::Split).unwrap();
    assert_ne!(family, next);
}

#[test]
fn birth_inherits_existing_household_and_keeps_its_individual_bed() {
    let mut state = state();
    let town = town(&mut state, "first", [8, 201, 8], 48);
    let a = resident(&mut state, town, [8.0, 201.0, 8.0]);
    let b = resident(&mut state, town, [9.0, 201.0, 8.0]);
    let family = form(&mut state, town, &[a, b], HouseholdOrigin::Adoption).unwrap();
    let child = resident(&mut state, town, [8.0, 201.0, 8.0]);
    let slot = state.entities.slot_of(child).unwrap();
    let home = BlockPos {
        x: 12,
        y: 201,
        z: 4,
    };
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.settlement = None;
    v.home = Some(home);
    v.age = -24_000;
    inherit_birth(&mut state, child, [a, b]);
    assert_eq!(
        villager(&state, child).household.as_deref(),
        Some(family.as_str())
    );
    assert_eq!(villager(&state, child).settlement, Some(town));
    assert_eq!(villager(&state, child).home, Some(home));
    assert_eq!(state.settlements.household_count(town, &family), 3);
    assert!(crate::settlements::validate_memberships(&state).is_ok());
}

fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
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
        if let Some(entry) = full.iter_mut().find(|(k, _)| k == key) {
            entry.1 = (*value).to_owned();
        }
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &full)
        .unwrap()
        .0
}

#[test]
fn relocation_releases_old_claims_and_reacquires_destination_without_replacing_body() {
    let mut state = state();
    let source = town(&mut state, "source", [4, 201, 8], 5);
    let target = town(&mut state, "target", [12, 201, 8], 5);
    let resident = resident(&mut state, source, [5.5, 201.0, 8.5]);
    let slot = state.entities.slot_of(resident).unwrap();
    let uuid = state.entities.row(slot).uuid;
    let family = form(&mut state, source, &[resident], HouseholdOrigin::Adoption).unwrap();
    let want = rustc_hash::FxHashMap::from_iter([(ChunkPos { x: 0, z: 0 }, 0)]);
    state.world.ensure_blocking(&want);
    state
        .tickets
        .insert("household-relocation".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(0);
    let air = state.registry.states.air;
    let stone = block(&state, "minecraft:stone", &[]);
    for x in 0..16 {
        for z in 0..16 {
            state.world.set_block(x, 200, z, stone);
            for y in 201..205 {
                state.world.set_block(x, y, z, air);
            }
        }
    }
    let head = block(
        &state,
        "minecraft:red_bed",
        &[("part", "head"), ("facing", "east")],
    );
    let foot = block(
        &state,
        "minecraft:red_bed",
        &[("part", "foot"), ("facing", "east")],
    );
    let old = BlockPos { x: 4, y: 201, z: 6 };
    let new = BlockPos {
        x: 12,
        y: 201,
        z: 6,
    };
    for pos in [old, new] {
        state.world.set_block(pos.x - 1, pos.y, pos.z, foot);
        state.world.set_block(pos.x, pos.y, pos.z, head);
    }
    state.world.fold_overlay();
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.home = Some(old);
    v.job_site = Some(BlockPos { x: 3, y: 201, z: 8 });
    v.meeting_point = Some(BlockPos { x: 3, y: 201, z: 9 });
    super::super::allocator::tick(&mut state, &mut ChangeSet::default());
    relocate(&mut state, source, &family, target).unwrap();
    let v = villager(&state, resident);
    assert!(v.home.is_none() && v.job_site.is_none() && v.meeting_point.is_none());
    assert!(
        !state
            .villager_pois
            .homes
            .get(&old)
            .is_some_and(|owners| owners.contains(&resident))
    );
    for tick in 1..=4 {
        state.tick = tick;
        state.tick_budget.reset(tick);
        super::super::allocator::tick(&mut state, &mut ChangeSet::default());
        if villager(&state, resident).home.is_some() {
            break;
        }
    }
    assert_eq!(villager(&state, resident).home, Some(new));
    assert_eq!(
        state.settlements.get(target).unwrap().households[&family].home,
        Some([new.x, new.y, new.z])
    );
    assert_eq!(villager(&state, resident).settlement, Some(target));
    assert_eq!(state.entities.row(slot).uuid, uuid);
    assert!(
        !state
            .settlements
            .get(source)
            .unwrap()
            .households
            .contains_key(&family)
    );
    assert!(
        state
            .settlements
            .get(target)
            .unwrap()
            .households
            .contains_key(&family)
    );
}

#[test]
fn household_checkpoint_roundtrip_rebuilds_members_from_villager_columns() {
    use crate::settlements::persistence::{checkpoint_rows, hydrate_dimension};
    let mut live = state();
    let town = town(&mut live, "saved", [8, 201, 8], 48);
    let person = resident(&mut live, town, [8.0, 201.0, 8.0]);
    let family = form(&mut live, town, &[person], HouseholdOrigin::Refugee).unwrap();
    let saved_person = serde_json::to_vec(villager(&live, person)).unwrap();
    let rows = checkpoint_rows(&live, 8).unwrap();
    let mut recovered = state();
    hydrate_dimension(
        &mut recovered,
        &rows,
        &crate::journal::Replay::default(),
        town.0,
    )
    .unwrap();
    let restored = resident(&mut recovered, town, [8.0, 201.0, 8.0]);
    let slot = recovered.entities.slot_of(restored).unwrap();
    recovered.entities.row_mut(slot).villager =
        Some(serde_json::from_slice(&saved_person).unwrap());
    crate::settlements::persistence::finish_recovery(&mut recovered).unwrap();
    assert_eq!(
        villager(&recovered, restored).household.as_deref(),
        Some(family.as_str())
    );
    assert_eq!(recovered.settlements.household_count(town, &family), 1);
    assert_eq!(recovered.settlements.get(town), live.settlements.get(town));
    assert!(
        recovered.settlements.take_images().is_empty(),
        "hydration is not a mutation"
    );
}

#[test]
fn household_compatibility_preserves_identity_without_authored_member_limits() {
    let mut state = state();
    let home = town(&mut state, "home", [8, 201, 8], 48);
    let other = town(&mut state, "other", [8, 201, 8], 48);
    let people: Vec<_> = (0..20)
        .map(|_| resident(&mut state, home, [8.0, 201.0, 8.0]))
        .collect();
    let stranger = resident(&mut state, other, [8.0, 201.0, 8.0]);
    assert!(!can_breed(&state, [people[0], stranger]));
    assert!(can_breed(&state, [people[0], people[1]]));
    let family = form(&mut state, home, &people, HouseholdOrigin::Adoption).unwrap();
    assert!(can_breed(&state, [people[0], people[1]]));
    detach(&mut state, people[19]);
    assert!(can_breed(&state, [people[0], people[1]]));
    assert!(
        can_breed(&state, [people[0], people[19]]),
        "the native free-bed admission, not household size, decides a birth"
    );
    assert_eq!(state.settlements.household_count(home, &family), 19);
    let separate = form(&mut state, home, &[people[19]], HouseholdOrigin::Split).unwrap();
    assert_ne!(family, separate);
    assert!(!can_breed(&state, [people[0], people[19]]));
}

fn foreign_member(state: &mut TickState, settlement: SettlementId, family: &str) -> EntityId {
    let dimension = crate::dimensions::Dimension::Nether;
    let foreign = state.other_dimensions.entry(dimension).or_insert_with(|| {
        let mut foreign = self::state();
        foreign.dimension = dimension;
        foreign.entities.set_next_wire_id(100);
        Box::new(foreign)
    });
    let kind = foreign
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = foreign.spawn_mob(kind, [8.5, 81.0, 8.5], 20.0);
    let slot = foreign.entities.slot_of(id).unwrap();
    foreign.entities.row_mut(slot).villager = Some(Box::new(crate::villager::VillagerState {
        settlement: Some(settlement),
        household: Some(family.to_owned()),
        home: Some(BlockPos { x: 8, y: 80, z: 6 }),
        job_site: Some(BlockPos { x: 8, y: 80, z: 7 }),
        meeting_point: Some(BlockPos { x: 8, y: 80, z: 9 }),
        home_dimension: foreign.dimension.name().to_owned(),
        job_site_dimension: foreign.dimension.name().to_owned(),
        meeting_point_dimension: foreign.dimension.name().to_owned(),
        ..Default::default()
    }));
    id
}

#[test]
fn foreign_household_members_survive_last_local_unbind_and_retire_at_global_tail() {
    let mut state = state();
    let home = town(&mut state, "home", [8, 201, 8], 48);
    let local = resident(&mut state, home, [8.0, 201.0, 8.0]);
    let family = form(&mut state, home, &[local], HouseholdOrigin::Adoption).unwrap();
    let abroad = foreign_member(&mut state, home, &family);
    // The per-dimension tick temporarily takes sibling states out of its owner.
    let mut nether = state
        .other_dimensions
        .remove(&crate::dimensions::Dimension::Nether)
        .unwrap();
    crate::settlements::unbind(&mut state, local).unwrap();
    state.other_dimensions.insert(nether.dimension, nether);
    crate::settlements::finish(&mut state);
    assert!(
        state
            .settlements
            .get(home)
            .unwrap()
            .households
            .contains_key(&family),
        "a foreign living member keeps the household"
    );
    assert!(crate::settlements::validate_memberships(&state).is_ok());
    nether = state
        .other_dimensions
        .remove(&crate::dimensions::Dimension::Nether)
        .unwrap();
    crate::settlements::unbind(&mut nether, abroad).unwrap();
    state.other_dimensions.insert(nether.dimension, nether);
    crate::settlements::finish(&mut state);
    assert!(
        !state
            .settlements
            .get(home)
            .unwrap()
            .households
            .contains_key(&family)
    );
    assert!(crate::settlements::validate_memberships(&state).is_ok());
}

#[test]
fn foreign_household_members_relocate_with_original_identity_at_global_tail() {
    let mut state = state();
    let source = town(&mut state, "source", [8, 201, 8], 48);
    let target = town(&mut state, "target", [16, 201, 8], 48);
    let local = resident(&mut state, source, [8.0, 201.0, 8.0]);
    let family = form(&mut state, source, &[local], HouseholdOrigin::Adoption).unwrap();
    let abroad = foreign_member(&mut state, source, &family);
    let nether = state
        .other_dimensions
        .remove(&crate::dimensions::Dimension::Nether)
        .unwrap();
    let uuid = nether
        .entities
        .row(nether.entities.slot_of(abroad).unwrap())
        .uuid;
    relocate(&mut state, source, &family, target).unwrap();
    state.other_dimensions.insert(nether.dimension, nether);
    crate::settlements::finish(&mut state);
    let nether = &state.other_dimensions[&crate::dimensions::Dimension::Nether];
    let v = villager(nether, abroad);
    assert_eq!(v.settlement, Some(target));
    assert_eq!(v.household.as_deref(), Some(family.as_str()));
    assert!(v.home.is_none() && v.job_site.is_none() && v.meeting_point.is_none());
    assert_eq!(
        nether
            .entities
            .row(nether.entities.slot_of(abroad).unwrap())
            .uuid,
        uuid
    );
    assert!(crate::settlements::validate_memberships(&state).is_ok());
}

#[test]
fn foreign_household_members_recover_without_artificial_birth_capacity() {
    let mut state = state();
    let home = town(&mut state, "home", [8, 201, 8], 48);
    let a = resident(&mut state, home, [8.0, 201.0, 8.0]);
    let b = resident(&mut state, home, [9.0, 201.0, 8.0]);
    let family = form(&mut state, home, &[a, b], HouseholdOrigin::Adoption).unwrap();
    for _ in 2..20 {
        foreign_member(&mut state, home, &family);
    }
    crate::settlements::persistence::finish_recovery(&mut state).unwrap();
    assert!(
        can_breed(&state, [a, b]),
        "foreign membership is not an authored population limit"
    );
    assert_eq!(state.settlements.household_size(&family), 20);
}
