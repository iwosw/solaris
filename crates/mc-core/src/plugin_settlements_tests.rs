//! Real SDK guest consumers for native registry, household, district, and
//! economy customization. Commands travel the actual guest -> host adapter ->
//! core `plugin::operation` dispatch into native effects. Tests neither call
//! `plugin_settlements::execute` directly nor hand-build the guest commands:
//! the request takes the same guest -> DTO path as a published component.
use std::num::NonZeroUsize;
use std::sync::Arc;

use crate::plugin_blueprint_tests::fixture as component_fixture;

use component_fixture::component_bytes;
use mc_plugin_host::bindings::exports::solaris::plugin::events::{
    CommandInvoked, Event, EventContext,
};
use mc_plugin_host::bindings::exports::solaris::plugin::lifecycle::InitContext;
use mc_plugin_host::bindings::solaris::plugin::types::Position;
use mc_plugin_host::{
    CommandBatch, CompiledPlugin, HostServices, LogLevel, NoSessions, PluginInstance, PluginLimits,
    PluginStartup, engine, linker,
};
use mc_script::{ScriptOperationRequest, ScriptOperationState};

use crate::tick::TickState;

/// The command root the guest fixture answers: the shared consumer contract.
const SETTLEMENT_API: &str = "settlement-api";

/// The guest configuration selecting the bounded settlement fixture mode.
const CONSUMER_CONFIG: &str = "mode = \"settlement-operations\"\n";

/// The services the consumer fixture runs with: it logs, and it has an id.
#[derive(Default)]
struct Services {
    id: String,
}

impl HostServices for Services {
    fn log(&mut self, _level: LogLevel, _message: &str) {}

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, position: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            position,
            mc_world::BlockStateId(self.0),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}

fn fixture() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn spawn_resident(state: &mut TickState, pos: [f64; 3]) -> uuid::Uuid {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState::default()));
    state.entities.row(slot).uuid
}

/// The actual SDK guest, compiled to a component by the shared fixture and
/// driven through configure/init. Startup contributes no catalogs and init
/// answers no commands: the fixture is a bounded reference consumer.
fn consumer() -> PluginInstance<Services> {
    let bytes = component_bytes();
    let limits = PluginLimits::default();
    let host_engine = engine(&limits).expect("engine");
    let compiled =
        CompiledPlugin::compile(&host_engine, &bytes, &limits, "0.8.0").expect("compile");
    let host_linker = linker::<Services>(&host_engine).expect("linker");
    let contribution = PluginStartup::instantiate(
        &host_linker,
        compiled.component(),
        Services {
            id: "consumer".to_owned(),
        },
        limits,
    )
    .expect("the startup store instantiates")
    .configure(CONSUMER_CONFIG)
    .expect("configure");
    assert!(
        contribution.is_none(),
        "the settlement fixture answers no startup contribution"
    );
    let mut plugin = PluginInstance::instantiate(
        &host_linker,
        compiled.component(),
        Services {
            id: "consumer".to_owned(),
        },
        limits,
    )
    .expect("instantiate");
    let init = plugin
        .init(
            CONSUMER_CONFIG,
            InitContext {
                plugin_id: "consumer".to_owned(),
                api_version: "0.8.0".to_owned(),
                world_fingerprint: String::new(),
            },
        )
        .expect("init");
    assert!(
        init.is_empty(),
        "the settlement fixture answers no init commands"
    );
    plugin
}
/// Drive one `settlement-api` command through the real guest and return its
/// actual staged batch: `["query", request, name]`,
/// `["bind"|"unbind", request, operation_id, id, uuid, revision]` or
/// `["rename", request, operation_id, id, new_name, revision]`. A valid
/// invocation answers exactly one operation command.
fn consumer_batch(plugin: &mut PluginInstance<Services>, arguments: &[&str]) -> CommandBatch {
    let invoked = CommandInvoked {
        player: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned(),
        session: 7,
        username: "Ada".to_owned(),
        operator: false,
        position: Position {
            x: 0.0,
            y: 64.0,
            z: 0.0,
        },
        name: SETTLEMENT_API.to_owned(),
        arguments: arguments.iter().map(ToString::to_string).collect(),
        raw_arguments: arguments.join(" "),
    };
    let context = EventContext {
        tick: 42,
        first_sequence: 0,
        count: 1,
    };
    let batch = plugin
        .on_events(context, &[Event::CommandInvoked(invoked)])
        .expect("guest answers the settlement command");
    assert_eq!(batch.len(), 1, "one operation command per valid invocation");
    batch
}

/// Drive one guest batch through the host adapter into the real core
/// dispatch, exactly as a guest answer travels: the staged batch is
/// converted to `ScriptCommand` under the package's grants, the admitted
/// operation request dispatches through `plugin::operation` (which also
/// records the durable receipt for mutation ids).
fn dispatch(state: &mut TickState, batch: CommandBatch) -> mc_script::ScriptOperationOutcome {
    let manifest = mc_script::ScriptPluginManifest::new(
        "consumer",
        "Consumer",
        "1.0.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_settlements()
    .validate()
    .expect("consumer manifest validates");
    let converted = mc_plugin_host::adapter::to_script_batch(
        batch,
        NonZeroUsize::new(256).expect("non-zero"),
        &NoSessions,
        &manifest.to_command_capabilities(),
    )
    .expect("settlement operation converts");
    assert_eq!(converted.commands().len(), 1);
    let mc_script::ScriptCommand::Operation { request } = converted.commands()[0].clone() else {
        panic!("settlement fixture answers an operation request");
    };
    crate::plugin::operation(state, "consumer", &request)
}

fn page_id(outcome: &mc_script::ScriptOperationOutcome, name: &str) -> (u64, u64) {
    let mc_script::ScriptOperationPayload::Settlement { result } = outcome.payload() else {
        panic!("query answers a settlement payload");
    };
    let mc_script::ScriptSettlementResult::Page(page) = result.as_ref() else {
        panic!("query answers a page");
    };
    let snapshot = page
        .settlements
        .iter()
        .find(|snapshot| snapshot.name == name)
        .expect("named record in page");
    (snapshot.id, snapshot.revision)
}

#[test]
fn settlement_consumer_query_then_bind_then_rename_through_real_dispatch() {
    let mut state = fixture();
    let mut plugin = consumer();
    // Native `/settlement create` is the only record path; the customization
    // surface never invents records.
    let anchor = [0, 64, 0];
    let id = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "town".into(),
            "consumer".into(),
            anchor,
            48,
        )
        .unwrap();
    let revision = state.settlements.get(id).unwrap().revision;
    crate::settlements::rebuild_counts(&mut state);

    // 1. query-settlement through guest -> host -> core dispatch. The
    // snapshot carries the monotonic id the later actions fence on.
    let outcome = dispatch(
        &mut state,
        consumer_batch(&mut plugin, &["query", "q-1", "town"]),
    );
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
    let (qid, qrev) = page_id(&outcome, "town");
    assert_eq!(qid, id.0);
    assert_eq!(qrev, revision);

    // 2. request-settlement-action bind through the same path.
    let uuid = spawn_resident(&mut state, [4.0, 65.0, 4.0]);
    let outcome = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "bind",
                "b-1",
                "bind-1",
                &qid.to_string(),
                &uuid.to_string(),
                &qrev.to_string(),
            ],
        ),
    );
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
    assert_eq!(crate::settlements::member_count(&state, id), 1);

    // 3. Stale-revision bind refuses without effects.
    let other = spawn_resident(&mut state, [5.0, 65.0, 5.0]);
    let outcome = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "bind",
                "b-2",
                "bind-2",
                &qid.to_string(),
                &other.to_string(),
                &(qrev + 99).to_string(),
            ],
        ),
    );
    assert_eq!(outcome.state(), ScriptOperationState::Rejected);

    // 4. Rename through the same path keeps the id and the membership.
    let outcome = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "rename",
                "r-1",
                "rename-1",
                &qid.to_string(),
                "borough",
                &qrev.to_string(),
            ],
        ),
    );
    let mc_script::ScriptOperationPayload::Settlement { result } = outcome.payload() else {
        panic!("rename answers a settlement payload");
    };
    let mc_script::ScriptSettlementResult::Changed { id: changed, name } = result.as_ref() else {
        panic!("rename answers the changed record");
    };
    assert_eq!(*changed, qid);
    assert_eq!(name, "borough");
    assert!(state.settlements.find("borough").is_some());
    assert_eq!(crate::settlements::member_count(&state, id), 1);

    // Mutation receipts are durable: re-probing the rename operation id
    // replays the recorded outcome instead of applying anything twice.
    let replay = ScriptOperationRequest::try_new(
        "replay",
        mc_script::ScriptOperation::Status {
            operation_id: "rename-1".into(),
        },
    )
    .unwrap();
    let outcome = crate::plugin::operation(&mut state, "consumer", &replay);
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
}

#[test]
fn settlement_consumer_stale_id_refuses_after_remove_and_recreate() {
    let mut state = fixture();
    let mut plugin = consumer();
    let anchor = [0, 64, 0];
    let first = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "town".into(),
            "consumer".into(),
            anchor,
            48,
        )
        .unwrap();
    let uuid = spawn_resident(&mut state, [4.0, 65.0, 4.0]);
    let resident = uuid_to_entity(&state, uuid);
    crate::settlements::bind(&mut state, first, resident).unwrap();
    assert_eq!(crate::settlements::member_count(&state, first), 1);

    // ABA: remove id1, recreate the same name (fresh id, revision 1 again).
    crate::settlements::remove(&mut state, first);
    let second = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "town".into(),
            "consumer".into(),
            anchor,
            48,
        )
        .unwrap();
    assert!(second.0 > first.0);
    assert_eq!(state.settlements.get(second).unwrap().revision, 1);

    // A stale bind fenced on the deleted id refuses; the new record keeps
    // zero members even though name+revision collide with the old read.
    let outcome = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "bind",
                "aba-1",
                "aba-bind-1",
                &first.0.to_string(),
                &uuid::Uuid::new_v4().to_string(),
                "1",
            ],
        ),
    );
    assert_eq!(outcome.state(), ScriptOperationState::Rejected);
    assert_eq!(crate::settlements::member_count(&state, second), 0);

    // A stale unbind naming the old id refuses even with a live villager.
    let villager = spawn_resident(&mut state, [5.0, 65.0, 5.0]);
    let outcome = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "unbind",
                "aba-2",
                "aba-unbind-1",
                &first.0.to_string(),
                &villager.to_string(),
                "1",
            ],
        ),
    );
    assert_eq!(outcome.state(), ScriptOperationState::Rejected);

    // A stale rename on the deleted id refuses; the recreated name survives.
    let outcome = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "rename",
                "aba-3",
                "aba-rename-1",
                &first.0.to_string(),
                "borough",
                "1",
            ],
        ),
    );
    assert_eq!(outcome.state(), ScriptOperationState::Rejected);
    assert!(state.settlements.find("town").is_some());
}

fn uuid_to_entity(state: &TickState, uuid: uuid::Uuid) -> crate::boundary::EntityId {
    let slot = state.entities.slot_of_uuid(uuid).expect("spawned resident");
    state.entities.body(slot).id
}

#[test]
fn household_consumer_forms_queries_and_relocates_with_both_revision_fences() {
    let mut state = fixture();
    let mut plugin = consumer();
    let source = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "source".into(),
            "consumer".into(),
            [0, 64, 0],
            48,
        )
        .unwrap();
    let target = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "target".into(),
            "consumer".into(),
            [16, 64, 0],
            48,
        )
        .unwrap();
    let a = spawn_resident(&mut state, [4.0, 65.0, 4.0]);
    let b = spawn_resident(&mut state, [5.0, 65.0, 4.0]);
    let people = [uuid_to_entity(&state, a), uuid_to_entity(&state, b)];
    for person in people {
        crate::settlements::bind(&mut state, source, person).unwrap();
    }
    let source_id = source.0.to_string();
    let target_id = target.0.to_string();
    let uuids = format!("{a},{b}");
    let form = [
        "form-household",
        "family-form-request",
        "family-form",
        &source_id,
        &uuids,
        "1",
    ];
    let outcome = dispatch(&mut state, consumer_batch(&mut plugin, &form));
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
    let revision = state.settlements.get(source).unwrap().revision;
    let replay = dispatch(&mut state, consumer_batch(&mut plugin, &form));
    assert_eq!(replay.state(), ScriptOperationState::Committed);
    assert_eq!(state.settlements.get(source).unwrap().revision, revision);
    let outcome = dispatch(
        &mut state,
        consumer_batch(&mut plugin, &["query", "family-query", "source"]),
    );
    let mc_script::ScriptOperationPayload::Settlement { result } = outcome.payload() else {
        panic!("settlement payload")
    };
    let mc_script::ScriptSettlementResult::Page(page) = result.as_ref() else {
        panic!("settlement page")
    };
    let family = &page.settlements[0].households[0];
    assert_eq!(family.origin, "adoption");
    let mut expected = vec![a.to_string(), b.to_string()];
    expected.sort_unstable();
    assert_eq!(family.members, expected);
    let family = family.id.clone();
    state
        .settlements
        .rename(target, "destination".into())
        .unwrap();
    let revision_text = revision.to_string();
    let stale = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "relocate-household",
                "family-stale-request",
                "family-stale",
                &source_id,
                &family,
                &target_id,
                &revision_text,
                "1",
            ],
        ),
    );
    assert_eq!(
        stale.failure(),
        Some(mc_script::ScriptOperationFailure::StaleRevision)
    );
    assert!(
        state
            .settlements
            .get(source)
            .unwrap()
            .households
            .contains_key(&family)
    );
    let movement = [
        "relocate-household",
        "family-move-request",
        "family-move",
        &source_id,
        &family,
        &target_id,
        &revision_text,
        "2",
    ];
    let outcome = dispatch(&mut state, consumer_batch(&mut plugin, &movement));
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
    let outcome = dispatch(&mut state, consumer_batch(&mut plugin, &movement));
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
    for person in people {
        let slot = state.entities.slot_of(person).unwrap();
        let v = state.entities.row(slot).villager.as_ref().unwrap();
        assert_eq!(v.settlement, Some(target));
        assert_eq!(v.household.as_deref(), Some(family.as_str()));
    }
    let outcome = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &["query", "family-destination-query", "destination"],
        ),
    );
    let mc_script::ScriptOperationPayload::Settlement { result } = outcome.payload() else {
        panic!("settlement payload")
    };
    let mc_script::ScriptSettlementResult::Page(page) = result.as_ref() else {
        panic!("settlement page")
    };
    assert_eq!(page.settlements[0].households[0].id, family);
    assert_eq!(page.settlements[0].households[0].members, expected);
}

#[test]
fn district_plan_requires_real_native_building_authority() {
    let mut state = fixture();
    let id = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "district".into(),
            "consumer".into(),
            [0, 64, 0],
            48,
        )
        .unwrap();
    state.settlements.take_images();
    let operation = serde_json::from_value(serde_json::json!({
        "kind": "plan_district",
        "operation_id": "district-missing-building",
        "settlement": id.0,
        "expected_revision": 1,
        "branch": "none",
        "level": 0,
        "buildings": [{
            "kind": "worksite",
            "id": "missing-certificate",
            "purpose": "solaris:warehouse"
        }]
    }))
    .expect("native district action is part of the settlement contract");
    let request = ScriptOperationRequest::try_new(
        "district-missing",
        mc_script::ScriptOperation::Settlement { operation },
    )
    .unwrap();
    let outcome = crate::plugin::operation(&mut state, "consumer", &request);
    assert_eq!(
        outcome.failure(),
        Some(mc_script::ScriptOperationFailure::NotFound)
    );
    assert!(state.settlements.take_images().is_empty());
}

#[test]
fn district_consumer_plans_real_worksite_queries_and_replays_through_guest() {
    use mc_script::{
        ScriptOperation, ScriptOperationFailure, ScriptOperationPayload, ScriptSettlementResult,
        ScriptWorksiteInteractionPoint, ScriptWorksiteRequest, ScriptWorldBounds,
        ScriptWorldStructureOperation, ScriptWorldStructureResult,
    };
    let mut state = fixture();
    let wanted = rustc_hash::FxHashMap::from_iter([(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]);
    state.world.ensure_blocking(&wanted);
    state.world.set_block(4, 64, 4, state.registry.states.stone);
    state.world.fold_overlay();
    let bounds = ScriptWorldBounds::new([4, 65, 4], [4, 65, 4]).unwrap();
    let survey_request = ScriptOperationRequest::try_new(
        "district-guest-survey",
        ScriptOperation::WorldStructure {
            operation: ScriptWorldStructureOperation::Survey {
                dimension: state.dimension.name().into(),
                bounds,
            },
        },
    )
    .unwrap();
    let surveyed = crate::plugin::operation(&mut state, "consumer", &survey_request);
    let ScriptOperationPayload::WorldStructure { result } = surveyed.payload() else {
        panic!("native survey payload: {surveyed:?}")
    };
    let ScriptWorldStructureResult::Survey { survey } = result.as_ref() else {
        panic!("native survey result")
    };
    let register = ScriptOperationRequest::try_new(
        "district-guest-register",
        ScriptOperation::WorldStructure {
            operation: ScriptWorldStructureOperation::RegisterWorksite {
                operation_id: "district-guest-register-once".into(),
                request: Box::new(ScriptWorksiteRequest::new(
                    bounds,
                    vec![ScriptWorksiteInteractionPoint::Entrance { at: [4, 65, 4] }],
                    survey.survey_token.clone(),
                    survey.world_revision,
                )),
            },
        },
    )
    .unwrap();
    let registered = crate::plugin::operation(&mut state, "consumer", &register);
    let ScriptOperationPayload::WorldStructure { result } = registered.payload() else {
        panic!("native worksite payload: {registered:?}")
    };
    let ScriptWorldStructureResult::Worksite { worksite } = result.as_ref() else {
        panic!("native worksite result")
    };
    let references = format!("worksite|{}|solaris:farm", worksite.worksite_id);
    let id = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "district-guest".into(),
            "consumer".into(),
            [4, 65, 4],
            48,
        )
        .unwrap();
    let id_text = id.0.to_string();
    state.settlements.take_images();
    let mut plugin = consumer();
    let args = [
        "plan-district",
        "district-guest-plan-request",
        "district-guest-plan",
        &id_text,
        "1",
        "none",
        "0",
        &references,
    ];
    let outcome = dispatch(&mut state, consumer_batch(&mut plugin, &args));
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
    assert_eq!(state.settlements.take_images().len(), 1);
    let outcome = dispatch(&mut state, consumer_batch(&mut plugin, &args));
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
    assert!(state.settlements.take_images().is_empty());
    let stale = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "plan-district",
                "district-guest-stale-request",
                "district-guest-stale",
                &id_text,
                "1",
                "town",
                "1",
                &references,
            ],
        ),
    );
    assert_eq!(stale.failure(), Some(ScriptOperationFailure::StaleRevision));
    let outcome = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &["query", "district-guest-query", "district-guest"],
        ),
    );
    let ScriptOperationPayload::Settlement { result } = outcome.payload() else {
        panic!("settlement result")
    };
    let ScriptSettlementResult::Page(page) = result.as_ref() else {
        panic!("settlement page")
    };
    let district = page.settlements[0].district.as_ref().unwrap();
    assert_eq!(district.buildings[0].kind, "worksite");
    assert_eq!(district.buildings[0].purpose, "solaris:farm");
    assert_eq!(district.population, 0);
    assert_eq!(district.stage, None);
    assert_eq!(
        district.reason.as_deref(),
        Some("No actual native residents.")
    );
    assert!(state.settlements.take_images().is_empty());
}

#[test]
fn economy_consumer_queries_quotes_and_accepts_real_native_stock_through_guest() {
    use mc_script::{ScriptOperationFailure, ScriptOperationPayload, ScriptSettlementResult};
    let mut state = fixture();
    let mut plugin = consumer();
    let id = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "guest-market".into(),
            "consumer".into(),
            [0, 64, 0],
            48,
        )
        .unwrap();
    let id_text = id.0.to_string();
    let seller = spawn_resident(&mut state, [4.0, 65.0, 4.0]);
    let buyer = spawn_resident(&mut state, [5.0, 65.0, 5.0]);
    for (uuid, request) in [(seller, "seller-bind"), (buyer, "buyer-bind")] {
        let revision = state.settlements.get(id).unwrap().revision.to_string();
        let outcome = dispatch(
            &mut state,
            consumer_batch(
                &mut plugin,
                &[
                    "bind",
                    request,
                    request,
                    &id_text,
                    &uuid.to_string(),
                    &revision,
                ],
            ),
        );
        assert_eq!(outcome.state(), ScriptOperationState::Committed);
    }
    let bread = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:bread").unwrap())
        .unwrap();
    let emerald = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    let seller_slot = state.entities.slot_of_uuid(seller).unwrap();
    let buyer_slot = state.entities.slot_of_uuid(buyer).unwrap();
    let mut loaf = mc_data::ItemStack::new(bread, 5);
    loaf.custom_name = Some("guest market loaf".into());
    state
        .entities
        .row_mut(seller_slot)
        .villager
        .as_mut()
        .unwrap()
        .inventory[0] = Some(loaf);
    state
        .entities
        .row_mut(buyer_slot)
        .villager
        .as_mut()
        .unwrap()
        .inventory[0] = Some(mc_data::ItemStack::new(emerald, 3));
    let queried = dispatch(
        &mut state,
        consumer_batch(&mut plugin, &["query", "market-query", "guest-market"]),
    );
    let (observed_id, revision) = page_id(&queried, "guest-market");
    assert_eq!(observed_id, id.0);
    let revision = revision.to_string();
    let seller_text = seller.to_string();
    let buyer_text = buyer.to_string();
    let quote_args = [
        "economy-quote",
        "quote-request",
        "guest-quote",
        &id_text,
        &revision,
        "1",
        &seller_text,
        &buyer_text,
        "0",
        "2",
        "3",
    ];
    let quoted = dispatch(&mut state, consumer_batch(&mut plugin, &quote_args));
    assert_eq!(quoted.state(), ScriptOperationState::Committed);
    let query = dispatch(
        &mut state,
        consumer_batch(&mut plugin, &["query", "quoted-query", "guest-market"]),
    );
    let ScriptOperationPayload::Settlement { result } = query.payload() else {
        panic!("native settlement query payload")
    };
    let ScriptSettlementResult::Page(page) = result.as_ref() else {
        panic!("native settlement page")
    };
    let observation = page.settlements[0].economy.as_ref().unwrap();
    assert_eq!(observation.quotes.len(), 1);
    assert_eq!(
        (
            observation.quotes[0].item_id.as_str(),
            observation.quotes[0].quantity,
            observation.quotes[0].price
        ),
        ("minecraft:bread", 2, 3),
    );
    let stale = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "economy-accept",
                "stale-request",
                "stale-accept",
                &id_text,
                &revision,
                "1",
            ],
        ),
    );
    assert_eq!(stale.failure(), Some(ScriptOperationFailure::StaleRevision));
    let revision = page.settlements[0].revision.to_string();
    let accepted = dispatch(
        &mut state,
        consumer_batch(
            &mut plugin,
            &[
                "economy-accept",
                "accept-request",
                "guest-accept",
                &id_text,
                &revision,
                "1",
            ],
        ),
    );
    assert_eq!(accepted.state(), ScriptOperationState::Committed);
    let revision = state.settlements.get(id).unwrap().revision;
    assert_eq!(
        dispatch(&mut state, consumer_batch(&mut plugin, &quote_args)),
        quoted
    );
    assert_eq!(state.settlements.get(id).unwrap().revision, revision);
    let policy = &state.settlements.get(id).unwrap().economy.commerce;
    assert!(policy.quotes.is_empty());
    assert_eq!(policy.completed_quote, 1);
    let total = |slot, item| -> i32 {
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .iter()
            .flatten()
            .filter(|stack| stack.item_id == item)
            .map(|stack| stack.count)
            .sum()
    };
    assert_eq!(
        (total(seller_slot, bread), total(seller_slot, emerald)),
        (3, 3)
    );
    assert_eq!(
        (total(buyer_slot, bread), total(buyer_slot, emerald)),
        (2, 0)
    );
    assert!(
        state
            .entities
            .row(buyer_slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .iter()
            .flatten()
            .any(|stack| stack.item_id == bread
                && stack.custom_name.as_deref() == Some("guest market loaf"))
    );
}
