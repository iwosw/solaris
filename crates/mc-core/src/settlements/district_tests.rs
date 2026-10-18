//! Native district behavioral tests through [`plan`](super::plan)/[`observe`](super::observe).
//!
//! Real prerequisites only: native head+foot bed pairs with clear air,
//! certified-container food in real chests, and owner-scoped worksites bound
//! through the generic Survey -> RegisterWorksite operations. Readiness is a
//! live observation; these tests pin that destruction loses it instead of
//! leaving a stale Ready behind.

use super::{DistrictPlan, observe, plan};
use crate::boundary::{BlockPos, ChunkPos, EntityId};
use crate::settlements::{RecordImage, SettlementId};
use crate::tick::TickState;
use mc_script::{
    ScriptOperation, ScriptOperationFailure as Failure, ScriptOperationPayload,
    ScriptOperationRequest, ScriptSettlementBuildingRef, ScriptSettlementDistrictSnapshot,
    ScriptWorksiteInteractionPoint, ScriptWorksiteRequest, ScriptWorldBounds,
    ScriptWorldStructureOperation, ScriptWorldStructureResult,
};
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

/// Flat stone floor with clear air above; beds, chests and furnaces sit on it.
/// Chunks load first: every helper below writes blocks into loaded chunks.
fn floor(state: &mut TickState) {
    ensure(state);
    let stone = block(state, "minecraft:stone", &[]);
    let air = state.registry.states.air;
    for x in 0..32 {
        for z in 0..32 {
            state.world.set_block(x, 200, z, stone);
            for y in 201..=204 {
                state.world.set_block(x, y, z, air);
            }
        }
    }
    state.world.fold_overlay();
}

fn ensure(state: &mut TickState) {
    for (i, (x, z)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
        state
            .tickets
            .insert(format!("district-{i}"), (ChunkPos { x, z }, 0));
    }
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    state.refresh_active_chunks(state.tick);
}

/// Real native head+foot pairs (facing east), returning the head positions
/// that villagers claim as homes.
fn bed_grid(state: &mut TickState, count: usize) -> Vec<BlockPos> {
    let mut heads = Vec::with_capacity(count);
    for i in 0..count {
        let head_x = 2 + (i % 8) as i32 * 3;
        let z = 2 + (i / 8) as i32 * 2;
        let foot = block(
            state,
            "minecraft:red_bed",
            &[("part", "foot"), ("facing", "east")],
        );
        let head = block(
            state,
            "minecraft:red_bed",
            &[("part", "head"), ("facing", "east")],
        );
        state.world.set_block(head_x - 1, 201, z, foot);
        state.world.set_block(head_x, 201, z, head);
        heads.push(BlockPos {
            x: head_x,
            y: 201,
            z,
        });
    }
    state.world.fold_overlay();
    heads
}

fn place_chest(state: &mut TickState, at: [i32; 3]) {
    let id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let default = state.registry.blocks.block(&id).unwrap().default;
    let mut properties = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (name, value) in &mut properties {
        if name == "type" {
            *value = "single".to_owned();
        }
        if name == "facing" {
            *value = "north".to_owned();
        }
    }
    let state_id = state
        .registry
        .blocks
        .by_name_and_props(&id, &properties)
        .unwrap();
    state.world.set_block(at[0], at[1], at[2], state_id.0);
    state.block_entities.insert(
        (at[0], at[1], at[2]),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    state.world.fold_overlay();
}

fn place_furnace(state: &mut TickState, at: [i32; 3]) {
    let furnace = block(state, "minecraft:furnace", &[]);
    state.world.set_block(at[0], at[1], at[2], furnace);
    state.world.fold_overlay();
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

fn stray(state: &mut TickState, pos: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState::default()));
    id
}

fn resident(state: &mut TickState, settlement: SettlementId, pos: [f64; 3]) -> EntityId {
    let id = stray(state, pos);
    crate::settlements::bind(state, settlement, id).unwrap();
    id
}

/// Spawn positions stay clear of the bed grid so bodies never block baby space.
fn spawn_all(state: &mut TickState, id: SettlementId, count: usize) -> Vec<EntityId> {
    (0..count)
        .map(|i| {
            resident(
                state,
                id,
                [(i % 32) as f64 + 0.5, 201.0, 28.0 + (i / 32) as f64 + 0.5],
            )
        })
        .collect()
}

fn home(state: &mut TickState, id: EntityId, at: BlockPos) {
    let dimension = state.dimension.name().to_owned();
    let slot = state.entities.slot_of(id).unwrap();
    let villager = state.entities.row_mut(slot).villager.as_mut().unwrap();
    villager.home = Some(at);
    villager.home_dimension = dimension;
}

fn stock(state: &mut TickState, at: [i32; 3], item: &str, count: u32) {
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(item).unwrap())
        .unwrap();
    state
        .block_entities
        .get_mut(&(at[0], at[1], at[2]))
        .expect("chest block entity")
        .slots[0] = mc_data::ItemStack::new(item, count as i32);
}

fn entrance(at: [i32; 3]) -> ScriptWorksiteInteractionPoint {
    ScriptWorksiteInteractionPoint::Entrance { at }
}

fn worksite(id: &str, purpose: &str) -> ScriptSettlementBuildingRef {
    ScriptSettlementBuildingRef::new("worksite".into(), id.into(), purpose.into())
}

fn survey_bounds(
    state: &mut TickState,
    owner: &str,
    min: [i32; 3],
    max: [i32; 3],
) -> (String, u64) {
    let request = ScriptOperationRequest::try_new(
        "survey-district",
        ScriptOperation::WorldStructure {
            operation: ScriptWorldStructureOperation::Survey {
                dimension: state.dimension.name().into(),
                bounds: ScriptWorldBounds::new(min, max).unwrap(),
            },
        },
    )
    .unwrap();
    let done = crate::plugin::operation(state, owner, &request);
    assert_eq!(done.failure(), None);
    let ScriptOperationPayload::WorldStructure { result } = done.payload() else {
        panic!("survey payload")
    };
    let ScriptWorldStructureResult::Survey { survey } = result.as_ref() else {
        panic!("survey result")
    };
    (survey.survey_token.clone(), survey.world_revision)
}

#[allow(clippy::too_many_arguments)]
fn register(
    state: &mut TickState,
    owner: &str,
    operation_id: &str,
    request_id: &str,
    token: &str,
    revision: u64,
    min: [i32; 3],
    max: [i32; 3],
    points: Vec<ScriptWorksiteInteractionPoint>,
) -> String {
    let request = ScriptWorksiteRequest::new(
        ScriptWorldBounds::new(min, max).unwrap(),
        points,
        token.to_owned(),
        revision,
    );
    let request = ScriptOperationRequest::try_new(
        request_id,
        ScriptOperation::WorldStructure {
            operation: ScriptWorldStructureOperation::RegisterWorksite {
                operation_id: operation_id.into(),
                request: Box::new(request),
            },
        },
    )
    .unwrap();
    let done = crate::plugin::operation(state, owner, &request);
    assert_eq!(done.failure(), None);
    let ScriptOperationPayload::WorldStructure { result } = done.payload() else {
        panic!("worksite payload")
    };
    let ScriptWorldStructureResult::Worksite { worksite } = result.as_ref() else {
        panic!("worksite result")
    };
    worksite.worksite_id.clone()
}

fn farm(
    state: &mut TickState,
    owner: &str,
    token: &str,
    revision: u64,
    name: &str,
    at: [i32; 3],
) -> String {
    register(
        state,
        owner,
        name,
        name,
        token,
        revision,
        at,
        at,
        vec![entrance(at)],
    )
}

/// Warehouse worksite: standable entrance beside a real chest that later
/// holds the counted food.
fn warehouse(
    state: &mut TickState,
    owner: &str,
    token: &str,
    revision: u64,
    name: &str,
    door: [i32; 3],
    chest: [i32; 3],
) -> String {
    register(
        state,
        owner,
        name,
        name,
        token,
        revision,
        [door[0].min(chest[0]), door[1], door[2].min(chest[2])],
        [door[0].max(chest[0]), door[1], door[2].max(chest[2])],
        vec![
            entrance(door),
            ScriptWorksiteInteractionPoint::Container { at: chest },
        ],
    )
}

#[allow(clippy::too_many_arguments)]
fn smithy(
    state: &mut TickState,
    owner: &str,
    token: &str,
    revision: u64,
    name: &str,
    door: [i32; 3],
    furnace: [i32; 3],
) -> String {
    register(
        state,
        owner,
        name,
        name,
        token,
        revision,
        [door[0].min(furnace[0]), door[1], door[2].min(furnace[2])],
        [door[0].max(furnace[0]), door[1], door[2].max(furnace[2])],
        vec![
            entrance(door),
            ScriptWorksiteInteractionPoint::Workstation {
                at: furnace,
                capability: "minecraft:furnace".into(),
            },
        ],
    )
}

fn do_plan(
    state: &mut TickState,
    id: SettlementId,
    revision: u64,
    owner: &str,
    branch: &str,
    level: u8,
    buildings: Vec<ScriptSettlementBuildingRef>,
) -> Result<(), Failure> {
    plan(
        state,
        id,
        revision,
        DistrictPlan {
            owner: owner.into(),
            branch: branch.into(),
            level,
            buildings,
        },
    )
}

fn observed(state: &mut TickState, id: SettlementId) -> ScriptSettlementDistrictSnapshot {
    let record = state.settlements.get(id).unwrap().clone();
    observe(
        state,
        record.id,
        record.anchor,
        record.radius,
        record.district.as_ref(),
    )
    .expect("planned district observes")
}

fn revision(state: &TickState, id: SettlementId) -> u64 {
    state.settlements.get(id).unwrap().revision
}

#[test]
fn hamlet_ready_with_exact_bed_and_single_bread() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 1);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "hamlet", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "none",
        0,
        vec![
            worksite(&farm, "solaris:farm"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 1);
    home(&mut state, people[0], beds[0]);
    stock(&mut state, chest, "minecraft:bread", 1);
    let snap = observed(&mut state, id);
    snap.validate().unwrap();
    assert_eq!(snap.requested_branch, "none");
    assert_eq!(snap.requested_level, 0);
    assert_eq!(snap.stage.as_deref(), Some("hamlet"));
    assert_eq!(snap.branch, "none");
    assert_eq!(snap.level, 0);
    assert_eq!((snap.population, snap.housed), (1, 1));
    assert_eq!(snap.food, 1);
    assert!(snap.reason.is_none());
    assert_eq!(
        snap.working,
        vec!["solaris:farm".to_owned(), "solaris:warehouse".to_owned()]
    );

    // Acknowledged native chunks may unload; querying must not reload a bed
    // or keep reporting the earlier readiness from a policy/roster cache.
    let written: Vec<_> = state
        .world
        .resident_positions()
        .into_iter()
        .map(|position| (position, state.world.storage_revision(position)))
        .collect();
    state.world.mark_flushed(&written);
    let want = [(ChunkPos { x: 1, z: 0 }, 0)].into_iter().collect();
    state.world.evict_outside(&want);
    assert!(!state.world.has_chunk(0, 0));
    let unloaded = observed(&mut state, id);
    assert_eq!((unloaded.population, unloaded.housed), (1, 0));
    assert!(unloaded.stage.is_none());
    assert_eq!(
        unloaded.reason.as_deref(),
        Some("Actual native housing unavailable or insufficient.")
    );
    assert!(!state.world.has_chunk(0, 0));
}

#[test]
fn unhomed_and_shared_beds_do_not_count_as_housing() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 1);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "shared", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "none",
        0,
        vec![
            worksite(&farm, "solaris:farm"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 2);
    home(&mut state, people[0], beds[0]);
    stock(&mut state, chest, "minecraft:bread", 2);
    let snap = observed(&mut state, id);
    assert_eq!((snap.population, snap.housed), (2, 1));
    assert_eq!(snap.stage, None);
    assert!(snap.reason.as_deref().unwrap().contains("housing"));
    // A second claimant on the same bed poisons it instead of housing both.
    home(&mut state, people[1], beds[0]);
    let snap = observed(&mut state, id);
    assert_eq!((snap.population, snap.housed), (2, 0));
    assert_eq!(snap.stage, None);
    assert!(snap.reason.as_deref().unwrap().contains("housing"));
}

#[test]
fn unbound_villagers_do_not_join_population_or_housing() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 3);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "members", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "none",
        0,
        vec![
            worksite(&farm, "solaris:farm"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 2);
    home(&mut state, people[0], beds[0]);
    home(&mut state, people[1], beds[1]);
    let outsider = stray(&mut state, [20.5, 201.0, 28.5]);
    home(&mut state, outsider, beds[2]);
    stock(&mut state, chest, "minecraft:bread", 2);
    let snap = observed(&mut state, id);
    assert_eq!((snap.population, snap.housed), (2, 2));
    assert_eq!(snap.stage.as_deref(), Some("hamlet"));
    assert!(snap.reason.is_none());
}

#[test]
fn village_threshold_needs_24_exact_housing_and_food() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 24);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "village", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "none",
        0,
        vec![
            worksite(&farm, "solaris:farm"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 24);
    for (person, bed) in people.iter().zip(beds.iter()) {
        home(&mut state, *person, *bed);
    }
    stock(&mut state, chest, "minecraft:bread", 24);
    let snap = observed(&mut state, id);
    snap.validate().unwrap();
    assert_eq!((snap.population, snap.housed), (24, 24));
    assert_eq!(snap.food, 24);
    assert_eq!(snap.stage.as_deref(), Some("village"));
    assert!(snap.reason.is_none());
}

#[test]
fn developed_estate_level2_ready_with_working_institutions() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 64);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let hall = self::farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-hall",
        [28, 201, 2],
    );
    let council = self::farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-council",
        [30, 201, 2],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "estate", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "estate",
        2,
        vec![
            worksite(&farm, "solaris:farm"),
            worksite(&hall, "solaris:manor_hall"),
            worksite(&council, "solaris:town_hall"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 64);
    for (person, bed) in people.iter().zip(beds.iter()) {
        home(&mut state, *person, *bed);
    }
    stock(&mut state, chest, "minecraft:bread", 64);
    let snap = observed(&mut state, id);
    snap.validate().unwrap();
    assert_eq!((snap.population, snap.housed), (64, 64));
    assert_eq!(snap.food, 64);
    assert_eq!(snap.stage.as_deref(), Some("developed"));
    assert_eq!(snap.branch, "estate");
    assert_eq!(snap.level, 2);
    assert!(snap.reason.is_none());
    assert_eq!(
        snap.working,
        vec![
            "solaris:farm".to_owned(),
            "solaris:manor_hall".to_owned(),
            "solaris:town_hall".to_owned(),
            "solaris:warehouse".to_owned(),
        ]
    );
}

#[test]
fn town_level1_missing_smithy_refuses_institution() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 64);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let market = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-market",
        [28, 201, 4],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "townless", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "town",
        1,
        vec![
            worksite(&market, "solaris:market"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 64);
    for (person, bed) in people.iter().zip(beds.iter()) {
        home(&mut state, *person, *bed);
    }
    stock(&mut state, chest, "minecraft:bread", 64);
    let snap = observed(&mut state, id);
    assert_eq!((snap.population, snap.housed), (64, 64));
    assert_eq!(snap.food, 64);
    assert_eq!(snap.requested_branch, "town");
    assert_eq!(snap.stage, None);
    assert_eq!(snap.branch, "none");
    assert!(snap.reason.as_deref().unwrap().contains("institution"));
}

#[test]
fn fortress_branch_refuses_before_developed_population() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 3);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let tower = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-tower",
        [26, 201, 6],
    );
    let wall = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-wall",
        [28, 201, 6],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "fort", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "fortress",
        1,
        vec![
            worksite(&tower, "solaris:stone_tower"),
            worksite(&wall, "solaris:stone_wall"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 3);
    for (person, bed) in people.iter().zip(beds.iter()) {
        home(&mut state, *person, *bed);
    }
    stock(&mut state, chest, "minecraft:bread", 3);
    let snap = observed(&mut state, id);
    assert_eq!((snap.population, snap.housed), (3, 3));
    assert_eq!(snap.food, 3);
    assert_eq!(snap.requested_branch, "fortress");
    assert_eq!(snap.stage, None);
    assert_eq!(snap.branch, "none");
    assert!(snap.reason.as_deref().unwrap().contains("Developed"));
}

#[test]
fn destroyed_entrance_loses_readiness_without_stale_ready() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 1);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "ruin", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "none",
        0,
        vec![
            worksite(&farm, "solaris:farm"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 1);
    home(&mut state, people[0], beds[0]);
    stock(&mut state, chest, "minecraft:bread", 1);
    let ready = observed(&mut state, id);
    assert_eq!(ready.stage.as_deref(), Some("hamlet"));
    assert!(ready.reason.is_none());
    // The entrance cell is filled in: the farm is gone, so is readiness.
    let stone = block(&state, "minecraft:stone", &[]);
    state.world.set_block(26, 201, 2, stone);
    state.world.fold_overlay();
    let snap = observed(&mut state, id);
    assert_eq!(snap.stage, None);
    assert_eq!(snap.branch, "none");
    assert!(
        snap.reason
            .as_deref()
            .unwrap()
            .contains("building unavailable")
    );
}

#[test]
fn destroyed_bed_loses_housing_without_stale_ready() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 1);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "burned", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "none",
        0,
        vec![
            worksite(&farm, "solaris:farm"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 1);
    home(&mut state, people[0], beds[0]);
    stock(&mut state, chest, "minecraft:bread", 1);
    let ready = observed(&mut state, id);
    assert_eq!(ready.stage.as_deref(), Some("hamlet"));
    // The claimed head block is broken: housing drops to zero.
    let air = state.registry.states.air;
    state.world.set_block(beds[0].x, beds[0].y, beds[0].z, air);
    state.world.fold_overlay();
    let snap = observed(&mut state, id);
    assert_eq!((snap.population, snap.housed), (1, 0));
    assert_eq!(snap.stage, None);
    assert!(snap.reason.as_deref().unwrap().contains("housing"));
}

#[test]
fn empty_and_nonfood_chest_counts_no_invented_stock() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 1);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "hungry", [8, 201, 8], 48);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "none",
        0,
        vec![
            worksite(&farm, "solaris:farm"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 1);
    home(&mut state, people[0], beds[0]);
    let snap = observed(&mut state, id);
    assert_eq!(snap.food, 0);
    assert_eq!(snap.stage, None);
    assert!(snap.reason.as_deref().unwrap().contains("food"));
    // Stone is stock, not food: still zero and still not ready.
    stock(&mut state, chest, "minecraft:stone", 10);
    let snap = observed(&mut state, id);
    assert_eq!(snap.food, 0);
    assert_eq!(snap.stage, None);
    assert!(snap.reason.as_deref().unwrap().contains("food"));
}

#[test]
fn plan_and_observe_change_no_entity_identity_or_count() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 2);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "stable", [8, 201, 8], 48);
    let people = spawn_all(&mut state, id, 2);
    home(&mut state, people[0], beds[0]);
    home(&mut state, people[1], beds[1]);
    stock(&mut state, chest, "minecraft:bread", 2);
    let census = |state: &TickState| {
        let mut uuids: Vec<_> = state
            .entities
            .slots()
            .map(|slot| state.entities.row(slot).uuid)
            .collect();
        uuids.sort();
        uuids
    };
    let before = census(&state);
    assert_eq!(before.len(), 2);
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "none",
        0,
        vec![
            worksite(&farm, "solaris:farm"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let first = observed(&mut state, id);
    assert_eq!(first.stage.as_deref(), Some("hamlet"));
    let second = observed(&mut state, id);
    assert_eq!(second.stage.as_deref(), Some("hamlet"));
    assert_eq!(census(&state), before);
}

#[test]
fn cross_owner_outside_and_double_bound_certificates_refused() {
    let mut state = state();
    floor(&mut state);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let near_a = town(&mut state, "alpha", [8, 201, 8], 48);
    let near_b = town(&mut state, "beta", [24, 201, 8], 48);
    let far = town(&mut state, "gamma", [-200, 201, -200], 8);
    state.settlements.take_images();
    let certificate = vec![worksite(&farm, "solaris:farm")];
    do_plan(
        &mut state,
        near_a,
        1,
        "owner",
        "none",
        0,
        certificate.clone(),
    )
    .unwrap();
    assert_eq!(revision(&state, near_a), 2);
    // Same owner cannot bind one certificate to two settlements.
    assert_eq!(
        do_plan(
            &mut state,
            near_b,
            1,
            "owner",
            "none",
            0,
            certificate.clone()
        )
        .err(),
        Some(Failure::InvalidRequest)
    );
    // A footprint outside the settlement radius is not bindable.
    assert_eq!(
        do_plan(&mut state, far, 1, "owner", "none", 0, certificate.clone()).err(),
        Some(Failure::InvalidRequest)
    );
    // Another owner's certificate is forbidden.
    assert_eq!(
        do_plan(
            &mut state,
            near_b,
            1,
            "other",
            "none",
            0,
            certificate.clone()
        )
        .err(),
        Some(Failure::Forbidden)
    );
    // Unknown certificates do not exist.
    assert_eq!(
        do_plan(
            &mut state,
            near_a,
            2,
            "owner",
            "none",
            0,
            vec![worksite("missing-certificate", "solaris:farm")],
        )
        .err(),
        Some(Failure::NotFound)
    );
    assert_eq!(revision(&state, near_a), 2);
    assert_eq!(revision(&state, near_b), 1);
    assert_eq!(revision(&state, far), 1);
    let images = state.settlements.take_images();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].id, near_a);
}

#[test]
fn branch_change_and_stale_revision_fences_hold_record_still() {
    let mut state = state();
    floor(&mut state);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let market = self::farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-market",
        [28, 201, 4],
    );
    let id = town(&mut state, "fenced", [8, 201, 8], 48);
    let single = vec![worksite(&farm, "solaris:farm")];
    let pair = vec![
        worksite(&farm, "solaris:farm"),
        worksite(&market, "solaris:market"),
    ];
    do_plan(&mut state, id, 1, "owner", "none", 0, single.clone()).unwrap();
    assert_eq!(revision(&state, id), 2);
    assert_eq!(
        do_plan(&mut state, id, 1, "owner", "town", 1, single.clone()).err(),
        Some(Failure::StaleRevision)
    );
    assert_eq!(revision(&state, id), 2);
    do_plan(&mut state, id, 2, "owner", "town", 1, single.clone()).unwrap();
    assert_eq!(revision(&state, id), 3);
    // Same branch may re-plan (level and set stay compatible here).
    do_plan(&mut state, id, 3, "owner", "town", 1, pair).unwrap();
    assert_eq!(revision(&state, id), 4);
    // A chosen branch is immutable afterwards, in both directions.
    assert_eq!(
        do_plan(&mut state, id, 4, "owner", "estate", 1, single.clone()).err(),
        Some(Failure::InvalidRequest)
    );
    assert_eq!(
        do_plan(&mut state, id, 4, "owner", "none", 0, single.clone()).err(),
        Some(Failure::InvalidRequest)
    );
    assert_eq!(revision(&state, id), 4);
}

#[test]
fn touched_images_carry_only_plans_and_roundtrip_without_readiness() {
    let mut state = state();
    floor(&mut state);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let farm = farm(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-farm",
        [26, 201, 2],
    );
    let planned = town(&mut state, "imaged", [8, 201, 8], 48);
    let untouched = town(&mut state, "plain", [24, 201, 8], 48);
    state.settlements.take_images();
    let expected = DistrictPlan {
        owner: "owner".into(),
        branch: "none".into(),
        level: 0,
        buildings: vec![worksite(&farm, "solaris:farm")],
    };
    plan(&mut state, planned, 1, expected.clone()).unwrap();
    let images = state.settlements.take_images();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].id, planned);
    assert_eq!(
        images[0].value.as_ref().unwrap().district,
        Some(expected.clone())
    );
    let json = serde_json::to_value(&images[0]).unwrap();
    let roundtrip: RecordImage = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(roundtrip, images[0]);
    let district = &json["value"]["district"];
    assert!(district.get("owner").is_some());
    assert!(district.get("branch").is_some());
    assert!(district.get("level").is_some());
    assert!(district.get("buildings").is_some());
    for absent in [
        "stage",
        "food",
        "capacity",
        "ready",
        "readiness",
        "population",
        "housed",
    ] {
        assert!(
            district.get(absent).is_none(),
            "no persisted {absent} in district image"
        );
    }
    // Observing is a query: it touches no registry image, planned or not.
    let _ = observed(&mut state, planned);
    let _ = state.settlements.get(untouched).unwrap().clone();
    assert!(state.settlements.take_images().is_empty());
}

#[test]
fn smithy_workstation_counts_as_working_institution() {
    let mut state = state();
    floor(&mut state);
    let beds = bed_grid(&mut state, 1);
    let chest = [27, 201, 4];
    place_chest(&mut state, chest);
    let furnace_at = [31, 201, 4];
    place_furnace(&mut state, furnace_at);
    ensure(&mut state);
    let (token, world_revision) = survey_bounds(&mut state, "owner", [26, 201, 2], [31, 201, 6]);
    let forge = smithy(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-forge",
        [30, 201, 4],
        furnace_at,
    );
    let store = warehouse(
        &mut state,
        "owner",
        &token,
        world_revision,
        "reg-store",
        [26, 201, 4],
        chest,
    );
    let id = town(&mut state, "forge", [8, 201, 8], 48);
    // A real furnace behind the workstation point keeps the smithy working;
    // the town still waits on its missing market, so this stays unready.
    do_plan(
        &mut state,
        id,
        1,
        "owner",
        "town",
        1,
        vec![
            worksite(&forge, "solaris:smithy"),
            worksite(&store, "solaris:warehouse"),
        ],
    )
    .unwrap();
    let people = spawn_all(&mut state, id, 1);
    home(&mut state, people[0], beds[0]);
    stock(&mut state, chest, "minecraft:bread", 1);
    // One resident cannot carry a town branch, but the observation must show
    // the smithy as working rather than blaming the building.
    let snap = observed(&mut state, id);
    assert!(snap.working.contains(&"solaris:smithy".to_owned()));
    assert!(snap.working.contains(&"solaris:warehouse".to_owned()));
    assert_eq!(snap.stage, None);
    // Break the furnace: the smithy stops working and the building is blamed.
    let air = state.registry.states.air;
    state
        .world
        .set_block(furnace_at[0], furnace_at[1], furnace_at[2], air);
    state.world.fold_overlay();
    let snap = observed(&mut state, id);
    assert!(!snap.working.contains(&"solaris:smithy".to_owned()));
    assert!(
        snap.reason
            .as_deref()
            .unwrap()
            .contains("building unavailable")
    );
}
