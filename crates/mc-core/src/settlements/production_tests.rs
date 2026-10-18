use super::*;
use crate::boundary::EntityId;
use mc_script::{
    ScriptOperation, ScriptOperationPayload, ScriptOperationRequest, ScriptSettlementBuildingRef,
    ScriptWorksiteInteractionPoint, ScriptWorksiteRequest, ScriptWorldBounds,
    ScriptWorldStructureOperation, ScriptWorldStructureResult,
};
use std::sync::Arc;

fn state() -> TickState {
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    // S1 native settlement startup: the 56 native work recipes are world
    // policy, materialized before admission like `Core::open` does.
    crate::plugin_startup::apply(
        &mut registry,
        &mc_plugin_host::DeploymentContribution::default(),
        None,
        &[],
    )
    .unwrap();
    let registry = Arc::new(registry);
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
    let id = Identifier::parse(name).unwrap();
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
        state.tickets.insert(
            format!("production-{i}"),
            (crate::boundary::ChunkPos { x, z }, 0),
        );
    }
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    state.refresh_active_chunks(state.tick);
}

fn place_table(state: &mut TickState, at: [i32; 3]) {
    state.world.set_block(
        at[0],
        at[1],
        at[2],
        block(state, "minecraft:crafting_table", &[]),
    );
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

fn resident(state: &mut TickState, settlement: SettlementId, pos: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState::default()));
    crate::settlements::bind(state, settlement, id).unwrap();
    id
}

fn uuid_of(state: &TickState, id: EntityId) -> Uuid {
    state.entities.row(state.entities.slot_of(id).unwrap()).uuid
}

fn survey_bounds(state: &mut TickState, min: [i32; 3], max: [i32; 3]) -> (String, u64) {
    let request = ScriptOperationRequest::try_new(
        "survey-production",
        ScriptOperation::WorldStructure {
            operation: ScriptWorldStructureOperation::Survey {
                dimension: state.dimension.name().into(),
                bounds: ScriptWorldBounds::new(min, max).unwrap(),
            },
        },
    )
    .unwrap();
    let done = crate::plugin::operation(state, "owner", &request);
    assert_eq!(done.failure(), None);
    let ScriptOperationPayload::WorldStructure { result } = done.payload() else {
        panic!("survey payload")
    };
    let ScriptWorldStructureResult::Survey { survey } = result.as_ref() else {
        panic!("survey result")
    };
    (survey.survey_token.clone(), survey.world_revision)
}
fn shop(
    state: &mut TickState,
    token: &str,
    revision: u64,
    name: &str,
    door: [i32; 3],
    table: [i32; 3],
) -> String {
    let request = ScriptWorksiteRequest::new(
        ScriptWorldBounds::new(
            [door[0].min(table[0]), door[1], door[2].min(table[2])],
            [door[0].max(table[0]), door[1], door[2].max(table[2])],
        )
        .unwrap(),
        vec![
            ScriptWorksiteInteractionPoint::Entrance { at: door },
            ScriptWorksiteInteractionPoint::Workstation {
                at: table,
                capability: "minecraft:crafting_table".into(),
            },
        ],
        token.to_owned(),
        revision,
    );
    let request = ScriptOperationRequest::try_new(
        name,
        ScriptOperation::WorldStructure {
            operation: ScriptWorldStructureOperation::RegisterWorksite {
                operation_id: name.into(),
                request: Box::new(request),
            },
        },
    )
    .unwrap();
    let done = crate::plugin::operation(state, "owner", &request);
    assert_eq!(done.failure(), None);
    let ScriptOperationPayload::WorldStructure { result } = done.payload() else {
        panic!("worksite payload")
    };
    let ScriptWorldStructureResult::Worksite { worksite } = result.as_ref() else {
        panic!("worksite result")
    };
    worksite.worksite_id.clone()
}

fn worksite_ref(id: &str) -> ScriptSettlementBuildingRef {
    ScriptSettlementBuildingRef::new("worksite".into(), id.into(), "solaris:workshop".into())
}

fn certify(state: &mut TickState, id: SettlementId, worksite: &str) {
    let revision = state.settlements.get(id).unwrap().revision;
    crate::settlements::districts::plan(
        state,
        id,
        revision,
        crate::settlements::districts::DistrictPlan {
            owner: "owner".into(),
            branch: "none".into(),
            level: 0,
            buildings: vec![worksite_ref(worksite)],
        },
    )
    .unwrap();
}

fn item(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap(),
        count,
    )
}

fn cake_inputs(state: &TickState) -> Vec<ItemStack> {
    vec![
        item(state, "minecraft:milk_bucket", 1),
        item(state, "minecraft:milk_bucket", 1),
        item(state, "minecraft:milk_bucket", 1),
        item(state, "minecraft:sugar", 2),
        item(state, "minecraft:egg", 1),
        item(state, "minecraft:wheat", 3),
        ItemStack::EMPTY,
        ItemStack::EMPTY,
    ]
}

fn give(stacks: Vec<ItemStack>, state: &mut TickState, id: EntityId) {
    let slot = state.entities.slot_of(id).unwrap();
    let villager = state.entities.row_mut(slot).villager.as_mut().unwrap();
    for (cell, stack) in villager.inventory.iter_mut().zip(stacks) {
        *cell = (!stack.is_empty()).then_some(stack);
    }
}

fn carry(state: &TickState, id: EntityId) -> Vec<ItemStack> {
    state
        .entities
        .row(state.entities.slot_of(id).unwrap())
        .villager
        .as_ref()
        .unwrap()
        .inventory
        .iter()
        .map(|stack| stack.clone().unwrap_or(ItemStack::EMPTY))
        .collect()
}

fn stock_total(state: &TickState, id: EntityId, name: &str) -> i32 {
    carry(state, id)
        .iter()
        .filter(|stack| resource(state, stack) == Some(name))
        .map(|stack| stack.count)
        .sum()
}

fn at(state: &mut TickState, id: EntityId, pos: [f64; 3]) {
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, pos);
}

fn start(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    start: Start,
) -> Result<bool, Failure> {
    let Start {
        job,
        worker,
        recipe_name: recipe,
        count,
        station,
        stand_at,
        building,
    } = start;
    execute(
        world,
        settlement,
        "owner",
        state,
        Request::Start {
            job,
            worker,
            recipe,
            count,
            station,
            stand_at,
            building,
        },
    )
}

fn setup() -> (
    TickState,
    SettlementId,
    EntityId,
    Uuid,
    [i32; 3],
    [i32; 3],
    String,
) {
    let mut world = state();
    floor(&mut world);
    let table = [8, 201, 8];
    let door = [6, 201, 8];
    let stand_at = [7, 201, 8];
    place_table(&mut world, table);
    ensure(&mut world);
    let (token, revision) = survey_bounds(
        &mut world,
        [door[0].min(table[0]), door[1], door[2].min(table[2])],
        [door[0].max(table[0]), door[1], door[2].max(table[2])],
    );
    let worksite = shop(&mut world, &token, revision, "reg-shop", door, table);
    let id = town(&mut world, "mill", [8, 201, 8], 48);
    certify(&mut world, id, &worksite);
    let body = resident(&mut world, id, [7.5, 201.0, 8.5]);
    let worker = uuid_of(&world, body);
    (world, id, body, worker, table, stand_at, worksite)
}

#[test]
fn cake_inputs_become_cake_after_reach_and_deadline() {
    let (mut world, id, body, worker, table, stand_at, worksite) = setup();
    give(cake_inputs(&world), &mut world, body);
    let mut policy = State::default();
    assert!(
        start(
            &mut world,
            id,
            &mut policy,
            Start {
                job: "bake-one".into(),
                worker,
                recipe_name: "solaris-settlements:cake".into(),
                count: 1,
                station: table,
                stand_at,
                building: worksite_ref(&worksite),
            },
        )
        .unwrap()
    );
    assert!(active(&policy));
    assert_eq!(
        target(&policy, worker),
        Some(BlockPos {
            x: stand_at[0],
            y: stand_at[1],
            z: stand_at[2]
        })
    );
    // Away from the stand post: no progress, no loss, retry parked.
    at(&mut world, body, [28.5, 201.0, 28.5]);
    world.tick = policy.jobs["bake-one"].next_due;
    assert!(tick(&mut world, id, &mut policy));
    assert_eq!(policy.jobs["bake-one"].done, 0);
    assert_eq!(stock_total(&world, body, "minecraft:milk_bucket"), 3);
    // At the stand post: one causal portion completes.
    at(&mut world, body, [7.5, 201.0, 8.5]);
    world.tick = policy.jobs["bake-one"].next_due;
    assert!(tick(&mut world, id, &mut policy));
    let job = &policy.jobs["bake-one"];
    assert_eq!(job.done, 1);
    assert_eq!(job.receipts.len(), 1);
    assert_eq!(job.receipts[0].sequence, 1);
    assert!(!active(&policy));
    assert_eq!(target(&policy, worker), None);
    assert_eq!(stock_total(&world, body, "minecraft:cake"), 1);
    assert_eq!(stock_total(&world, body, "minecraft:bucket"), 3);
    assert_eq!(stock_total(&world, body, "minecraft:milk_bucket"), 0);
    assert_eq!(stock_total(&world, body, "minecraft:wheat"), 0);
    // Caught-up ticks never fabricate a second portion.
    world.tick += 10_000;
    assert!(!tick(&mut world, id, &mut policy));
    assert_eq!(stock_total(&world, body, "minecraft:cake"), 1);
    assert!(validate(&policy).is_ok());
}

#[test]
fn missing_inputs_station_and_capacity_pause_without_loss() {
    let (mut world, id, body, worker, table, stand_at, worksite) = setup();
    let mut policy = State::default();
    assert!(
        start(
            &mut world,
            id,
            &mut policy,
            Start {
                job: "hungry".into(),
                worker,
                recipe_name: "solaris-settlements:cake".into(),
                count: 1,
                station: table,
                stand_at,
                building: worksite_ref(&worksite),
            },
        )
        .unwrap()
    );
    at(&mut world, body, [7.5, 201.0, 8.5]);
    world.tick = policy.jobs["hungry"].next_due;
    assert!(tick(&mut world, id, &mut policy));
    assert_eq!(
        policy.jobs["hungry"].paused.as_deref(),
        Some("missing input")
    );
    assert_eq!(policy.jobs["hungry"].done, 0);
    // Break the station: pause names the station, inputs untouched.
    give(cake_inputs(&world), &mut world, body);
    world
        .world
        .set_block(table[0], table[1], table[2], world.registry.states.air);
    world.world.fold_overlay();
    world.tick = policy.jobs["hungry"].next_due;
    assert!(tick(&mut world, id, &mut policy));
    assert_eq!(
        policy.jobs["hungry"].paused.as_deref(),
        Some("missing station")
    );
    assert_eq!(stock_total(&world, body, "minecraft:milk_bucket"), 3);
    // A log remains after consumption, so with all eight cells occupied the
    // planks genuinely have nowhere to go. Inputs/tool wear stay atomic.
    place_table(&mut world, table);
    execute(
        &mut world,
        id,
        "owner",
        &mut policy,
        Request::Cancel {
            job: "hungry".into(),
        },
    )
    .unwrap();
    start(
        &mut world,
        id,
        &mut policy,
        Start {
            job: "capacity".into(),
            worker,
            recipe_name: "solaris-settlements:oak_planks".into(),
            count: 1,
            station: table,
            stand_at,
            building: worksite_ref(&worksite),
        },
    )
    .unwrap();
    let mut full = vec![item(&world, "minecraft:cobblestone", 64); 8];
    full[0] = item(&world, "minecraft:oak_log", 2);
    full[1] = item(&world, "minecraft:wooden_axe", 1);
    give(full, &mut world, body);
    world.tick = policy.jobs["capacity"].next_due;
    assert!(tick(&mut world, id, &mut policy));
    assert_eq!(
        policy.jobs["capacity"].paused.as_deref(),
        Some("no storage")
    );
    assert_eq!(policy.jobs["capacity"].done, 0);
    assert_eq!(stock_total(&world, body, "minecraft:oak_log"), 2);
    assert_eq!(carry(&world, body)[1].damage, None);
}

#[test]
fn cancel_and_replay_never_duplicate() {
    let (mut world, id, body, worker, table, stand_at, worksite) = setup();
    give(cake_inputs(&world), &mut world, body);
    let mut policy = State::default();
    let reference = worksite_ref(&worksite);
    assert!(
        start(
            &mut world,
            id,
            &mut policy,
            Start {
                job: "once".into(),
                worker,
                recipe_name: "solaris-settlements:cake".into(),
                count: 1,
                station: table,
                stand_at,
                building: reference.clone(),
            },
        )
        .unwrap()
    );
    // Identical replay before progress is a no-op.
    assert!(
        !start(
            &mut world,
            id,
            &mut policy,
            Start {
                job: "once".into(),
                worker,
                recipe_name: "solaris-settlements:cake".into(),
                count: 1,
                station: table,
                stand_at,
                building: reference.clone(),
            },
        )
        .unwrap()
    );
    // Conflicting reuse of the identity refuses.
    assert_eq!(
        start(
            &mut world,
            id,
            &mut policy,
            Start {
                job: "once".into(),
                worker,
                recipe_name: "solaris-settlements:stick".into(),
                count: 1,
                station: table,
                stand_at,
                building: reference.clone(),
            },
        ),
        Err(Failure::OperationConflict)
    );
    // Manual pause hides routing; resume restores it; cancel ends it.
    assert!(
        execute(
            &mut world,
            id,
            "owner",
            &mut policy,
            Request::Pause { job: "once".into() }
        )
        .unwrap()
    );
    assert!(!active(&policy));
    assert_eq!(target(&policy, worker), None);
    assert!(
        execute(
            &mut world,
            id,
            "owner",
            &mut policy,
            Request::Resume { job: "once".into() }
        )
        .unwrap()
    );
    assert!(active(&policy));
    assert!(
        execute(
            &mut world,
            id,
            "owner",
            &mut policy,
            Request::Cancel { job: "once".into() }
        )
        .unwrap()
    );
    assert_eq!(policy.cancelled, 1);
    assert!(!active(&policy));
    assert!(!tick(&mut world, id, &mut policy));
    assert_eq!(stock_total(&world, body, "minecraft:cake"), 0);
    assert_eq!(stock_total(&world, body, "minecraft:milk_bucket"), 3);
    // Replay after cancel is a terminal no-op, never a second job.
    assert!(
        !execute(
            &mut world,
            id,
            "owner",
            &mut policy,
            Request::Cancel { job: "once".into() }
        )
        .unwrap()
    );
}

#[test]
fn component_tool_and_remainder_paths_use_native_identity() {
    let (mut world, id, body, worker, table, stand_at, worksite) = setup();
    // Tool path: oak_planks requires a real wooden axe; wear lands on it.
    let mut policy = State::default();
    assert!(
        start(
            &mut world,
            id,
            &mut policy,
            Start {
                job: "planks".into(),
                worker,
                recipe_name: "solaris-settlements:oak_planks".into(),
                count: 1,
                station: table,
                stand_at,
                building: worksite_ref(&worksite),
            },
        )
        .unwrap()
    );
    at(&mut world, body, [7.5, 201.0, 8.5]);
    world.tick = policy.jobs["planks"].next_due;
    assert!(tick(&mut world, id, &mut policy));
    assert_eq!(
        policy.jobs["planks"].paused.as_deref(),
        Some("missing input")
    );
    let axe = item(&world, "minecraft:wooden_axe", 1);
    give(
        vec![
            item(&world, "minecraft:oak_log", 1),
            axe,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
        ],
        &mut world,
        body,
    );
    world.tick = policy.jobs["planks"].next_due;
    assert!(tick(&mut world, id, &mut policy));
    assert_eq!(stock_total(&world, body, "minecraft:oak_planks"), 4);
    assert_eq!(policy.jobs["planks"].receipts[0].tool_wear.len(), 1);
    assert_eq!(policy.jobs["planks"].receipts[0].tool_wear[0].wear, 1);
    // Remainder path: buckets return; replay of the finished plan is a no-op.
    let mut second = State::default();
    give(cake_inputs(&world), &mut world, body);
    assert!(
        start(
            &mut world,
            id,
            &mut second,
            Start {
                job: "cake".into(),
                worker,
                recipe_name: "solaris-settlements:cake".into(),
                count: 1,
                station: table,
                stand_at,
                building: worksite_ref(&worksite),
            },
        )
        .unwrap()
    );
    world.tick = second.jobs["cake"].next_due;
    assert!(tick(&mut world, id, &mut second));
    assert_eq!(stock_total(&world, body, "minecraft:bucket"), 3);
    let outputs: Vec<_> = second.jobs["cake"].receipts[0]
        .outputs
        .iter()
        .map(|stack| resource(&world, stack).unwrap())
        .collect();
    assert!(outputs.contains(&"minecraft:cake"));
    assert!(outputs.contains(&"minecraft:bucket"));
}

#[test]
fn policy_roundtrip_preserves_jobs_and_counters() {
    let (mut world, id, body, worker, table, stand_at, worksite) = setup();
    give(cake_inputs(&world), &mut world, body);
    let mut policy = State::default();
    assert!(
        start(
            &mut world,
            id,
            &mut policy,
            Start {
                job: "round".into(),
                worker,
                recipe_name: "solaris-settlements:cake".into(),
                count: 1,
                station: table,
                stand_at,
                building: worksite_ref(&worksite),
            },
        )
        .unwrap()
    );
    at(&mut world, body, [7.5, 201.0, 8.5]);
    world.tick = policy.jobs["round"].next_due;
    assert!(tick(&mut world, id, &mut policy));
    let bytes = serde_json::to_vec(&policy).unwrap();
    let restored: State = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restored, policy);
    assert!(validate(&restored).is_ok());
    assert_eq!(
        (restored.sequence, restored.completed, restored.cancelled),
        (1, 1, 0)
    );
    assert_eq!(restored.jobs["round"].receipts.len(), 1);
}

#[test]
fn retry_timer_changes_are_reported_but_early_ticks_leave_policy_untouched() {
    let (mut world, id, body, worker, table, stand_at, worksite) = setup();
    let mut policy = State::default();
    start(
        &mut world,
        id,
        &mut policy,
        Start {
            job: "retry".into(),
            worker,
            recipe_name: "solaris-settlements:cake".into(),
            count: 1,
            station: table,
            stand_at,
            building: worksite_ref(&worksite),
        },
    )
    .unwrap();
    at(&mut world, body, [7.5, 201.0, 8.5]);
    world.tick = policy.jobs["retry"].next_due;
    assert!(tick(&mut world, id, &mut policy));
    assert_eq!(
        policy.jobs["retry"].paused.as_deref(),
        Some("missing input")
    );
    let parked = policy.clone();
    world.tick = policy.jobs["retry"].next_due - 1;
    assert!(!tick(&mut world, id, &mut policy));
    assert_eq!(policy, parked);
    world.tick += 1;
    assert!(
        tick(&mut world, id, &mut policy),
        "the next retry time must reach the WAL"
    );
    assert!(policy.jobs["retry"].next_due > world.tick);
    assert_eq!(policy.jobs["retry"].done, 0);
    assert!(policy.jobs["retry"].receipts.is_empty());
}

#[test]
fn due_portion_budget_rotates_instead_of_starving_the_seventeenth_worker() {
    let (mut world, id, _, _, table, stand_at, worksite) = setup();
    let mut policy = State::default();
    for index in 0..17 {
        let body = resident(&mut world, id, [7.5, 201.0, 8.5]);
        let worker = uuid_of(&world, body);
        give(
            vec![
                item(&world, "minecraft:oak_planks", 64),
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
            ],
            &mut world,
            body,
        );
        start(
            &mut world,
            id,
            &mut policy,
            Start {
                job: format!("rotated-{index:02}"),
                worker,
                recipe_name: "solaris-settlements:stick".into(),
                count: 20,
                station: table,
                stand_at,
                building: worksite_ref(&worksite),
            },
        )
        .unwrap();
    }
    world.tick = policy.jobs.values().map(|job| job.next_due).max().unwrap();
    assert!(tick(&mut world, id, &mut policy));
    assert_eq!(policy.jobs.values().filter(|job| job.done == 1).count(), 16);
    assert_eq!(policy.jobs["rotated-16"].done, 0);
    world.tick += policy.jobs["rotated-00"].duration;
    assert!(tick(&mut world, id, &mut policy));
    assert_eq!(policy.jobs["rotated-16"].done, 1);
}
