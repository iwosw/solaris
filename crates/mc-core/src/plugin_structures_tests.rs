use super::*;
use crate::boundary::ChunkPos;

fn state() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let terrain = mc_worldgen::TerrainGenerator::new(42, registry.blocks.clone());
    let spawn = terrain.locate_safe_spawn().unwrap();
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(terrain),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    state.tickets.insert(
        "structure-tests".into(),
        (
            ChunkPos {
                x: spawn.block_x.div_euclid(16),
                z: spawn.block_z.div_euclid(16),
            },
            0,
        ),
    );
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    state.refresh_active_chunks(state.tick);
    state
}
fn anchor(state: &TickState) -> [i32; 3] {
    let spawn = state.world.world_spawn();
    [
        spawn[0].div_euclid(16) * 16 + 4,
        200,
        spawn[2].div_euclid(16) * 16 + 4,
    ]
}
fn authored(id: &str) -> String {
    format!(
        "id = \"{id}\"\nrevision = 1\n[footprint]\nsize = [2,2,3]\nanchor = [0,0,0]\n[[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {{}}\n[[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n[[blocks]]\nx = 1\ny = 1\nz = 2\npalette = 0\n[[street_connection]]\nat = [0,0,1]\nfacing = \"north\"\n"
    )
}
fn blueprint(state: &TickState) -> Arc<Blueprint> {
    BlueprintCatalog::from_files(
        &state.registry.blocks,
        "authored",
        &[("house.toml".into(), authored("authored:house"))],
    )
    .unwrap()
    .get("authored:house")
    .unwrap()
    .clone()
}
fn stored_structure(state: &mut TickState) -> String {
    let blueprint = blueprint(state);
    let at = anchor(state);
    let instance = BlueprintInstance::new(blueprint.clone(), QuarterTurn::None, at);
    let (stages, cells) = planned_cells(state, &instance).unwrap();
    let plan = resource_plan(&stages);
    let remaining: Vec<_> = stages.iter().flat_map(|s| s.materials.clone()).collect();
    let snapshot = ScriptStructureSnapshot::new(
        "saved-house".into(),
        blueprint.id().into(),
        state.dimension.name().into(),
        ScriptStructureState::Prepared,
        1,
        at,
        0,
        blueprint.size(),
        stages,
        0,
        0,
        inventories::resource_plan_hash(&plan),
        None,
        0,
        Vec::new(),
        remaining,
        None,
        worksites::planned_points(state, &instance).unwrap(),
    );
    let image = fingerprint(state, bounds(at, blueprint.size()).unwrap()).unwrap();
    let mut ledger = Ledger {
        revision: 1,
        ..Default::default()
    };
    ledger.structures.insert(
        snapshot.structure_id.clone(),
        Structure {
            owner: "demo".into(),
            snapshot,
            blueprint_hash: blueprint.content_hash().into(),
            image,
            cells,
            receipts: Vec::new(),
        },
    );
    save(state, &ledger);
    "saved-house".into()
}

#[test]
fn strict_catalog_uses_authored_namespace_not_package_id() {
    let state = state();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("structures")).unwrap();
    std::fs::write(
        root.path().join("structures/house.toml"),
        authored("authored:house"),
    )
    .unwrap();
    let roots = BTreeMap::from([("arbitrary-package-id".into(), root.path().to_path_buf())]);
    let catalogs = load_catalog(&roots, &state.registry.blocks).unwrap();
    assert!(
        catalogs["arbitrary-package-id"]
            .get("authored:house")
            .is_some()
    );
    assert!(
        catalogs["arbitrary-package-id"]
            .get("arbitrary-package-id:house")
            .is_none()
    );
}
#[test]
fn duplicate_cross_package_blueprint_identity_fails_startup() {
    let state = state();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    for root in [&first, &second] {
        std::fs::create_dir(root.path().join("structures")).unwrap();
        std::fs::write(
            root.path().join("structures/house.toml"),
            authored("authored:house"),
        )
        .unwrap();
    }
    let roots = BTreeMap::from([
        ("one".into(), first.path().to_path_buf()),
        ("two".into(), second.path().to_path_buf()),
    ]);
    assert!(load_catalog(&roots, &state.registry.blocks).is_err());
}
#[test]
fn stray_and_symlinked_catalog_entries_are_not_silently_accepted() {
    let state = state();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("structures");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a blueprint").unwrap();
    let roots = BTreeMap::from([("demo".into(), root.path().to_path_buf())]);
    assert!(load_catalog(&roots, &state.registry.blocks).is_err());
    std::fs::remove_file(dir.join("notes.txt")).unwrap();
    let outside = root.path().join("outside.toml");
    std::fs::write(&outside, authored("authored:house")).unwrap();
    std::os::unix::fs::symlink(outside, dir.join("house.toml")).unwrap();
    assert!(load_catalog(&roots, &state.registry.blocks).is_err());
}
#[test]
fn stage_geometry_rotates_actual_native_states_and_costs_exact_blocks() {
    let state = state();
    let at = anchor(&state);
    let instance = BlueprintInstance::new(blueprint(&state), QuarterTurn::Cw90, at);
    let (plans, cells) = planned_cells(&state, &instance).unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].stage, "body");
    assert_eq!(plans[0].work_units, 2);
    assert_eq!(
        plans[0].materials,
        vec![ScriptStructureMaterial::new("minecraft:stone".into(), 2)]
    );
    let positions: Vec<_> = cells[0].iter().map(|cell| cell.at).collect();
    assert_eq!(
        positions,
        vec![[at[0], at[1] + 1, at[2] + 1], [at[0] + 2, at[1], at[2]]]
    );
    assert!(cells[0].iter().all(|cell| {
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(cell.state))
            .unwrap()
            .block
            .id
            .as_str()
            == "minecraft:stone"
    }));
}
#[test]
fn survey_token_is_real_volume_fence_not_unrelated_chunk_revision() {
    let mut state = state();
    let at = anchor(&state);
    let bounds = ScriptWorldBounds::new(at, [at[0] + 1, at[1] + 1, at[2] + 1]).unwrap();
    let (_, result) = survey(&mut state, "demo", "minecraft:overworld", bounds).unwrap();
    let ScriptWorldStructureResult::Survey { survey } = result else {
        panic!("survey")
    };
    let stone = state.registry.states.stone;
    state.world.set_block(at[0] + 3, at[1], at[2], stone);
    let ledger = ledger(&state).unwrap();
    validate_survey(
        &state,
        "demo",
        &ledger,
        &survey.survey_token,
        "minecraft:overworld",
        bounds,
        survey.world_revision,
    )
    .unwrap();
    state.world.set_block(at[0], at[1], at[2], stone);
    assert_eq!(
        validate_survey(
            &state,
            "demo",
            &ledger,
            &survey.survey_token,
            "minecraft:overworld",
            bounds,
            survey.world_revision
        )
        .err(),
        Some(Failure::StaleRevision)
    );
}
#[test]
fn survey_unknown_chunks_refuses_instead_of_minting_air_facts() {
    let mut state = state();
    let bounds = ScriptWorldBounds::new([20_000, 100, 20_000], [20_001, 101, 20_001]).unwrap();
    assert_eq!(
        survey(&mut state, "demo", "minecraft:overworld", bounds).err(),
        Some(Failure::Unloaded)
    );
    assert!(ledger(&state).unwrap().surveys.is_empty());
    let facts = worksites::facts(&state, [20_000, 100, 20_000]).unwrap();
    assert_eq!(facts.block, None);
    assert!(!facts.standable);
    assert!(!facts.container);
}
#[test]
fn worksite_registration_checks_live_point_and_owned_survey() {
    let mut state = state();
    let at = anchor(&state);
    let bounds = ScriptWorldBounds::new(at, at).unwrap();
    let (_, result) = survey(&mut state, "demo", "minecraft:overworld", bounds).unwrap();
    let ScriptWorldStructureResult::Survey { survey } = result else {
        panic!("survey")
    };
    let request = ScriptWorksiteRequest::new(
        bounds,
        vec![ScriptWorksiteInteractionPoint::Block {
            at,
            block: "minecraft:stone".into(),
        }],
        survey.survey_token.clone(),
        survey.world_revision,
    );
    let operation = ScriptWorldStructureOperation::RegisterWorksite {
        operation_id: "register".into(),
        request: Box::new(request),
    };
    assert_eq!(
        execute_operation(&mut state, "other", &operation).err(),
        Some(Failure::Forbidden)
    );
    assert_eq!(
        execute_operation(&mut state, "demo", &operation).err(),
        Some(Failure::Blocked)
    );
    assert!(ledger(&state).unwrap().worksites.is_empty());
}
#[test]
fn saved_structure_has_real_status_and_lifecycle_fences() {
    let mut state = state();
    let id = stored_structure(&mut state);
    validate(&state).unwrap();
    let (_, result) = execute_operation(
        &mut state,
        "demo",
        &ScriptWorldStructureOperation::Status {
            structure_id: id.clone(),
        },
    )
    .unwrap();
    let ScriptWorldStructureResult::Structure { structure } = result else {
        panic!("status")
    };
    assert_eq!(structure.current_stage_index, 0);
    assert_eq!(structure.completed_work_units, 0);
    assert_eq!(
        execute_operation(
            &mut state,
            "other",
            &ScriptWorldStructureOperation::Status {
                structure_id: id.clone()
            }
        )
        .err(),
        Some(Failure::Forbidden)
    );
    let (_, paused) = execute_operation(
        &mut state,
        "demo",
        &ScriptWorldStructureOperation::PauseStructure {
            operation_id: "pause".into(),
            structure_id: id.clone(),
            expected_revision: 1,
        },
    )
    .unwrap();
    let ScriptWorldStructureResult::Structure { structure: paused } = paused else {
        panic!("pause")
    };
    assert_eq!(paused.state, ScriptStructureState::Paused);
    assert_eq!(
        execute_operation(
            &mut state,
            "demo",
            &ScriptWorldStructureOperation::ResumeStructure {
                operation_id: "resume-stale".into(),
                structure_id: id.clone(),
                expected_revision: 1
            }
        )
        .err(),
        Some(Failure::StaleRevision)
    );
    let (_, resumed) = execute_operation(
        &mut state,
        "demo",
        &ScriptWorldStructureOperation::ResumeStructure {
            operation_id: "resume".into(),
            structure_id: id,
            expected_revision: paused.revision,
        },
    )
    .unwrap();
    let ScriptWorldStructureResult::Structure { structure: resumed } = resumed else {
        panic!("resume")
    };
    assert_eq!(resumed.state, ScriptStructureState::Running);
}
#[test]
fn changed_site_prevents_resume_without_hiding_native_changes() {
    let mut state = state();
    let id = stored_structure(&mut state);
    let (revision, _) = execute_operation(
        &mut state,
        "demo",
        &ScriptWorldStructureOperation::PauseStructure {
            operation_id: "pause".into(),
            structure_id: id.clone(),
            expected_revision: 1,
        },
    )
    .unwrap();
    let at = anchor(&state);
    let stone = state.registry.states.stone;
    state.world.set_block(at[0], at[1], at[2], stone);
    assert_eq!(
        execute_operation(
            &mut state,
            "demo",
            &ScriptWorldStructureOperation::ResumeStructure {
                operation_id: "resume".into(),
                structure_id: id,
                expected_revision: revision
            }
        )
        .err(),
        Some(Failure::Blocked)
    );
    assert_eq!(state.world.block_at(at[0], at[1], at[2]), Some(stone));
}
#[test]
fn corrupt_staged_geometry_cannot_recover_as_success() {
    let mut state = state();
    let id = stored_structure(&mut state);
    let mut ledger = ledger(&state).unwrap();
    ledger.structures.get_mut(&id).unwrap().cells[0][0].at[0] += 100;
    save(&mut state, &ledger);
    assert!(validate(&state).is_err());
}

#[test]
fn survey_expiry_counts_global_plugin_transactions_not_local_domain_updates() {
    let mut state = state();
    let at = anchor(&state);
    let bounds = ScriptWorldBounds::new(at, at).unwrap();
    let (_, result) = survey(&mut state, "demo", "minecraft:overworld", bounds).unwrap();
    let ScriptWorldStructureResult::Survey { survey } = result else {
        panic!("survey")
    };
    let mut local = ledger(&state).unwrap();
    assert_eq!(local.surveys[&survey.survey_token].expires, 64);
    local.revision += 100;
    save(&mut state, &local);
    for index in 0..64 {
        state.plugins.apply_storage(
            "other-package",
            &[ScriptStorageMutation::CompareAndSwap {
                key: format!("transaction-{index}"),
                expected_version: None,
                value: "committed".into(),
            }],
        );
    }
    validate_survey(
        &state,
        "demo",
        &local,
        &survey.survey_token,
        "minecraft:overworld",
        bounds,
        survey.world_revision,
    )
    .unwrap();
    state.plugins.apply_storage(
        "other-package",
        &[ScriptStorageMutation::CompareAndSwap {
            key: "transaction-64".into(),
            expected_version: None,
            value: "committed".into(),
        }],
    );
    assert_eq!(
        validate_survey(
            &state,
            "demo",
            &local,
            &survey.survey_token,
            "minecraft:overworld",
            bounds,
            survey.world_revision,
        ),
        Err(Failure::StaleRevision)
    );
}

fn held_structure(state: &mut TickState) -> (String, String, ScriptInventoryEndpoint) {
    let id = stored_structure(state);
    let at = anchor(state);
    let pos = BlockPos {
        x: at[0] + 4,
        y: at[1],
        z: at[2],
    };
    let chest = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:chest").unwrap())
        .unwrap()
        .default
        .0;
    crate::block_entity_defaults::accepted_placement(state, pos, chest);
    state.world.set_block(pos.x, pos.y, pos.z, chest);
    crate::block_entity_defaults::after_blocks(state, &[(pos, chest)]);
    let stone = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    state
        .block_entities
        .get_mut(&(pos.x, pos.y, pos.z))
        .unwrap()
        .slots[0] = mc_data::ItemStack::new(stone, 2);
    let endpoint = ScriptInventoryEndpoint::Container {
        dimension: state.dimension.name().into(),
        position: ScriptBlockPosition::new(pos.x, pos.y, pos.z),
    };
    let mut local = ledger(state).unwrap();
    let record = local.structures.get_mut(&id).unwrap();
    let plan = resource_plan(&record.snapshot.stages);
    let fence = inventories::load(state, "demo", &endpoint).unwrap().fence;
    let request = ScriptOperationRequest::try_new(
        "reserve-materials",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Reserve {
                operation_id: "reserve-materials".into(),
                endpoint: endpoint.clone(),
                resource_plan: plan,
                expected_revision: fence,
            },
        },
    )
    .unwrap();
    inventories::execute(state, "demo", &request);
    let reference =
        inventories::matching_reservation(state, "demo", &record.snapshot.resource_plan_hash, &id)
            .unwrap();
    let mut reservation = inventories::reservation(state, "demo", &reference).unwrap();
    reservation.snapshot.bound_to = Some(id.clone());
    inventories::save_reservation(state, reservation).unwrap();
    record.snapshot.reservation_ref = Some(reference.clone());
    save(state, &local);
    (id, reference, endpoint)
}

#[test]
fn cancellation_unlocks_native_source_without_minting_refunds() {
    let mut state = state();
    let (id, reference, endpoint) = held_structure(&mut state);
    let before = inventories::load(&state, "demo", &endpoint).unwrap();
    assert_eq!(
        inventories::reserved(&state, &endpoint)["minecraft:stone"],
        2
    );
    execute_operation(
        &mut state,
        "demo",
        &ScriptWorldStructureOperation::CancelStructure {
            operation_id: "cancel".into(),
            structure_id: id,
            expected_revision: 1,
        },
    )
    .unwrap();
    let after = inventories::load(&state, "demo", &endpoint).unwrap();
    assert_eq!(before.slots, after.slots);
    assert_eq!(before.fence, after.fence);
    assert!(inventories::reserved(&state, &endpoint).is_empty());
    let reservation = inventories::reservation(&state, "demo", &reference).unwrap();
    assert!(reservation.snapshot.released);
    assert_eq!(reservation.snapshot.quantities[0].returned, 2);
    assert!(reservation.snapshot.bound_to.is_none());
    assert_eq!(reservation.snapshot.quantities[0].consumed, 0);
}

#[test]
fn cancelled_structure_survives_pruned_history_but_live_or_wrong_owner_reference_fails() {
    let mut state = state();
    let (id, reference, _) = held_structure(&mut state);
    execute_operation(
        &mut state,
        "demo",
        &ScriptWorldStructureOperation::CancelStructure {
            operation_id: "cancel".into(),
            structure_id: id.clone(),
            expected_revision: 1,
        },
    )
    .unwrap();
    let released = inventories::reservation(&state, "demo", &reference).unwrap();
    for index in 0..256 {
        let mut later = released.clone();
        later.snapshot.reservation_ref = format!("later-{index}");
        inventories::save_reservation(&mut state, later).unwrap();
    }
    assert!(!inventories::retains_reservation(&state, &reference));
    inventories::validate(&state).unwrap();
    validate(&state).unwrap();

    let mut local = ledger(&state).unwrap();
    local.structures.get_mut(&id).unwrap().snapshot.state = ScriptStructureState::Running;
    save(&mut state, &local);
    assert!(validate(&state).is_err());

    let mut foreign = released;
    foreign.owner = "other".into();
    foreign.snapshot.reservation_ref = "foreign".into();
    inventories::save_reservation(&mut state, foreign).unwrap();
    let record = local.structures.get_mut(&id).unwrap();
    record.snapshot.state = ScriptStructureState::Cancelled;
    record.snapshot.reservation_ref = Some("foreign".into());
    save(&mut state, &local);
    assert!(validate(&state).is_err());
}

#[test]
fn cancellation_capacity_failure_keeps_source_reservation_and_domain_unchanged() {
    let mut state = state();
    let (id, reference, endpoint) = held_structure(&mut state);
    let mut local = ledger(&state).unwrap();
    local.revision = u64::MAX;
    save(&mut state, &local);
    let source = inventories::load(&state, "demo", &endpoint).unwrap();
    let reservation =
        serde_json::to_value(inventories::reservation(&state, "demo", &reference).unwrap())
            .unwrap();
    let domain = state.plugins.get_domain(OWNER, KEY).unwrap().clone();
    assert_eq!(
        execute_operation(
            &mut state,
            "demo",
            &ScriptWorldStructureOperation::CancelStructure {
                operation_id: "cancel".into(),
                structure_id: id,
                expected_revision: 1,
            },
        )
        .err(),
        Some(Failure::Capacity)
    );
    assert_eq!(state.plugins.get_domain(OWNER, KEY), Some(&domain));
    assert_eq!(
        serde_json::to_value(inventories::reservation(&state, "demo", &reference).unwrap())
            .unwrap(),
        reservation
    );
    assert_eq!(
        source.slots,
        inventories::load(&state, "demo", &endpoint).unwrap().slots
    );
    assert_eq!(
        inventories::reserved(&state, &endpoint)["minecraft:stone"],
        2
    );
}

struct StructureBuildFixture {
    state: TickState,
    boundary: ScriptBoundary,
    host: ScriptHostEndpoint,
    admission: HostCommandAdmission,
    request: ScriptOperationRequest,
    endpoint: ScriptInventoryEndpoint,
}

impl StructureBuildFixture {
    fn new(hooked: bool) -> Self {
        use mc_script::precommit::{HookFailurePolicy, HookKind, HookRegistration};
        use std::num::NonZeroUsize;
        let mut state = state();
        let (boundary, host) = script_boundary_pair(
            NonZeroUsize::new(128).unwrap(),
            NonZeroUsize::new(128).unwrap(),
        );
        let manifest = ScriptPluginManifest::new(
            "demo",
            "Generic structure builder",
            "1",
            COMPONENT_PLUGIN_API_VERSION,
        )
        .declare_world_structures()
        .validate()
        .unwrap();
        host.register_plugin_routes(&manifest).unwrap();
        if hooked {
            boundary
                .set_precommit_hooks(vec![HookRegistration::new(
                    "protector",
                    HookKind::Build,
                    0,
                    HookFailurePolicy::Deny,
                )])
                .unwrap();
        }
        let mut runtime = crate::plugin::PluginRuntime::with_test_boundary(boundary.clone());
        runtime.set_test_catalogs(BTreeMap::from([(
            "demo".into(),
            BlueprintCatalog::from_files(
                &state.registry.blocks,
                "authored",
                &[("house.toml".into(), authored("authored:house"))],
            )
            .unwrap(),
        )]));
        state.plugin_runtime = Some(runtime);
        let (structure_id, reservation_ref, endpoint) = held_structure(&mut state);
        let request = ScriptOperationRequest::try_new(
            "advance-request",
            ScriptOperation::WorldStructure {
                operation: ScriptWorldStructureOperation::AdvanceStructure {
                    operation_id: "advance".into(),
                    structure_id,
                    stage: "body".into(),
                    reservation_ref,
                    expected_revision: 1,
                    work_units: 2,
                },
            },
        )
        .unwrap();
        Self {
            state,
            boundary,
            host,
            admission: HostCommandAdmission::from_manifest(&manifest),
            request,
            endpoint,
        }
    }

    fn submit(&self, request: ScriptOperationRequest) {
        let mut batch = CommandBatch::new(std::num::NonZeroUsize::new(1).unwrap());
        batch
            .try_push_authorized(
                ScriptCommand::Operation { request },
                self.admission.capabilities(),
            )
            .unwrap();
        self.host
            .try_submit_plugin_batch(&self.admission, batch)
            .unwrap();
    }

    fn question(&mut self) -> mc_script::precommit::Request {
        match self.host.recv_input_blocking() {
            Some(ScriptHostInput::Precommit(request)) => request,
            _ => panic!("server-owned construction must ask the canonical build hook"),
        }
    }

    fn outcomes(&self) -> Vec<ScriptOperationOutcome> {
        self.state
            .plugin_runtime
            .as_ref()
            .unwrap()
            .pending_events
            .iter()
            .filter_map(|event| {
                assert_eq!(event.target_plugin_id(), Some("demo"));
                match event.kind() {
                    ScriptEventKind::OperationResult { outcome, .. } => Some(outcome.clone()),
                    _ => None,
                }
            })
            .collect()
    }
}

#[test]
fn server_owned_build_waits_nonblocking_for_keep_and_coalesces_exact_once_retries() {
    use mc_script::precommit::{HookActor, HookContext, HookDecision};
    let mut fixture = StructureBuildFixture::new(true);
    let plan = prepare_build(&fixture.state, "demo", &fixture.request)
        .unwrap()
        .unwrap();
    let before = serde_json::to_value(&fixture.state.plugins).unwrap();
    fixture.submit(fixture.request.clone());
    fixture.submit(
        ScriptOperationRequest::try_new("retry-request", fixture.request.operation().clone())
            .unwrap(),
    );
    crate::plugin::pump(&mut fixture.state);
    assert!(fixture.outcomes().is_empty());
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        before
    );
    assert!(fixture.state.plugin_block_writes.is_empty());
    let question = fixture.question();
    assert_eq!(question.context(), &plan.context);
    let HookContext::Build(context) = question.context() else {
        panic!("construction asks a build question");
    };
    assert_eq!(context.actor(), &HookActor::Plugin("demo".into()));
    assert_eq!(context.dimension(), fixture.state.dimension.name());
    assert_eq!(context.edits().len(), 2);
    for edit in context.edits() {
        assert_eq!(
            fixture.state.world.block_at(edit.x, edit.y, edit.z),
            Some(edit.previous_state)
        );
    }
    question.answer(HookDecision::Keep, Vec::new()).unwrap();
    crate::plugin::pump(&mut fixture.state);
    let outcomes = fixture.outcomes();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(outcomes[0].state(), ScriptOperationState::Committed);
    assert_eq!(fixture.state.plugin_block_writes.len(), 2);
    assert!(
        inventories::load(&fixture.state, "demo", &fixture.endpoint)
            .unwrap()
            .slots
            .values()
            .all(|stack| stack.is_empty())
    );
    let committed = serde_json::to_value(&fixture.state.plugins).unwrap();
    assert_eq!(
        crate::plugin::operation(&mut fixture.state, "demo", &fixture.request),
        outcomes[0]
    );
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        committed
    );
}

#[test]
fn denied_failed_and_expired_server_owned_builds_leave_no_world_or_plugin_post_image() {
    use mc_script::precommit::{HookDecision, HookFailure};
    for failure in [
        None,
        Some(HookFailure::Unavailable),
        Some(HookFailure::Expired),
    ] {
        let mut fixture = StructureBuildFixture::new(true);
        let before = serde_json::to_value(&fixture.state.plugins).unwrap();
        let inventory = inventories::load(&fixture.state, "demo", &fixture.endpoint).unwrap();
        fixture.submit(fixture.request.clone());
        crate::plugin::pump(&mut fixture.state);
        let question = fixture.question();
        if let Some(failure) = failure {
            question.fail(failure).unwrap();
        } else {
            question.answer(HookDecision::Cancel, Vec::new()).unwrap();
        }
        crate::plugin::pump(&mut fixture.state);
        let outcomes = fixture.outcomes();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].state(), ScriptOperationState::Rejected);
        assert_eq!(
            serde_json::to_value(&fixture.state.plugins).unwrap(),
            before
        );
        assert_eq!(
            inventories::load(&fixture.state, "demo", &fixture.endpoint)
                .unwrap()
                .slots,
            inventory.slots
        );
        assert!(fixture.state.plugin_block_writes.is_empty());
        assert!(
            fixture
                .state
                .plugins
                .operation_receipt("demo", "advance")
                .is_none()
        );
    }
}

#[test]
fn server_owned_build_rechecks_blocks_material_custody_and_source_registration_before_commit() {
    use mc_script::precommit::{HookContext, HookDecision};
    for changed in ["footprint", "inventory", "source", "roster"] {
        let mut fixture = StructureBuildFixture::new(true);
        fixture.submit(fixture.request.clone());
        crate::plugin::pump(&mut fixture.state);
        let question = fixture.question();
        match changed {
            "footprint" => {
                let HookContext::Build(context) = question.context() else {
                    unreachable!()
                };
                let edit = context.edits()[0];
                fixture
                    .state
                    .world
                    .set_block(edit.x, edit.y, edit.z, edit.proposed_state);
            }
            "inventory" => {
                let ScriptInventoryEndpoint::Container { position, .. } = &fixture.endpoint else {
                    unreachable!()
                };
                fixture
                    .state
                    .block_entities
                    .get_mut(&(position.x, position.y, position.z))
                    .unwrap()
                    .slots[0]
                    .count -= 1;
            }
            "source" => fixture.host.unregister_plugin_routes("demo"),
            "roster" => fixture.boundary.set_precommit_hooks(Vec::new()).unwrap(),
            _ => unreachable!(),
        }
        let before = serde_json::to_value(&fixture.state.plugins).unwrap();
        question.answer(HookDecision::Keep, Vec::new()).unwrap();
        crate::plugin::pump(&mut fixture.state);
        assert_eq!(
            fixture.outcomes()[0].failure(),
            Some(Failure::StaleRevision),
            "{changed}"
        );
        assert_eq!(
            serde_json::to_value(&fixture.state.plugins).unwrap(),
            before,
            "{changed}"
        );
        assert!(fixture.state.plugin_block_writes.is_empty(), "{changed}");
    }
}

#[test]
fn server_owned_build_rechecks_hook_storage_guards_in_the_handlers_namespace() {
    use mc_script::precommit::{HookDecision, HookStorageVersionGuards};
    let mut fixture = StructureBuildFixture::new(true);
    fixture.state.plugins.apply_storage(
        "protector",
        &[ScriptStorageMutation::compare_and_swap("claim", None, "allowed").unwrap()],
    );
    let version = fixture.state.plugins.version("protector", "claim").unwrap();
    fixture.submit(fixture.request.clone());
    crate::plugin::pump(&mut fixture.state);
    fixture
        .question()
        .answer(
            HookDecision::Keep,
            vec![HookStorageVersionGuards::new(
                "protector".into(),
                vec![ScriptPluginStorageVersionGuard::new(
                    "claim".into(),
                    version,
                )],
            )],
        )
        .unwrap();
    fixture.state.plugins.apply_storage(
        "protector",
        &[ScriptStorageMutation::compare_and_swap("claim", Some(version), "denied").unwrap()],
    );
    let before = serde_json::to_value(&fixture.state.plugins).unwrap();
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(
        fixture.outcomes()[0].failure(),
        Some(Failure::StaleRevision)
    );
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        before
    );
    assert!(fixture.state.plugin_block_writes.is_empty());
}

#[test]
fn pending_server_owned_operation_conflicts_do_not_replace_the_original_build() {
    use mc_script::precommit::HookDecision;
    let mut fixture = StructureBuildFixture::new(true);
    fixture.submit(fixture.request.clone());
    let mut conflicting = fixture.request.operation().clone();
    let ScriptOperation::WorldStructure {
        operation: ScriptWorldStructureOperation::AdvanceStructure { work_units, .. },
    } = &mut conflicting
    else {
        unreachable!()
    };
    *work_units = 1;
    fixture.submit(ScriptOperationRequest::try_new("conflicting-request", conflicting).unwrap());
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(
        fixture.outcomes()[0].failure(),
        Some(Failure::OperationConflict)
    );
    fixture
        .question()
        .answer(HookDecision::Keep, Vec::new())
        .unwrap();
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(
        fixture.outcomes()[1].state(),
        ScriptOperationState::Committed
    );
    assert_eq!(fixture.state.plugin_block_writes.len(), 2);
}

#[test]
fn hook_free_server_owned_construction_retains_its_immediate_native_path() {
    let mut fixture = StructureBuildFixture::new(false);
    fixture.submit(fixture.request.clone());
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(
        fixture.outcomes()[0].state(),
        ScriptOperationState::Committed
    );
    assert_eq!(fixture.state.plugin_block_writes.len(), 2);
}

#[test]
fn pending_server_owned_retry_state_is_bounded_and_overflow_fails_closed() {
    use mc_script::precommit::{HookDecision, MAX_PRECOMMIT_REQUESTS};
    let mut fixture = StructureBuildFixture::new(true);
    let before = serde_json::to_value(&fixture.state.plugins).unwrap();
    for index in 0..=MAX_PRECOMMIT_REQUESTS {
        fixture.submit(
            ScriptOperationRequest::try_new(
                format!("retry-{index}"),
                fixture.request.operation().clone(),
            )
            .unwrap(),
        );
    }
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(fixture.outcomes().len(), 1);
    assert_eq!(fixture.outcomes()[0].failure(), Some(Failure::Busy));
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        before
    );
    fixture
        .question()
        .answer(HookDecision::Cancel, Vec::new())
        .unwrap();
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(fixture.outcomes().len(), MAX_PRECOMMIT_REQUESTS + 1);
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        before
    );
    assert!(fixture.state.plugin_block_writes.is_empty());
}

#[test]
fn server_owned_build_rechecks_the_callers_original_storage_preconditions() {
    use mc_script::precommit::HookDecision;
    let mut fixture = StructureBuildFixture::new(true);
    fixture.state.plugins.apply_storage(
        "demo",
        &[ScriptStorageMutation::compare_and_swap("budget", None, "2").unwrap()],
    );
    let version = fixture.state.plugins.version("demo", "budget").unwrap();
    fixture.request = fixture
        .request
        .clone()
        .with_expected_plugin_storage_versions(vec![ScriptPluginStorageVersionGuard::new(
            "budget".into(),
            version,
        )])
        .unwrap();
    fixture.submit(fixture.request.clone());
    crate::plugin::pump(&mut fixture.state);
    fixture
        .question()
        .answer(HookDecision::Keep, Vec::new())
        .unwrap();
    fixture.state.plugins.apply_storage(
        "demo",
        &[ScriptStorageMutation::compare_and_swap("budget", Some(version), "1").unwrap()],
    );
    let before = serde_json::to_value(&fixture.state.plugins).unwrap();
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(
        fixture.outcomes()[0].failure(),
        Some(Failure::StaleRevision)
    );
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        before
    );
    assert!(fixture.state.plugin_block_writes.is_empty());
}

#[test]
fn initial_native_build_rejection_keeps_its_queryable_exact_once_operation_receipt() {
    let mut fixture = StructureBuildFixture::new(true);
    let mut invalid = fixture.request.operation().clone();
    let ScriptOperation::WorldStructure {
        operation:
            ScriptWorldStructureOperation::AdvanceStructure {
                expected_revision, ..
            },
    } = &mut invalid
    else {
        unreachable!()
    };
    *expected_revision = 999;
    fixture.request = ScriptOperationRequest::try_new("stale-request", invalid).unwrap();
    fixture.submit(fixture.request.clone());
    crate::plugin::pump(&mut fixture.state);
    let rejected = fixture.outcomes()[0].clone();
    assert_eq!(rejected.failure(), Some(Failure::StaleRevision));
    assert_eq!(
        fixture.state.plugins.operation_receipt("demo", "advance"),
        Some(&rejected)
    );
    let status = ScriptOperationRequest::try_new(
        "status",
        ScriptOperation::Status {
            operation_id: "advance".into(),
        },
    )
    .unwrap();
    assert_eq!(
        crate::plugin::operation(&mut fixture.state, "demo", &status),
        rejected
    );
    let before = serde_json::to_value(&fixture.state.plugins).unwrap();
    assert_eq!(
        crate::plugin::operation(&mut fixture.state, "demo", &fixture.request),
        rejected
    );
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        before
    );
    assert_eq!(fixture.boundary.event_queue_depth(), 0);
    assert!(fixture.state.plugin_block_writes.is_empty());
}

fn assign_construct_work(
    fixture: &mut StructureBuildFixture,
) -> (String, crate::entities::EntitySlot) {
    let at = anchor(&fixture.state);
    let spawn = ScriptOperationRequest::try_new(
        "worker-spawn",
        ScriptOperation::Npc {
            operation: ScriptNpcOperation::Spawn {
                operation_id: "worker-spawn".into(),
                generation_id: "native-constructor".into(),
                position: ScriptPosition::try_new(
                    f64::from(at[0] + 6) + 0.5,
                    200.0,
                    f64::from(at[2]) + 0.5,
                )
                .unwrap(),
                entity_type: "minecraft:villager".into(),
            },
        },
    )
    .unwrap();
    let spawned = crate::plugin_npcs::execute(&mut fixture.state, "demo", &spawn);
    let ScriptOperationPayload::Npc { result } = spawned.payload() else {
        panic!("native worker spawn: {spawned:?}");
    };
    let ScriptNpcResult::Snapshot { npc } = &**result else {
        unreachable!()
    };
    let handle = npc.handle.clone();
    let ScriptOperation::WorldStructure {
        operation:
            ScriptWorldStructureOperation::AdvanceStructure {
                structure_id,
                stage,
                expected_revision,
                ..
            },
    } = fixture.request.operation()
    else {
        unreachable!()
    };
    let assign = ScriptOperationRequest::try_new(
        "assign-construction",
        ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::AssignWork {
                operation_id: "construct".into(),
                handle: handle.clone(),
                work: ScriptNpcWorkOrder::Construct {
                    structure_id: structure_id.clone(),
                    stage: stage.clone(),
                    expected_revision: *expected_revision,
                },
                work_units: 2,
                expected_revision: 0,
            },
        },
    )
    .unwrap();
    let assigned = crate::plugin_npcs::execute(&mut fixture.state, "demo", &assign);
    assert_eq!(assigned.failure(), None, "{assigned:?}");
    let slot = crate::plugin_npcs::owned_slot(&fixture.state, "demo", &handle).unwrap();
    let controls = crate::plugin_npcs::npc_controls(&fixture.state, slot).unwrap();
    fixture.state.tick = controls.work.unwrap().due_tick.unwrap();
    (handle, slot)
}

#[test]
fn npc_construct_waits_for_the_canonical_hook_then_counts_one_real_committed_portion() {
    use mc_script::precommit::{HookActor, HookContext, HookDecision};
    let mut fixture = StructureBuildFixture::new(true);
    let (_, slot) = assign_construct_work(&mut fixture);
    let before = serde_json::to_value(&fixture.state.plugins).unwrap();
    crate::plugin_npcs::advance(&mut fixture.state);
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        before
    );
    assert!(fixture.state.plugin_block_writes.is_empty());
    let question = fixture.question();
    let HookContext::Build(context) = question.context() else {
        unreachable!()
    };
    assert_eq!(context.actor(), &HookActor::Plugin("demo".into()));
    assert_eq!(context.edits().len(), 1);
    crate::plugin_npcs::advance(&mut fixture.state);
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        before
    );
    assert_eq!(
        fixture.boundary.event_queue_depth(),
        0,
        "no duplicate question while pending"
    );
    question.answer(HookDecision::Keep, Vec::new()).unwrap();
    crate::plugin_npcs::advance(&mut fixture.state);
    let assignment = crate::plugin_npcs::npc_controls(&fixture.state, slot)
        .unwrap()
        .work
        .unwrap();
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(assignment.changes.len(), 1);
    assert_eq!(assignment.changes[0].delta, -1);
    assert_eq!(fixture.state.plugin_block_writes.len(), 1);
    let local = ledger(&fixture.state).unwrap();
    let structure = local.structures.get("saved-house").unwrap();
    assert_eq!(structure.receipts.len(), 1);
    assert_eq!(structure.snapshot.completed_work_units, 1);
    assert_eq!(
        inventories::load(&fixture.state, "demo", &fixture.endpoint)
            .unwrap()
            .slots[&0]
            .count,
        1
    );
    crate::plugin_npcs::advance(&mut fixture.state);
    assert_eq!(fixture.state.plugin_block_writes.len(), 1);
    assert_eq!(
        crate::plugin_npcs::npc_controls(&fixture.state, slot)
            .unwrap()
            .work
            .unwrap()
            .work_units_done,
        1
    );
}

#[test]
fn npc_hook_denial_and_stale_guards_do_not_mint_work_receipts_or_debit_custody() {
    use mc_script::precommit::{HookDecision, HookFailure, HookStorageVersionGuards};
    for denied in ["cancel", "expired", "guard", "source"] {
        let mut fixture = StructureBuildFixture::new(true);
        let (_, slot) = assign_construct_work(&mut fixture);
        fixture.state.plugins.apply_storage(
            "protector",
            &[ScriptStorageMutation::compare_and_swap("claim", None, "allowed").unwrap()],
        );
        let version = fixture.state.plugins.version("protector", "claim").unwrap();
        let structure = fixture
            .state
            .plugins
            .get_domain(OWNER, KEY)
            .unwrap()
            .clone();
        let inventory = inventories::load(&fixture.state, "demo", &fixture.endpoint).unwrap();
        crate::plugin_npcs::advance(&mut fixture.state);
        let question = fixture.question();
        match denied {
            "cancel" => question.answer(HookDecision::Cancel, Vec::new()).unwrap(),
            "expired" => question.fail(HookFailure::Expired).unwrap(),
            "source" => {
                fixture.host.unregister_plugin_routes("demo");
                question.answer(HookDecision::Keep, Vec::new()).unwrap();
            }
            "guard" => {
                question
                    .answer(
                        HookDecision::Keep,
                        vec![HookStorageVersionGuards::new(
                            "protector".into(),
                            vec![ScriptPluginStorageVersionGuard::new(
                                "claim".into(),
                                version,
                            )],
                        )],
                    )
                    .unwrap();
                fixture.state.plugins.apply_storage(
                    "protector",
                    &[
                        ScriptStorageMutation::compare_and_swap("claim", Some(version), "denied")
                            .unwrap(),
                    ],
                );
            }
            _ => unreachable!(),
        }
        crate::plugin_npcs::advance(&mut fixture.state);
        let work = crate::plugin_npcs::npc_controls(&fixture.state, slot)
            .unwrap()
            .work
            .unwrap();
        assert_eq!(work.work_units_done, 0, "{denied}");
        assert!(work.changes.is_empty(), "{denied}");
        assert!(work.receipts.is_empty(), "{denied}");
        assert_eq!(work.state, ScriptWorkState::Paused, "{denied}");
        assert_eq!(
            fixture.state.plugins.get_domain(OWNER, KEY),
            Some(&structure),
            "{denied}"
        );
        assert_eq!(
            inventories::load(&fixture.state, "demo", &fixture.endpoint)
                .unwrap()
                .slots,
            inventory.slots,
            "{denied}"
        );
        assert!(fixture.state.plugin_block_writes.is_empty(), "{denied}");
    }
}

#[test]
fn canceled_npc_construction_abandons_its_pending_hook_without_spending_an_answer() {
    use mc_script::precommit::{HookAnswerError, HookDecision};
    let mut fixture = StructureBuildFixture::new(true);
    let (handle, slot) = assign_construct_work(&mut fixture);
    crate::plugin_npcs::advance(&mut fixture.state);
    let question = fixture.question();
    let controls = crate::plugin_npcs::npc_controls(&fixture.state, slot).unwrap();
    let cancel = ScriptOperationRequest::try_new(
        "cancel-construction",
        ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::CancelWork {
                operation_id: "cancel-construction".into(),
                handle,
                expected_revision: controls.revision,
            },
        },
    )
    .unwrap();
    assert_eq!(
        crate::plugin_npcs::execute(&mut fixture.state, "demo", &cancel).failure(),
        None
    );
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(
        question.answer(HookDecision::Keep, Vec::new()),
        Err(HookAnswerError::Abandoned)
    );
    crate::plugin_npcs::advance(&mut fixture.state);
    let work = crate::plugin_npcs::npc_controls(&fixture.state, slot)
        .unwrap()
        .work
        .unwrap();
    assert_eq!(work.state, ScriptWorkState::Cancelled);
    assert_eq!(work.work_units_done, 0);
    assert!(work.receipts.is_empty());
    assert!(fixture.state.plugin_block_writes.is_empty());
}

#[test]
fn hook_free_npc_construct_still_commits_at_its_native_work_deadline() {
    let mut fixture = StructureBuildFixture::new(false);
    let (_, slot) = assign_construct_work(&mut fixture);
    crate::plugin_npcs::advance(&mut fixture.state);
    assert_eq!(
        crate::plugin_npcs::npc_controls(&fixture.state, slot)
            .unwrap()
            .work
            .unwrap()
            .work_units_done,
        1
    );
    assert_eq!(fixture.state.plugin_block_writes.len(), 1);
}

#[test]
fn npc_construct_revalidates_its_frozen_footprint_and_material_before_images() {
    use mc_script::precommit::{HookContext, HookDecision};
    for changed in ["footprint", "materials"] {
        let mut fixture = StructureBuildFixture::new(true);
        let (_, slot) = assign_construct_work(&mut fixture);
        let structure_before = fixture
            .state
            .plugins
            .get_domain(OWNER, KEY)
            .unwrap()
            .clone();
        crate::plugin_npcs::advance(&mut fixture.state);
        let question = fixture.question();
        if changed == "footprint" {
            let HookContext::Build(context) = question.context() else {
                unreachable!()
            };
            let edit = context.edits()[0];
            fixture
                .state
                .world
                .set_block(edit.x, edit.y, edit.z, edit.proposed_state);
        } else {
            let ScriptInventoryEndpoint::Container { position, .. } = &fixture.endpoint else {
                unreachable!()
            };
            fixture
                .state
                .block_entities
                .get_mut(&(position.x, position.y, position.z))
                .unwrap()
                .slots[0]
                .count = 1;
        }
        let inventory_before =
            inventories::load(&fixture.state, "demo", &fixture.endpoint).unwrap();
        question.answer(HookDecision::Keep, Vec::new()).unwrap();
        crate::plugin_npcs::advance(&mut fixture.state);
        let assignment = crate::plugin_npcs::npc_controls(&fixture.state, slot)
            .unwrap()
            .work
            .unwrap();
        assert_eq!(assignment.work_units_done, 0, "{changed}");
        assert!(assignment.changes.is_empty(), "{changed}");
        assert!(assignment.receipts.is_empty(), "{changed}");
        assert_eq!(
            fixture.state.plugins.get_domain(OWNER, KEY),
            Some(&structure_before),
            "{changed}"
        );
        assert_eq!(
            inventories::load(&fixture.state, "demo", &fixture.endpoint)
                .unwrap()
                .slots,
            inventory_before.slots,
            "{changed}"
        );
        assert!(fixture.state.plugin_block_writes.is_empty(), "{changed}");
    }
}

#[test]
fn npc_construction_shares_the_programmatic_pending_build_capacity_bound() {
    let mut fixture = StructureBuildFixture::new(true);
    let (_, slot) = assign_construct_work(&mut fixture);
    for index in 0..mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
        fixture.submit(
            ScriptOperationRequest::try_new(
                format!("pending-{index}"),
                fixture.request.operation().clone(),
            )
            .unwrap(),
        );
    }
    crate::plugin::pump(&mut fixture.state);
    crate::plugin_npcs::advance(&mut fixture.state);
    let assignment = crate::plugin_npcs::npc_controls(&fixture.state, slot)
        .unwrap()
        .work
        .unwrap();
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::Interrupted));
    assert!(assignment.changes.is_empty());
    assert!(assignment.receipts.is_empty());
    assert_eq!(
        fixture.boundary.event_queue_depth(),
        1,
        "only the coalesced guest question exists"
    );
    assert!(fixture.state.plugin_block_writes.is_empty());
}

#[test]
fn district_requires_committed_matching_structure_and_loses_destroyed_readiness() {
    use crate::settlements::districts::{DistrictPlan, observe, plan};
    let mut fixture = StructureBuildFixture::new(false);
    let at = anchor(&fixture.state);
    let stone = fixture
        .state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    for x in at[0]..=at[0] + 7 {
        for z in at[2]..=at[2] + 3 {
            fixture.state.world.set_block(x, at[1] - 1, z, stone);
        }
    }
    let id = fixture
        .state
        .settlements
        .create(
            &mut fixture.state.settlement_sequence,
            "structure-district".into(),
            "demo".into(),
            at,
            32,
        )
        .unwrap();
    let structure = |purpose: &str| {
        ScriptSettlementBuildingRef::new("structure".into(), "saved-house".into(), purpose.into())
    };
    let district = |buildings| DistrictPlan {
        owner: "demo".into(),
        branch: "none".into(),
        level: 0,
        buildings,
    };
    assert_eq!(
        plan(
            &mut fixture.state,
            id,
            1,
            district(vec![structure("authored:house")]),
        ),
        Err(Failure::Blocked)
    );
    fixture.submit(fixture.request.clone());
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(
        fixture.outcomes()[0].state(),
        ScriptOperationState::Committed
    );
    assert_eq!(
        ledger(&fixture.state).unwrap().structures["saved-house"]
            .snapshot
            .state,
        ScriptStructureState::Committed
    );
    assert_eq!(
        plan(
            &mut fixture.state,
            id,
            1,
            district(vec![structure("solaris:farm")]),
        ),
        Err(Failure::InvalidRequest)
    );
    let chest = [at[0] + 4, at[1], at[2]];
    let bread = fixture
        .state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:bread").unwrap())
        .unwrap();
    fixture
        .state
        .block_entities
        .get_mut(&(chest[0], chest[1], chest[2]))
        .unwrap()
        .slots[0] = mc_data::ItemStack::new(bread, 1);
    fixture.state.world.fold_overlay();
    let footprint = ScriptWorldBounds::new([at[0] + 3, at[1], at[2]], chest).unwrap();
    let (_, result) = survey(&mut fixture.state, "demo", "minecraft:overworld", footprint).unwrap();
    let ScriptWorldStructureResult::Survey { survey } = result else {
        panic!("native survey")
    };
    let mut buildings = vec![structure("authored:house")];
    // Distinct certificates referring to one physical chest cannot double food.
    for operation_id in ["store-one", "store-two"] {
        let (_, result) = execute_operation(
            &mut fixture.state,
            "demo",
            &ScriptWorldStructureOperation::RegisterWorksite {
                operation_id: operation_id.into(),
                request: Box::new(ScriptWorksiteRequest::new(
                    footprint,
                    vec![
                        ScriptWorksiteInteractionPoint::Entrance {
                            at: [at[0] + 3, at[1], at[2]],
                        },
                        ScriptWorksiteInteractionPoint::Container { at: chest },
                    ],
                    survey.survey_token.clone(),
                    survey.world_revision,
                )),
            },
        )
        .unwrap();
        let ScriptWorldStructureResult::Worksite { worksite } = result else {
            panic!("native warehouse")
        };
        buildings.push(ScriptSettlementBuildingRef::new(
            "worksite".into(),
            worksite.worksite_id,
            "solaris:warehouse".into(),
        ));
    }
    let foot = [at[0] + 6, at[1], at[2] + 1];
    let head = BlockPos {
        x: foot[0] + 1,
        y: foot[1],
        z: foot[2],
    };
    let bed = mc_data::Identifier::parse("minecraft:red_bed").unwrap();
    for (position, part) in [(foot, "foot"), ([head.x, head.y, head.z], "head")] {
        let default = fixture.state.registry.blocks.block(&bed).unwrap().default;
        let mut properties = fixture
            .state
            .registry
            .blocks
            .by_id(default)
            .unwrap()
            .properties
            .clone();
        for (key, value) in &mut properties {
            match key.as_str() {
                "part" => *value = part.into(),
                "facing" => *value = "east".into(),
                _ => {}
            }
        }
        let state = fixture
            .state
            .registry
            .blocks
            .by_name_and_props(&bed, &properties)
            .unwrap();
        fixture
            .state
            .world
            .set_block(position[0], position[1], position[2], state.0);
    }
    fixture.state.world.fold_overlay();
    let kind = fixture
        .state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let entity = fixture.state.spawn_mob(
        kind,
        [f64::from(at[0] + 6), f64::from(at[1]), f64::from(at[2] + 3)],
        20.0,
    );
    let slot = fixture.state.entities.slot_of(entity).unwrap();
    fixture.state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState {
            home: Some(head),
            home_dimension: "minecraft:overworld".into(),
            ..Default::default()
        }));
    crate::settlements::bind(&mut fixture.state, id, entity).unwrap();
    plan(&mut fixture.state, id, 1, district(buildings)).unwrap();
    let record = fixture.state.settlements.get(id).unwrap().clone();
    let ready = observe(
        &mut fixture.state,
        record.id,
        record.anchor,
        record.radius,
        record.district.as_ref(),
    )
    .unwrap();
    assert_eq!(ready.stage.as_deref(), Some("hamlet"));
    assert_eq!((ready.population, ready.housed, ready.food), (1, 1, 1));
    assert_eq!(ready.working, ["authored:house", "solaris:warehouse"]);
    let air = fixture.state.registry.states.air;
    fixture
        .state
        .world
        .set_block(at[0], at[1] - 1, at[2] + 1, air);
    fixture.state.world.fold_overlay();
    let destroyed = observe(
        &mut fixture.state,
        record.id,
        record.anchor,
        record.radius,
        record.district.as_ref(),
    )
    .unwrap();
    assert!(destroyed.stage.is_none());
    assert_eq!(
        destroyed.reason.as_deref(),
        Some("Native district building unavailable: Blocked.")
    );
}
