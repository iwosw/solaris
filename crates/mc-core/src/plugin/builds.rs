//! Plugin structure/NPC/world block-set builds: admission, polling, apply.
//! Moved verbatim from `plugin.rs`.

use super::*;

/// The tick only checks delivered answers; guest work stays on the host thread.
pub(super) fn poll_structure_builds(state: &mut TickState) {
    let mut pending = std::mem::take(
        &mut state
            .plugin_runtime
            .as_mut()
            .expect("runtime retained")
            .pending_structure_builds,
    );
    for _ in 0..pending.len() {
        let mut build = pending.pop_front().expect("bounded pending queue");
        let Some(answer) = build.decision.try_resolve() else {
            pending.push_back(build);
            continue;
        };
        let original = &build.commands[0];
        let mc_script::ScriptCommand::Operation { request } = original.request() else {
            unreachable!("pending operation");
        };
        let owner = original.plugin_id();
        let outcome = if let Some(outcome) = operation_precheck(state, owner, request) {
            outcome
        } else {
            match answer {
                Err(failure) => ScriptOperationOutcome::rejected(hook_operation_failure(failure)),
                Ok(mut approval) => {
                    match crate::plugin_structures::prepare_build(state, owner, request) {
                        Ok(Some(current)) if current == build.plan => {
                            match crate::build_precommit::consume_approval(state, &mut approval) {
                                Ok(()) => operation(state, owner, request),
                                Err(failure) => ScriptOperationOutcome::rejected(
                                    hook_operation_failure(failure),
                                ),
                            }
                        }
                        _ => {
                            ScriptOperationOutcome::rejected(ScriptOperationFailure::StaleRevision)
                        }
                    }
                }
            }
        };
        let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
        runtime.pending_build_commands -= build.commands.len();
        let outcome_state = outcome.state().as_str();
        let mut outcome = Some(outcome);
        let mut commands = build.commands.into_iter();
        while let Some(admitted) = commands.next() {
            let owner = admitted.plugin_id().to_owned();
            let (session, request) = command_identity(admitted.request());
            let result = if commands.len() == 0 {
                outcome.take().expect("last operation result")
            } else {
                outcome.as_ref().expect("shared operation result").clone()
            };
            runtime.pending_traces.push(ExecutionTrace {
                owner,
                session,
                request,
                outcome: outcome_state,
            });
            if let Ok(event) = admitted.operation_result(result) {
                runtime.pending_events.push(event);
            }
        }
    }
    state
        .plugin_runtime
        .as_mut()
        .expect("runtime retained")
        .pending_structure_builds = pending;
}

/// NPC work calls this only after its ordinary live work prerequisites passed.
/// `false` leaves progress unchanged while the host owns the queued question.
pub(crate) fn admit_npc_structure_build(
    state: &mut TickState,
    owner: &str,
    handle: &str,
    work_revision: u64,
    entity_uuid: uuid::Uuid,
    operation: &mc_script::ScriptWorldStructureOperation,
) -> Result<bool, ScriptOperationFailure> {
    let Some(runtime) = state.plugin_runtime.as_mut() else {
        return Ok(true);
    };
    if let Some(index) = runtime
        .pending_npc_builds
        .iter()
        .position(|pending| pending.owner == owner && pending.handle == handle)
    {
        let mut pending = runtime.pending_npc_builds.swap_remove(index);
        if pending.work_revision != work_revision
            || pending.entity_uuid != entity_uuid
            || pending.dimension != state.dimension
            || !matches!(&pending.plan, NpcBuildPlan::Structure { operation: original, .. } if original == operation)
        {
            runtime.pending_build_commands -= 1;
            return Err(ScriptOperationFailure::StaleRevision);
        }
        let Some(answer) = pending.decision.try_resolve() else {
            runtime.pending_npc_builds.push(pending);
            return Ok(false);
        };
        runtime.pending_build_commands -= 1;
        let NpcBuildPlan::Structure { plan, .. } = pending.plan else {
            unreachable!("structure identity checked above");
        };
        let mut approval = answer.map_err(hook_operation_failure)?;
        if crate::plugin_structures::prepare_structure_build(state, owner, operation)?
            .is_none_or(|current| current != plan)
        {
            return Err(ScriptOperationFailure::StaleRevision);
        }
        crate::build_precommit::consume_approval(state, &mut approval)
            .map_err(hook_operation_failure)?;
        return Ok(true);
    }
    if !runtime
        .boundary
        .has_precommit_hooks(mc_script::precommit::HookKind::Build)
    {
        return Ok(true);
    }
    if runtime.pending_build_commands >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
        return Err(ScriptOperationFailure::Busy);
    }
    let Some(plan) = crate::plugin_structures::prepare_structure_build(state, owner, operation)?
    else {
        return Ok(true);
    };
    let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
    let decision = runtime
        .boundary
        .begin_precommit(plan.context.clone())
        .map_err(hook_operation_failure)?;
    runtime.pending_npc_builds.push(PendingNpcBuild {
        owner: owner.into(),
        handle: handle.into(),
        work_revision,
        entity_uuid,
        dimension: state.dimension,
        plan: NpcBuildPlan::Structure {
            operation: operation.clone(),
            plan,
        },
        decision,
    });
    runtime.pending_build_commands += 1;
    Ok(false)
}

pub(crate) fn poll_npc_block_build(
    state: &mut TickState,
    owner: &str,
    handle: &str,
    work_revision: u64,
    entity_uuid: uuid::Uuid,
    work: &mc_script::ScriptNpcWorkOrder,
) -> Result<NpcBlockBuild, ScriptOperationFailure> {
    let Some(runtime) = state.plugin_runtime.as_mut() else {
        return Ok(NpcBlockBuild::Direct);
    };
    if let Some(index) = runtime
        .pending_npc_builds
        .iter()
        .position(|pending| pending.owner == owner && pending.handle == handle)
    {
        let mut pending = runtime.pending_npc_builds.swap_remove(index);
        if pending.work_revision != work_revision
            || pending.entity_uuid != entity_uuid
            || pending.dimension != state.dimension
            || !matches!(&pending.plan, NpcBuildPlan::Blocks { work: original, .. } if original == work)
        {
            runtime.pending_build_commands -= 1;
            return Err(ScriptOperationFailure::StaleRevision);
        }
        let Some(answer) = pending.decision.try_resolve() else {
            runtime.pending_npc_builds.push(pending);
            return Ok(NpcBlockBuild::Pending);
        };
        runtime.pending_build_commands -= 1;
        let approval = answer.map_err(hook_operation_failure)?;
        let NpcBuildPlan::Blocks { plan, .. } = pending.plan else {
            unreachable!("physical work identity checked above");
        };
        return Ok(NpcBlockBuild::Ready { plan, approval });
    }
    if !runtime
        .boundary
        .has_precommit_hooks(mc_script::precommit::HookKind::Build)
    {
        return Ok(NpcBlockBuild::Direct);
    }
    if runtime.pending_build_commands >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
        return Err(ScriptOperationFailure::Busy);
    }
    Ok(NpcBlockBuild::Prepare)
}

pub(crate) fn begin_npc_block_build(
    state: &mut TickState,
    owner: &str,
    handle: &str,
    work_revision: u64,
    entity_uuid: uuid::Uuid,
    work: &mc_script::ScriptNpcWorkOrder,
    plan: PreparedNpcBlockWork,
) -> Result<(), ScriptOperationFailure> {
    let context = mc_script::precommit::BuildContext::try_new(
        mc_script::precommit::HookActor::Plugin(owner.into()),
        state.dimension.name(),
        vec![plan.edit],
    )
    .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    let runtime = state
        .plugin_runtime
        .as_mut()
        .expect("hooked native work runtime");
    if runtime.pending_build_commands >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
        return Err(ScriptOperationFailure::Busy);
    }
    let decision = runtime
        .boundary
        .begin_precommit(mc_script::precommit::HookContext::Build(context))
        .map_err(hook_operation_failure)?;
    runtime.pending_npc_builds.push(PendingNpcBuild {
        owner: owner.into(),
        handle: handle.into(),
        work_revision,
        entity_uuid,
        dimension: state.dimension,
        plan: NpcBuildPlan::Blocks {
            work: work.clone(),
            plan,
        },
        decision,
    });
    runtime.pending_build_commands += 1;
    Ok(())
}

pub(super) fn retire_inactive_npc_builds(state: &mut TickState) {
    let mut pending = std::mem::take(
        &mut state
            .plugin_runtime
            .as_mut()
            .expect("runtime retained")
            .pending_npc_builds,
    );
    let before = pending.len();
    pending.retain(|build| {
        crate::plugin_npcs::pending_work_is_current(
            state,
            &build.owner,
            &build.handle,
            build.entity_uuid,
            build.work_revision,
            build.dimension,
        )
    });
    let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
    runtime.pending_build_commands -= before - pending.len();
    runtime.pending_npc_builds = pending;
}

pub(super) fn prepare_world_block_set(
    state: &TickState,
    request: &mc_script::ScriptWorldBlockSetRequest,
) -> Result<PreparedWorldBlockSet, mc_script::ScriptWorldBlockSetFailure> {
    use mc_script::ScriptWorldBlockSetFailure as Failure;
    let dimension = crate::dimensions::Dimension::parse(request.dimension())
        .ok_or(Failure::UnsupportedDimension)?;
    let block = mc_data::Identifier::parse(request.block_id())
        .ok()
        .and_then(|id| state.registry.blocks.block(&id))
        .map(|block| block.default.0)
        .ok_or(Failure::UnknownBlock)?;
    let target = if dimension == state.dimension {
        state
    } else {
        state
            .other_dimensions
            .get(&dimension)
            .ok_or(Failure::UnsupportedDimension)?
    };
    if request.y() < target.dimension.geometry().min_y()
        || request.y() >= target.dimension.geometry().max_y()
    {
        return Err(Failure::OutOfWorld);
    }
    let previous = target
        .world
        .block_at(request.x(), request.y(), request.z())
        .ok_or(Failure::Busy)?;
    let position = crate::boundary::BlockPos {
        x: request.x(),
        y: request.y(),
        z: request.z(),
    };
    if crate::plugin_inventories::has_reservations_for_block(target, position) {
        return Err(Failure::Busy);
    }
    Ok(PreparedWorldBlockSet {
        dimension,
        position,
        previous,
        proposed: block,
    })
}

pub(super) fn apply_world_block_set(
    state: &mut TickState,
    plan: &PreparedWorldBlockSet,
    plugin: &str,
) {
    let target = crate::plugin_inventories::world_mut(state, plan.dimension.name())
        .expect("preflight dimension");
    let position = plan.position;
    crate::block_log::note_plugin(target, position, plugin);
    target.stage_command_container_removal(position);
    target
        .world
        .set_block(position.x, position.y, position.z, plan.proposed);
    target.plugin_block_writes.push((position, plan.proposed));
}

pub(super) fn hook_block_set_failure(
    failure: mc_script::precommit::HookFailure,
) -> mc_script::ScriptWorldBlockSetFailure {
    use mc_script::ScriptWorldBlockSetFailure as Failure;
    use mc_script::precommit::HookFailure;
    match failure {
        HookFailure::Cancelled | HookFailure::Stale => Failure::Rejected,
        HookFailure::QueueFull => Failure::Busy,
        _ => Failure::RuntimeUnavailable,
    }
}

pub(super) fn execute_world_block_set(
    state: &mut TickState,
    admitted: mc_script::AdmittedScriptCommand,
) -> Option<mc_script::ScriptEvent> {
    let mc_script::ScriptCommand::SetWorldBlock { request } = admitted.request() else {
        unreachable!("block set dispatch");
    };
    let plan = match prepare_world_block_set(state, request) {
        Ok(plan) => plan,
        Err(failure) => return admitted.world_block_set_result(false, Some(failure)).ok(),
    };
    let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
    if !runtime
        .boundary
        .has_precommit_hooks(mc_script::precommit::HookKind::Build)
    {
        apply_world_block_set(state, &plan, admitted.plugin_id());
        return admitted.world_block_set_result(true, None).ok();
    }
    if runtime.pending_build_commands >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
        return admitted
            .world_block_set_result(false, Some(mc_script::ScriptWorldBlockSetFailure::Busy))
            .ok();
    }
    let context = mc_script::precommit::BuildContext::try_new(
        mc_script::precommit::HookActor::Plugin(admitted.plugin_id().into()),
        plan.dimension.name(),
        vec![mc_script::precommit::BuildEdit::new(
            plan.position.x,
            plan.position.y,
            plan.position.z,
            plan.previous,
            plan.proposed,
        )],
    )
    .expect("one validated native block edit");
    match runtime
        .boundary
        .begin_precommit(mc_script::precommit::HookContext::Build(context))
    {
        Ok(decision) => {
            runtime
                .pending_world_block_sets
                .push_back(PendingWorldBlockSet {
                    command: admitted,
                    plan,
                    decision,
                });
            runtime.pending_build_commands += 1;
            None
        }
        Err(failure) => admitted
            .world_block_set_result(false, Some(hook_block_set_failure(failure)))
            .ok(),
    }
}

pub(super) fn poll_world_block_sets(state: &mut TickState) {
    let mut pending = std::mem::take(
        &mut state
            .plugin_runtime
            .as_mut()
            .expect("runtime retained")
            .pending_world_block_sets,
    );
    for _ in 0..pending.len() {
        let mut build = pending.pop_front().expect("bounded pending queue");
        let Some(answer) = build.decision.try_resolve() else {
            pending.push_back(build);
            continue;
        };
        let owner = build.command.plugin_id();
        let mc_script::ScriptCommand::SetWorldBlock { request } = build.command.request() else {
            unreachable!("pending block set");
        };
        let failure = match answer {
            Err(failure) => Some(hook_block_set_failure(failure)),
            Ok(mut approval) => match prepare_world_block_set(state, request) {
                Ok(current) if current == build.plan => {
                    match crate::build_precommit::consume_approval(state, &mut approval) {
                        Ok(()) => {
                            apply_world_block_set(state, &build.plan, owner);
                            None
                        }
                        Err(failure) => Some(hook_block_set_failure(failure)),
                    }
                }
                _ => Some(mc_script::ScriptWorldBlockSetFailure::Rejected),
            },
        };
        let owner = owner.to_owned();
        let (session, request) = command_identity(build.command.request());
        let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
        runtime.pending_build_commands -= 1;
        runtime.pending_traces.push(ExecutionTrace {
            owner,
            session,
            request,
            outcome: if failure.is_none() {
                "committed"
            } else {
                "rejected"
            },
        });
        if let Ok(event) = build
            .command
            .world_block_set_result(failure.is_none(), failure)
        {
            runtime.pending_events.push(event);
        }
    }
    state
        .plugin_runtime
        .as_mut()
        .expect("runtime retained")
        .pending_world_block_sets = pending;
}
