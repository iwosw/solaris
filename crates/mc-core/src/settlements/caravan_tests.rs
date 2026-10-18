//! Native caravan behavioral tests through [`execute`](super::execute)/[`tick`](super::tick).
//!
//! Real prerequisites only: certified warehouse worksites bound through the
//! generic Survey -> RegisterWorksite operations, real chests with real native
//! items, and one exact living carrier body moved through actual poses. These
//! tests pin commodity conservation across source -> carrier -> destination,
//! stale fences, capacity, death, duplicates, restart receipts and that
//! arrival is never invented.

use super::*;
use crate::boundary::{ChunkPos, EntityId};
use crate::tick::TickState;
use mc_script::{
    ScriptOperation, ScriptOperationFailure as Failure, ScriptOperationPayload,
    ScriptOperationRequest, ScriptSettlementBuildingRef, ScriptWorksiteInteractionPoint,
    ScriptWorksiteRequest, ScriptWorldBounds, ScriptWorldStructureOperation,
    ScriptWorldStructureResult,
};
use std::sync::Arc;

const OWNER: &str = "owner";
const SRC_CHEST: [i32; 3] = [10, 201, 10];
const SRC_DOOR: [i32; 3] = [10, 201, 11];
const DST_CHEST: [i32; 3] = [20, 201, 10];
const DST_DOOR: [i32; 3] = [20, 201, 11];

fn state() -> TickState {
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
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

fn ensure(state: &mut TickState) {
    for (i, (x, z)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
        state
            .tickets
            .insert(format!("caravan-{i}"), (ChunkPos { x, z }, 0));
    }
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    state.refresh_active_chunks(state.tick);
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

fn town(state: &mut TickState, name: &str, anchor: [i32; 3], radius: u32) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            name.into(),
            OWNER.into(),
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

fn uuid_of(state: &TickState, id: EntityId) -> Uuid {
    state.entities.row(state.entities.slot_of(id).unwrap()).uuid
}

fn teleport(state: &mut TickState, id: EntityId, cell: [i32; 3]) {
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(
        slot,
        [
            f64::from(cell[0]) + 0.5,
            f64::from(cell[1]),
            f64::from(cell[2]) + 0.5,
        ],
    );
}

fn stock(state: &mut TickState, at: [i32; 3], item: &str, count: i32) {
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(item).unwrap())
        .unwrap();
    state
        .block_entities
        .get_mut(&(at[0], at[1], at[2]))
        .expect("chest block entity")
        .slots[0] = mc_data::ItemStack::new(item, count);
}

fn chest_total(state: &TickState, at: [i32; 3], item_id: u32) -> i32 {
    state.block_entities[&(at[0], at[1], at[2])]
        .slots
        .iter()
        .filter(|stack| !stack.is_empty() && stack.item_id == item_id)
        .map(|stack| stack.count)
        .sum()
}

fn carry_total(state: &TickState, id: EntityId) -> i32 {
    let slot = state.entities.slot_of_any(id).unwrap();
    state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .inventory
        .iter()
        .flatten()
        .map(|stack| stack.count)
        .sum()
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
        "survey-caravan",
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

struct WorksiteLayout {
    min: [i32; 3],
    max: [i32; 3],
    points: Vec<ScriptWorksiteInteractionPoint>,
}

fn register(
    state: &mut TickState,
    owner: &str,
    operation_id: &str,
    request_id: &str,
    token: &str,
    revision: u64,
    layout: WorksiteLayout,
) -> String {
    let request = ScriptWorksiteRequest::new(
        ScriptWorldBounds::new(layout.min, layout.max).unwrap(),
        layout.points,
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
        WorksiteLayout {
            min: [door[0].min(chest[0]), door[1], door[2].min(chest[2])],
            max: [door[0].max(chest[0]), door[1], door[2].max(chest[2])],
            points: vec![
                entrance(door),
                ScriptWorksiteInteractionPoint::Container { at: chest },
            ],
        },
    )
}

struct Fixture {
    state: TickState,
    settlement: SettlementId,
    source: ContainerRef,
    destination: ContainerRef,
    carrier: EntityId,
    carrier_uuid: Uuid,
    bread: u32,
}

fn fixture() -> Fixture {
    let mut state = state();
    floor(&mut state);
    place_chest(&mut state, SRC_CHEST);
    place_chest(&mut state, DST_CHEST);
    ensure(&mut state);
    let (token, revision) = survey_bounds(&mut state, OWNER, [9, 201, 9], [21, 201, 12]);
    let src = warehouse(
        &mut state, OWNER, &token, revision, "reg-src", SRC_DOOR, SRC_CHEST,
    );
    let dst = warehouse(
        &mut state, OWNER, &token, revision, "reg-dst", DST_DOOR, DST_CHEST,
    );
    let settlement = town(&mut state, "halt", [8, 201, 8], 48);
    let record = state.settlements.get(settlement).unwrap().clone();
    assert_eq!(record.revision, 1);
    super::super::districts::plan(
        &mut state,
        settlement,
        1,
        super::super::districts::DistrictPlan {
            owner: OWNER.into(),
            branch: "none".into(),
            level: 0,
            buildings: vec![
                worksite(&src, "solaris:warehouse"),
                worksite(&dst, "solaris:warehouse"),
            ],
        },
    )
    .unwrap();
    let carrier = resident(&mut state, settlement, [8.5, 201.0, 20.5]);
    let carrier_uuid = uuid_of(&state, carrier);
    let dimension = state.dimension.name().to_owned();
    let bread = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:bread").unwrap())
        .unwrap();
    Fixture {
        state,
        settlement,
        source: ContainerRef {
            building: worksite(&src, "solaris:warehouse"),
            dimension: dimension.clone(),
            position: SRC_CHEST,
        },
        destination: ContainerRef {
            building: worksite(&dst, "solaris:warehouse"),
            dimension,
            position: DST_CHEST,
        },
        carrier,
        carrier_uuid,
        bread,
    }
}

fn manifest(fixture: &Fixture, count: i32) -> ItemStack {
    ItemStack::new(fixture.bread, count)
}

fn open(fixture: &Fixture, id: &str, quantity: u32, paid_mandate: Option<String>) -> Request {
    Request::Open(Box::new(Open {
        id: id.into(),
        source: fixture.source.clone(),
        destination: fixture.destination.clone(),
        carrier: fixture.carrier_uuid,
        manifest: manifest(fixture, quantity as i32),
        quantity,
        waypoints: vec![SRC_DOOR, DST_DOOR],
        max_ticks: 72_000,
        paid_mandate,
    }))
}

fn run(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    request: Request,
) -> bool {
    execute(world, settlement, OWNER, state, request).unwrap()
}

#[test]
fn happy_path_conserves_commodity_via_real_body_poses() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "s1", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    assert!(active(&policy));
    assert_eq!(
        target(&policy, f.carrier_uuid),
        Some(crate::boundary::BlockPos {
            x: SRC_DOOR[0],
            y: SRC_DOOR[1],
            z: SRC_DOOR[2],
        })
    );
    // The body is elsewhere: no invented pickup, nothing moves, nothing reported.
    assert!(!tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.shipments["s1"].status, Status::Loading);
    assert!(!policy.shipments["s1"].loaded);
    // The real body reaches the source stance: autonomous pickup.
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    let shipment = &policy.shipments["s1"];
    assert_eq!(shipment.status, Status::Transit);
    assert!(shipment.loaded);
    assert_eq!(shipment.cursor, 1);
    assert!(shipment.load.is_some());
    assert!(shipment.carrier_live_fence.is_some());
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 0);
    assert_eq!(carry_total(&f.state, f.carrier), 3);
    assert_eq!(
        target(&policy, f.carrier_uuid),
        Some(crate::boundary::BlockPos {
            x: DST_DOOR[0],
            y: DST_DOOR[1],
            z: DST_DOOR[2],
        })
    );
    // Halfway poses advance nothing; only the real waypoint cell counts.
    teleport(&mut f.state, f.carrier, [15, 201, 11]);
    assert!(!tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.shipments["s1"].cursor, 1);
    // The real body reaches the destination stance: autonomous unload.
    teleport(&mut f.state, f.carrier, DST_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    let shipment = &policy.shipments["s1"];
    assert_eq!(shipment.status, Status::Delivered);
    assert!(!shipment.loaded);
    assert!(shipment.delivery.is_some());
    assert!(shipment.carry.is_empty());
    assert!(!active(&policy));
    assert_eq!(target(&policy, f.carrier_uuid), None);
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 0);
    assert_eq!(carry_total(&f.state, f.carrier), 0);
    assert_eq!(chest_total(&f.state, DST_CHEST, f.bread), 3);
    validate(&policy).unwrap();
}

#[test]
fn split_carry_spreads_exact_manifest_across_native_slots() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 50);
    // Slot 0 already holds compatible bread: pickup must top it up and spill
    // the remainder into the next empty native slot, never mint or drop.
    let slot = f.state.entities.slot_of(f.carrier).unwrap();
    f.state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .inventory[0] = Some(manifest(&f, 40));
    let mut policy = State::default();
    let request = open(&f, "split", 50, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    let shipment = &policy.shipments["split"];
    assert_eq!(shipment.status, Status::Transit);
    assert_eq!(
        shipment.carry,
        vec![
            CarryLot { slot: 0, count: 24 },
            CarryLot { slot: 1, count: 26 },
        ]
    );
    assert_eq!(carry_total(&f.state, f.carrier), 90);
    teleport(&mut f.state, f.carrier, DST_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.shipments["split"].status, Status::Delivered);
    assert_eq!(chest_total(&f.state, DST_CHEST, f.bread), 50);
    assert_eq!(carry_total(&f.state, f.carrier), 40);
    validate(&policy).unwrap();
}

#[test]
fn stale_source_fence_blocks_without_loss() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "stale", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    // Unrelated stock movement after admission changes the fence.
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 9);
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    let shipment = &policy.shipments["stale"];
    assert_eq!(shipment.status, Status::Blocked);
    assert!(!shipment.loaded);
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 9);
    assert_eq!(carry_total(&f.state, f.carrier), 0);
}

#[test]
fn tampered_carrier_fence_blocks_unload_without_loss() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "tamper", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert!(policy.shipments["tamper"].loaded);
    // Third-party meddling with the carried stack breaks our fence.
    let slot = f.state.entities.slot_of(f.carrier).unwrap();
    let carried = f.state.entities.row_mut(slot).villager.as_mut().unwrap();
    carried.inventory[0].as_mut().unwrap().count = 1;
    teleport(&mut f.state, f.carrier, DST_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    let shipment = &policy.shipments["tamper"];
    assert_eq!(shipment.status, Status::Blocked);
    assert!(shipment.loaded);
    assert_eq!(carry_total(&f.state, f.carrier), 1);
    assert_eq!(chest_total(&f.state, DST_CHEST, f.bread), 0);
}

#[test]
fn destination_full_blocks_with_cargo_retained() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let stone = f
        .state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    for slot in f
        .state
        .block_entities
        .get_mut(&(DST_CHEST[0], DST_CHEST[1], DST_CHEST[2]))
        .expect("destination chest")
        .slots
        .iter_mut()
    {
        *slot = ItemStack::new(stone, 64);
    }
    let mut policy = State::default();
    let request = open(&f, "full", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert!(policy.shipments["full"].loaded);
    teleport(&mut f.state, f.carrier, DST_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    let shipment = &policy.shipments["full"];
    assert_eq!(shipment.status, Status::Blocked);
    assert!(shipment.loaded);
    assert_eq!(carry_total(&f.state, f.carrier), 3);
}

#[test]
fn duplicate_pickup_and_unload_are_idempotent() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "dup", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    // Identical re-admission is a no-op, conflicting reuse is refused.
    let request = open(&f, "dup", 3, None);
    assert!(!run(&mut f.state, f.settlement, &mut policy, request));
    let conflict = open(&f, "dup", 4, None);
    assert_eq!(
        execute(&mut f.state, f.settlement, OWNER, &mut policy, conflict),
        Err(Failure::OperationConflict)
    );
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    // Explicit replay after the autonomous pickup changes nothing.
    assert!(
        !execute(
            &mut f.state,
            f.settlement,
            OWNER,
            &mut policy,
            Request::Pickup { id: "dup".into() }
        )
        .unwrap()
    );
    teleport(&mut f.state, f.carrier, DST_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.shipments["dup"].status, Status::Delivered);
    // Explicit replay after delivery changes nothing and never double-moves.
    assert!(
        !execute(
            &mut f.state,
            f.settlement,
            OWNER,
            &mut policy,
            Request::Unload { id: "dup".into() }
        )
        .unwrap()
    );
    assert_eq!(chest_total(&f.state, DST_CHEST, f.bread), 3);
}

#[test]
fn block_and_cancel_are_terminal_and_preserve_inventory() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "b1", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::Block {
            id: "b1".into(),
            reason: "bridge gone".into(),
        }
    ));
    assert_eq!(policy.shipments["b1"].status, Status::Blocked);
    assert!(!run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::Block {
            id: "b1".into(),
            reason: "again".into(),
        }
    ));
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 3);

    let other = resident(&mut f.state, f.settlement, [12.5, 201.0, 20.5]);
    let other_uuid = uuid_of(&f.state, other);
    let mut second = open(&f, "c1", 3, None);
    if let Request::Open(open) = &mut second {
        open.carrier = other_uuid;
    }
    assert!(run(&mut f.state, f.settlement, &mut policy, second));
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::Cancel { id: "c1".into() }
    ));
    assert_eq!(policy.shipments["c1"].status, Status::Cancelled);
    assert!(!active(&policy));
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 3);
    validate(&policy).unwrap();
}

#[test]
fn carrier_death_loses_without_refund_or_mint() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "dead", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert!(policy.shipments["dead"].loaded);
    let slot = f.state.entities.slot_of(f.carrier).unwrap();
    f.state.entities.body_mut(slot).alive = false;
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    let shipment = &policy.shipments["dead"];
    assert_eq!(shipment.status, Status::Lost);
    assert!(!delivered(&policy, "dead"));
    // Whatever the native body actually holds stays there: no refund, no mint.
    assert_eq!(carry_total(&f.state, f.carrier), 3);
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 0);
    assert_eq!(chest_total(&f.state, DST_CHEST, f.bread), 0);
}

#[test]
fn death_after_delivery_cannot_rewrite_the_receipt() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "late", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    teleport(&mut f.state, f.carrier, DST_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert!(delivered(&policy, "late"));
    let slot = f.state.entities.slot_of(f.carrier).unwrap();
    f.state.entities.body_mut(slot).alive = false;
    assert!(!tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.shipments["late"].status, Status::Delivered);
    assert!(delivered(&policy, "late"));
    assert_eq!(chest_total(&f.state, DST_CHEST, f.bread), 3);
}

#[test]
fn restart_preserves_receipts_and_no_stock_mirror() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "restart", 3, Some("1".into()));
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    teleport(&mut f.state, f.carrier, DST_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    let text = serde_json::to_string(&policy).unwrap();
    assert!(
        !text.contains("\"slots\""),
        "policy must not mirror native stock"
    );
    let revived: State = serde_json::from_str(&text).unwrap();
    assert_eq!(revived, policy);
    validate(&revived).unwrap();
    assert!(delivered(&revived, "restart"));
    // Replayed delivery evidence still gates exactly once.
    assert_eq!(
        revived.shipments["restart"].load.as_ref().unwrap().op,
        policy.shipments["restart"].load.as_ref().unwrap().op,
        "exact-once receipt identity survives restart"
    );
}

#[test]
fn paid_mandate_gates_on_genuine_delivery_only() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "paid", 3, Some("9".into()));
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    assert_eq!(policy.shipments["paid"].paid_mandate.as_deref(), Some("9"));
    assert!(!delivered(&policy, "paid"));
    // Arrival alone never counts: only the committed unload flips the signal.
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert!(!delivered(&policy, "paid"));
    teleport(&mut f.state, f.carrier, DST_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert!(delivered(&policy, "paid"));
    assert!(!delivered(&policy, "missing"));
}

#[test]
fn route_bounds_reject_bad_geometry() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    for waypoints in [
        vec![SRC_DOOR],
        vec![SRC_DOOR; 17],
        vec![SRC_DOOR, SRC_DOOR],
        vec![SRC_DOOR, [SRC_DOOR[0] + 129, SRC_DOOR[1], SRC_DOOR[2]]],
        vec![DST_DOOR, SRC_DOOR],
        vec![[0, 201, 0], DST_DOOR],
    ] {
        let mut request = open(&f, "bad", 1, None);
        if let Request::Open(open) = &mut request {
            open.waypoints = waypoints;
        }
        let mut policy = State::default();
        assert!(execute(&mut f.state, f.settlement, OWNER, &mut policy, request).is_err());
        assert!(policy.shipments.is_empty());
    }
}

#[test]
fn owner_and_carrier_authorization() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "n1", 3, None);
    assert_eq!(
        execute(&mut f.state, f.settlement, "intruder", &mut policy, request),
        Err(Failure::Forbidden)
    );
    let outsider = stray(&mut f.state, [8.5, 201.0, 20.5]);
    let outsider_uuid = uuid_of(&f.state, outsider);
    let mut foreign = open(&f, "n2", 3, None);
    if let Request::Open(open) = &mut foreign {
        open.carrier = outsider_uuid;
    }
    assert_eq!(
        execute(&mut f.state, f.settlement, OWNER, &mut policy, foreign),
        Err(Failure::Forbidden)
    );
    let request = open(&f, "n3", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    // One living body carries one active shipment.
    let clash = open(&f, "n4", 3, None);
    assert_eq!(
        execute(&mut f.state, f.settlement, OWNER, &mut policy, clash),
        Err(Failure::OperationConflict)
    );
}

#[test]
fn unload_without_load_is_refused() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let request = open(&f, "early", 3, None);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    assert_eq!(
        execute(
            &mut f.state,
            f.settlement,
            OWNER,
            &mut policy,
            Request::Unload { id: "early".into() }
        ),
        Err(Failure::Blocked)
    );
    assert!(!policy.shipments["early"].loaded);
}

#[test]
fn block_reason_is_clamped_and_tick_budget_is_finite() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let mut policy = State::default();
    let mut request = open(&f, "clamp", 3, None);
    if let Request::Open(open) = &mut request {
        open.max_ticks = 2;
    }
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::Block {
            id: "clamp".into(),
            reason: "x".repeat(400),
        }
    ));
    assert!(policy.shipments["clamp"].blocked.as_ref().unwrap().len() <= MAX_BLOCKED_BYTES);

    let mut policy = State::default();
    let mut request = open(&f, "budget", 3, None);
    if let Request::Open(open) = &mut request {
        open.max_ticks = 1;
    }
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    // The body never arrives: idle ticks report nothing until the finite
    // budget blocks instead of travelling forever offscreen.
    f.state.tick += 1;
    assert!(!tick(&mut f.state, f.settlement, &mut policy));
    f.state.tick += 1;
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.shipments["budget"].status, Status::Blocked);
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 3);
}

#[test]
fn state_contracts_hold_policy_and_receipts_only() {
    assert!(validate(&State::default()).is_ok());
    assert!(!active(&State::default()));
    let fence = || ScriptInventoryFence::try_new(1, "a".repeat(64)).unwrap();
    let mut policy = State::default();
    policy.shipments.insert(
        "s".into(),
        Shipment {
            id: "s".into(),
            owner: OWNER.into(),
            settlement: SettlementId(7),
            source: ContainerRef {
                building: worksite("w", "solaris:warehouse"),
                dimension: "minecraft:overworld".into(),
                position: SRC_CHEST,
            },
            destination: ContainerRef {
                building: worksite("w", "solaris:warehouse"),
                dimension: "minecraft:overworld".into(),
                position: DST_CHEST,
            },
            carrier: Uuid::nil(),
            manifest: ItemStack::new(1, 1),
            quantity: 1,
            waypoints: vec![SRC_DOOR, DST_DOOR],
            epoch: 0,
            max_ticks: 10,
            paid_mandate: None,
            source_fence: fence(),
            destination_fence: fence(),
            carrier_fence: fence(),
            status: Status::Loading,
            blocked: None,
            cursor: 0,
            ticks: 0,
            attempt: 0,
            loaded: false,
            carry: Vec::new(),
            carrier_live_fence: None,
            load: None,
            unload: None,
            delivery: None,
        },
    );
    validate(&policy).unwrap();
    assert!(active(&policy));
    assert!(target(&policy, Uuid::nil()).is_some());
    assert!(!delivered(&policy, "s"));
    let mut broken = policy.clone();
    broken.shipments.get_mut("s").unwrap().quantity = 0;
    assert!(validate(&broken).is_err());
    let mut broken = policy.clone();
    broken.shipments.get_mut("s").unwrap().status = Status::Delivered;
    assert!(validate(&broken).is_err());
}

#[test]
fn unloading_preserves_matching_stock_the_carrier_already_owned() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 2);
    let slot = f.state.entities.slot_of(f.carrier).unwrap();
    f.state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .inventory[0] = Some(ItemStack::new(f.bread, 3));
    let mut policy = State::default();
    let request = open(&f, "merged-stock", 2, None);
    run(&mut f.state, f.settlement, &mut policy, request);
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(carry_total(&f.state, f.carrier), 5);
    teleport(&mut f.state, f.carrier, DST_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert!(delivered(&policy, "merged-stock"));
    assert_eq!(carry_total(&f.state, f.carrier), 3);
    assert_eq!(chest_total(&f.state, DST_CHEST, f.bread), 2);
    validate(&policy).unwrap();
}

#[test]
fn exhausted_receipt_sequence_refuses_before_any_native_pickup_write() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 2);
    let mut policy = State::default();
    let request = open(&f, "exhausted", 2, None);
    run(&mut f.state, f.settlement, &mut policy, request);
    policy.shipments.get_mut("exhausted").unwrap().attempt = mc_script::MAX_SCRIPT_WORLD_TIME;
    teleport(&mut f.state, f.carrier, SRC_DOOR);
    let before = policy.clone();
    assert_eq!(
        execute(
            &mut f.state,
            f.settlement,
            OWNER,
            &mut policy,
            Request::Pickup {
                id: "exhausted".into()
            }
        ),
        Err(Failure::Capacity)
    );
    assert_eq!(policy, before);
    assert_eq!(carry_total(&f.state, f.carrier), 0);
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 2);
}

#[test]
fn native_brain_walks_the_real_carrier_and_completes_actual_delivery() {
    use crate::tick_budget::Consumer;
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let request = open(&f, "native-walking", 3, None);
    super::super::economy::execute(
        &mut f.state,
        f.settlement,
        super::super::economy::Request::Caravan(request),
    )
    .unwrap();
    let slot = f.state.entities.slot_of(f.carrier).unwrap();
    let start = f.state.entities.pos(slot);
    for _ in 0..2_000 {
        f.state.tick_budget.reset(f.state.tick);
        let mut changes = crate::boundary::ChangeSet::default();
        crate::settlements::tick(&mut f.state, &mut changes);
        for dimension in crate::dimensions::Dimension::ALL {
            for consumer in [Consumer::Allocator, Consumer::Brain] {
                if dimension != f.state.dimension || consumer != Consumer::Brain {
                    f.state.tick_budget.finish(dimension, consumer);
                }
            }
        }
        crate::villager::brain::tick(&mut f.state, &mut changes);
        f.state.tick += 1;
        if delivered(
            &f.state
                .settlements
                .get(f.settlement)
                .unwrap()
                .economy
                .caravan,
            "native-walking",
        ) {
            break;
        }
    }
    let shipment = &f
        .state
        .settlements
        .get(f.settlement)
        .unwrap()
        .economy
        .caravan
        .shipments["native-walking"];
    assert_eq!(
        shipment.status,
        Status::Delivered,
        "actual body {:?}, policy {:?}",
        f.state.entities.pos(slot),
        shipment
    );
    assert_ne!(f.state.entities.pos(slot), start);
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 0);
    assert_eq!(carry_total(&f.state, f.carrier), 0);
    assert_eq!(chest_total(&f.state, DST_CHEST, f.bread), 3);
}

#[test]
fn production_and_caravan_share_one_real_body_without_conflicting_goals() {
    use super::super::{economy, production};
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 3);
    let station = [12, 201, 10];
    let stand_at = [12, 201, 11];
    let table = block(&f.state, "minecraft:crafting_table", &[]);
    f.state
        .world
        .set_block(station[0], station[1], station[2], table);
    f.state.world.fold_overlay();
    let (token, revision) = survey_bounds(&mut f.state, OWNER, station, stand_at);
    let workshop = register(
        &mut f.state,
        OWNER,
        "reg-economic-shop",
        "reg-economic-shop",
        &token,
        revision,
        WorksiteLayout {
            min: station,
            max: stand_at,
            points: vec![
                entrance(stand_at),
                ScriptWorksiteInteractionPoint::Workstation {
                    at: station,
                    capability: "minecraft:crafting_table".into(),
                },
            ],
        },
    );
    let building = worksite(&workshop, "solaris:workshop");
    let record = f.state.settlements.get(f.settlement).unwrap();
    let revision = record.revision;
    let mut plan = record.district.clone().unwrap();
    plan.buildings.push(building.clone());
    super::super::districts::plan(&mut f.state, f.settlement, revision, plan).unwrap();
    economy::execute(
        &mut f.state,
        f.settlement,
        economy::Request::Production(production::Request::Start {
            job: "body-job".into(),
            worker: f.carrier_uuid,
            recipe: "solaris-settlements:stick".into(),
            count: 1,
            station,
            stand_at,
            building,
        }),
    )
    .unwrap();
    let expected = crate::boundary::BlockPos {
        x: stand_at[0],
        y: stand_at[1],
        z: stand_at[2],
    };
    assert_eq!(
        economy::target(&f.state, Some(f.settlement), f.carrier_uuid),
        Some(expected)
    );
    let images = f.state.settlements.take_images();
    let bytes = serde_json::to_vec(&images).unwrap();
    assert!(
        !String::from_utf8(bytes.clone())
            .unwrap()
            .contains("route_generation")
    );
    for image in serde_json::from_slice::<Vec<super::super::registry::RecordImage>>(&bytes).unwrap()
    {
        f.state.settlements.apply_image(image);
    }
    assert_eq!(
        economy::target(&f.state, Some(f.settlement), f.carrier_uuid),
        Some(expected)
    );
    economy::tick(&mut f.state);
    assert!(
        f.state.settlements.take_images().is_empty(),
        "early idle work must not touch the WAL"
    );
    let before = f.state.settlements.get(f.settlement).unwrap().clone();
    let request = open(&f, "body-route", 3, None);
    assert_eq!(
        economy::execute(
            &mut f.state,
            f.settlement,
            economy::Request::Caravan(request)
        ),
        Err(Failure::Blocked)
    );
    assert_eq!(f.state.settlements.get(f.settlement), Some(&before));
    assert!(f.state.settlements.take_images().is_empty());
    economy::execute(
        &mut f.state,
        f.settlement,
        economy::Request::Production(production::Request::Pause {
            job: "body-job".into(),
        }),
    )
    .unwrap();
    let request = open(&f, "body-route", 3, None);
    economy::execute(
        &mut f.state,
        f.settlement,
        economy::Request::Caravan(request),
    )
    .unwrap();
    assert_eq!(
        economy::execute(
            &mut f.state,
            f.settlement,
            economy::Request::Production(production::Request::Resume {
                job: "body-job".into()
            })
        ),
        Err(Failure::Blocked)
    );
    economy::execute(
        &mut f.state,
        f.settlement,
        economy::Request::Caravan(Request::Cancel {
            id: "body-route".into(),
        }),
    )
    .unwrap();
    economy::execute(
        &mut f.state,
        f.settlement,
        economy::Request::Production(production::Request::Resume {
            job: "body-job".into(),
        }),
    )
    .unwrap();
    assert_eq!(
        economy::target(&f.state, Some(f.settlement), f.carrier_uuid),
        Some(expected)
    );
}

#[test]
fn finite_tick_budget_rotates_past_sixteen_idle_contracts() {
    let mut f = fixture();
    stock(&mut f.state, SRC_CHEST, "minecraft:bread", 17);
    let mut policy = State::default();
    for index in 0..17 {
        let carrier = resident(&mut f.state, f.settlement, [8.5, 201.0, 20.5]);
        let uuid = uuid_of(&f.state, carrier);
        let mut request = open(&f, &format!("rotated-{index:02}"), 1, None);
        if let Request::Open(open) = &mut request {
            open.carrier = uuid;
        }
        run(&mut f.state, f.settlement, &mut policy, request);
    }
    let before = policy.clone();
    assert!(!tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy, before);
    let last = policy.shipments["rotated-16"].carrier;
    let last_slot = f.state.entities.slot_of_uuid(last).unwrap();
    let last_entity = f.state.entities.body(last_slot).id;
    teleport(&mut f.state, last_entity, SRC_DOOR);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.shipments["rotated-16"].status, Status::Transit);
    assert_eq!(carry_total(&f.state, last_entity), 1);
    assert_eq!(chest_total(&f.state, SRC_CHEST, f.bread), 16);
}
