//! Bounded client build admission: native read-only planning, an off-tick hook,
//! then one synchronous native commit of the frozen plan.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use parking_lot::Mutex;

use mc_script::precommit::{
    Approval, BuildContext, BuildEdit, HookActor, HookContext, HookDecision, HookFailure, HookKind,
    HookPlayer, MAX_PRECOMMIT_REQUESTS, PRECOMMIT_DEADLINE,
};
use tokio::sync::oneshot;

use crate::boundary::{BlockPos, ConnectionId, DigAction, Hand, PlayerChange, PlayerInput};
use crate::intent::Intent;
use crate::tick::TickState;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
enum CorrectionTarget {
    Block {
        position: BlockPos,
        face: Option<u8>,
    },
    Ray {
        hand: Hand,
        yaw: f32,
        pitch: f32,
    },
}

/// Click-time planning geometry is stable while live reach, occupancy,
/// support, protection and custody are checked again at native merge.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PlanningPose {
    Placement(crate::placement::Click),
    Ray { eye: [f64; 3] },
}

/// A prediction correction, not a mutation. Its cells never enter the tick WAL.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BuildResync {
    connection: ConnectionId,
    sequence: i32,
    target: CorrectionTarget,
    additional: Vec<BlockPos>,
}

impl BuildResync {
    fn for_input(input: &PlayerInput) -> Option<Self> {
        let (sequence, target) = match input {
            PlayerInput::Dig {
                position,
                action,
                sequence,
                ..
            } if *action != DigAction::Cancel => (
                *sequence,
                CorrectionTarget::Block {
                    position: *position,
                    face: None,
                },
            ),
            PlayerInput::UseItemOn {
                position,
                face,
                sequence,
                ..
            } => (
                *sequence,
                CorrectionTarget::Block {
                    position: *position,
                    face: Some(*face),
                },
            ),
            PlayerInput::UseItem {
                hand,
                yaw,
                pitch,
                sequence,
                ..
            } => (
                *sequence,
                CorrectionTarget::Ray {
                    hand: *hand,
                    yaw: *yaw,
                    pitch: *pitch,
                },
            ),
            _ => return None,
        };
        Some(Self {
            connection: input.connection(),
            sequence,
            target,
            additional: Vec::new(),
        })
    }

    pub(crate) fn connection(&self) -> ConnectionId {
        self.connection
    }
}

/// A fail-closed decision together with the ordinary client prediction repair.
#[derive(Debug)]
pub struct BuildRefusal {
    pub reason: HookFailure,
    pub(crate) correction: BuildResync,
}

pub(crate) struct PreviewQueries {
    receiver: mpsc::Receiver<PreviewQuery>,
    pending: Option<PreviewQuery>,
}

struct PreviewQuery {
    input: PlayerInput,
    fence: Option<PreviewFence>,
    reply: oneshot::Sender<PreviewAnswer>,
}

#[derive(Default)]
struct PreviewFrontier {
    issued: AtomicU64,
    applied: AtomicU64,
}

/// Native-input FIFO barrier; it neither mutates the world nor advances a tick.
#[derive(Clone)]
pub struct PreviewFence {
    connection: ConnectionId,
    sequence: u64,
    frontier: Arc<PreviewFrontier>,
}

impl fmt::Debug for PreviewFence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreviewFence")
            .field("sequence", &self.sequence)
            .finish()
    }
}

impl PartialEq for PreviewFence {
    fn eq(&self, other: &Self) -> bool {
        self.sequence == other.sequence && Arc::ptr_eq(&self.frontier, &other.frontier)
    }
}

impl PreviewFence {
    pub(crate) fn connection(&self) -> ConnectionId {
        self.connection
    }

    pub(crate) fn apply(&self) {
        self.frontier
            .applied
            .fetch_max(self.sequence, Ordering::SeqCst);
    }

    fn ready(&self) -> bool {
        self.frontier.applied.load(Ordering::SeqCst) >= self.sequence
    }
}

// The bounded oneshot already owns this storage; boxing the plan adds a
// throwaway allocation before transferring it into the single-use approval.
#[allow(clippy::large_enum_variant)]
enum PreviewAnswer {
    Direct(PlayerInput),
    Prepared {
        plan: PreparedBuild,
        context: HookContext,
    },
    Refused(BuildRefusal),
}

/// The edge owns this facade; the receiver belongs exclusively to the tick.
pub(crate) struct Gateway {
    boundary: Option<mc_script::ScriptBoundary>,
    queries: Option<mpsc::SyncSender<PreviewQuery>>,
    tick_thread: Option<std::thread::Thread>,
    input_tx: Option<tokio::sync::mpsc::Sender<PlayerInput>>,
    frontier: Option<Arc<PreviewFrontier>>,
}

impl fmt::Debug for Gateway {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BuildGateway")
            .field("enabled", &self.boundary.is_some())
            .finish()
    }
}

impl Gateway {
    pub(crate) fn new(
        boundary: Option<mc_script::ScriptBoundary>,
    ) -> (Self, Option<PreviewQueries>) {
        let boundary = boundary.filter(|boundary| boundary.has_precommit_hooks(HookKind::Build));
        if boundary.is_none() {
            return (
                Self {
                    boundary,
                    queries: None,
                    tick_thread: None,
                    input_tx: None,
                    frontier: None,
                },
                None,
            );
        }
        let (queries, receiver) = mpsc::sync_channel(MAX_PRECOMMIT_REQUESTS);
        (
            Self {
                boundary,
                queries: Some(queries),
                tick_thread: None,
                input_tx: None,
                frontier: Some(Arc::new(PreviewFrontier::default())),
            },
            Some(PreviewQueries {
                receiver,
                pending: None,
            }),
        )
    }

    pub(crate) fn set_tick_owner(
        &mut self,
        thread: std::thread::Thread,
        input_tx: tokio::sync::mpsc::Sender<PlayerInput>,
    ) {
        if self.queries.is_some() {
            self.tick_thread = Some(thread);
            self.input_tx = Some(input_tx);
        }
    }

    pub(crate) async fn admit(&self, input: PlayerInput) -> Result<PlayerInput, BuildRefusal> {
        if self.boundary.is_none() {
            return Ok(input);
        }
        let Some(correction) = BuildResync::for_input(&input) else {
            return Ok(input);
        };
        self.admit_with_deadline(input, correction, Instant::now() + PRECOMMIT_DEADLINE)
            .await
    }

    async fn admit_with_deadline(
        &self,
        input: PlayerInput,
        correction: BuildResync,
        deadline: Instant,
    ) -> Result<PlayerInput, BuildRefusal> {
        let Some(boundary) = &self.boundary else {
            return Ok(input);
        };
        // One deadline covers native planning, host admission and the handler
        // answer. Later native consumption retains non-clock safety checks.
        let (reply, receiver) = oneshot::channel();
        // Allocate a sequence only after the caller's earlier input sends have
        // completed. Any applied fence >= this sequence covers those sends,
        // even when different clients enqueue their fences out of order.
        let fence = if self.input_tx.is_some() {
            let frontier = self.frontier.as_ref().expect("enabled build frontier");
            let sequence = frontier
                .issued
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| BuildRefusal {
                    reason: HookFailure::Unavailable,
                    correction: correction.clone(),
                })?
                + 1;
            Some(PreviewFence {
                connection: input.connection(),
                sequence,
                frontier: Arc::clone(frontier),
            })
        } else {
            None
        };
        let query = PreviewQuery {
            input,
            fence: fence.clone(),
            reply,
        };
        let Some(queries) = &self.queries else {
            return Err(BuildRefusal {
                reason: HookFailure::Unavailable,
                correction,
            });
        };
        if let Err(error) = queries.try_send(query) {
            let reason = match error {
                mpsc::TrySendError::Full(_) => HookFailure::QueueFull,
                mpsc::TrySendError::Disconnected(_) => HookFailure::Unavailable,
            };
            return Err(BuildRefusal { reason, correction });
        }
        if let (Some(input_tx), Some(fence)) = (&self.input_tx, fence)
            && let Err(error) = input_tx.try_send(PlayerInput::BuildPreviewFence { fence })
        {
            let reason = match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => HookFailure::QueueFull,
                tokio::sync::mpsc::error::TrySendError::Closed(_) => HookFailure::Unavailable,
            };
            return Err(BuildRefusal { reason, correction });
        }
        if let Some(thread) = &self.tick_thread {
            thread.unpark();
        }
        let answer =
            match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), receiver).await
            {
                Ok(Ok(answer)) => answer,
                Ok(Err(_)) => {
                    return Err(BuildRefusal {
                        reason: HookFailure::Unavailable,
                        correction,
                    });
                }
                Err(_) => {
                    return Err(BuildRefusal {
                        reason: HookFailure::Expired,
                        correction,
                    });
                }
            };
        let (prepared, context) = match answer {
            PreviewAnswer::Direct(input) => return Ok(input),
            PreviewAnswer::Prepared { plan, context } => (plan, context),
            PreviewAnswer::Refused(refusal) => return Err(refusal),
        };
        let pending = match boundary.begin_precommit_with_deadline(context, deadline) {
            Ok(pending) => pending,
            Err(reason) => {
                return Err(BuildRefusal {
                    reason,
                    correction: prepared.correction(),
                });
            }
        };
        let approval = match pending.resolve().await {
            Ok(approval) => approval,
            Err(reason) => {
                return Err(BuildRefusal {
                    reason,
                    correction: prepared.correction(),
                });
            }
        };
        if approval.decision() != HookDecision::Keep {
            return Err(BuildRefusal {
                reason: HookFailure::Cancelled,
                correction: prepared.correction(),
            });
        }
        Ok(PlayerInput::AuthorizedBuild {
            authorization: AuthorizedBuild {
                connection: prepared.connection,
                actor: prepared.intent.actor,
                inner: Arc::new(Mutex::new(Some(AdmittedBuild { prepared, approval }))),
            },
        })
    }
}

struct PreparedBuild {
    connection: ConnectionId,
    dimension: crate::dimensions::Dimension,
    uuid: uuid::Uuid,
    game_mode: crate::commands::GameMode,
    planning_tick: u64,
    planning_pose: Option<PlanningPose>,
    held: Option<(Hand, usize, mc_data::ItemStack)>,
    source: PlayerInput,
    intent: Intent,
}

impl PreparedBuild {
    fn correction(&self) -> BuildResync {
        let mut correction = BuildResync::for_input(&self.source).expect("a prepared client build");
        correction.additional = self
            .intent
            .footprint
            .block_write
            .iter()
            .map(|(pos, _)| *pos)
            .collect();
        correction
    }
}

struct AdmittedBuild {
    prepared: PreparedBuild,
    approval: Approval,
}

/// A frozen native plan and its single-use approval. Cloned inputs share the
/// same payload; neither replay nor a second consumer can duplicate a build.
#[derive(Clone)]
pub struct AuthorizedBuild {
    connection: ConnectionId,
    actor: crate::boundary::EntityId,
    inner: Arc<Mutex<Option<AdmittedBuild>>>,
}

impl fmt::Debug for AuthorizedBuild {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizedBuild")
            .field("connection", &self.connection)
            .finish()
    }
}

impl PartialEq for AuthorizedBuild {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl AuthorizedBuild {
    pub(crate) fn connection(&self) -> ConnectionId {
        self.connection
    }
}

/// Answer only bounded, read-only previews. Abandoned queries do no work.
pub(crate) fn answer_previews(state: &mut TickState) {
    let Some(mut queries) = state.build_queries.take() else {
        return;
    };
    let mut worlds = std::mem::take(&mut state.other_dimensions);
    for _ in 0..MAX_PRECOMMIT_REQUESTS {
        let Some(query) = queries
            .pending
            .take()
            .or_else(|| queries.receiver.try_recv().ok())
        else {
            break;
        };
        if query.reply.is_closed() {
            continue;
        }
        if !query.fence.as_ref().is_none_or(PreviewFence::ready) {
            queries.pending = Some(query);
            break;
        }
        let connection = query.input.connection();
        let answer = if state
            .connections
            .entity_of(connection)
            .is_some_and(|entity| state.players.contains_key(&entity))
        {
            prepare(state, query.input)
        } else if let Some(native) = worlds.values_mut().find(|native| {
            native
                .connections
                .entity_of(connection)
                .is_some_and(|entity| native.players.contains_key(&entity))
        }) {
            // Reuse the canonical dimension ownership transfer: protection,
            // custody and plugin KV remain the ONE global native authority.
            crate::dimensions::swap_globals(state, native);
            let answer = prepare(native, query.input);
            crate::dimensions::swap_globals(state, native);
            answer
        } else {
            PreviewAnswer::Direct(query.input)
        };
        let _ = query.reply.send(answer);
    }
    state.other_dimensions = worlds;
    state.build_queries = Some(queries);
}

fn prepare(state: &TickState, input: PlayerInput) -> PreviewAnswer {
    let connection = input.connection();
    let Some(entity) = state.connections.entity_of(connection) else {
        return PreviewAnswer::Direct(input);
    };
    if crate::tick::client_build_refused(state, entity, &input) {
        return PreviewAnswer::Refused(BuildRefusal {
            reason: HookFailure::Cancelled,
            correction: BuildResync::for_input(&input).expect("a refused client build"),
        });
    }
    let Some(player) = state.players.get(&entity) else {
        return PreviewAnswer::Direct(input);
    };
    let Some(intent) = crate::tick::plan_client_build(state, entity, &input) else {
        return PreviewAnswer::Direct(input);
    };
    if intent
        .footprint
        .block_write
        .iter()
        .any(|(pos, _)| !crate::plugin_player::allows_block(state, entity, *pos))
    {
        return PreviewAnswer::Refused(BuildRefusal {
            reason: HookFailure::Cancelled,
            correction: BuildResync::for_input(&input).expect("a protected client build"),
        });
    }
    let correction = || BuildResync::for_input(&input).expect("a prepared client build");
    let Some(uuid) = state.connections.uuid_of(connection) else {
        return PreviewAnswer::Refused(BuildRefusal {
            reason: HookFailure::Stale,
            correction: correction(),
        });
    };
    let actor = match HookPlayer::try_new(uuid.to_string(), connection.0) {
        Ok(player) => HookActor::Player(player),
        Err(_) => {
            return PreviewAnswer::Refused(BuildRefusal {
                reason: HookFailure::Invalid,
                correction: correction(),
            });
        }
    };
    let edits = intent
        .footprint
        .block_write
        .iter()
        .map(|(pos, proposed)| {
            let previous = intent
                .footprint
                .block_expect
                .iter()
                .find(|(at, _)| at == pos)?
                .1;
            Some(BuildEdit::new(pos.x, pos.y, pos.z, previous, *proposed))
        })
        .collect::<Option<Vec<_>>>();
    let context = match edits
        .and_then(|edits| BuildContext::try_new(actor, state.dimension.name(), edits).ok())
    {
        Some(context) => HookContext::Build(context),
        None => {
            return PreviewAnswer::Refused(BuildRefusal {
                reason: HookFailure::Invalid,
                correction: correction(),
            });
        }
    };
    let hand = match &input {
        PlayerInput::UseItemOn { hand, .. } | PlayerInput::UseItem { hand, .. } => Some(*hand),
        PlayerInput::Dig { .. } => Some(Hand::Main),
        _ => None,
    };
    let held = hand.map(|hand| {
        let slot = crate::tick::held_slot_index(player, hand);
        (hand, slot, player.inventory.slots[slot].clone())
    });
    let planning_pose = match (&intent.kind, &input) {
        (
            crate::intent::IntentKind::PlaceBlock { .. },
            PlayerInput::UseItemOn {
                position,
                face,
                cursor,
                ..
            },
        ) => crate::tick::placement_click(state, entity, *position, *face, *cursor)
            .map(PlanningPose::Placement),
        (crate::intent::IntentKind::Bucket { .. }, PlayerInput::UseItem { .. }) => state
            .entities
            .slot_of(entity)
            .map(|slot| PlanningPose::Ray {
                eye: crate::tick::bucket_eye(state, slot),
            }),
        _ => None,
    };
    PreviewAnswer::Prepared {
        plan: PreparedBuild {
            connection,
            dimension: state.dimension,
            uuid,
            game_mode: player.game_mode,
            planning_tick: state.tick,
            planning_pose,
            held,
            source: input,
            intent,
        },
        context,
    }
}

/// The native storage owner checks guards in the same synchronous turn as the
/// mutation. No secondary store, version-cache race, I/O, or async binding.
pub(crate) fn consume_approval(
    state: &TickState,
    approval: &mut Approval,
) -> Result<(), HookFailure> {
    if approval.kind() != HookKind::Build {
        return Err(HookFailure::Invalid);
    }
    if approval
        .expected_plugin_storage_versions()
        .iter()
        .any(|scope| {
            scope.guards.iter().any(|guard| {
                state
                    .plugins
                    .version(&scope.plugin_id, &guard.key)
                    .unwrap_or(0)
                    != guard.version
            })
        })
    {
        return Err(HookFailure::Stale);
    }
    match approval.consume()? {
        HookDecision::Keep => Ok(()),
        _ => Err(HookFailure::Invalid),
    }
}

pub(crate) fn apply_authorized(
    state: &mut TickState,
    authorization: &AuthorizedBuild,
    intents: &mut Vec<Intent>,
) {
    let look =
        authorization
            .inner
            .lock()
            .as_ref()
            .and_then(|admitted| match admitted.prepared.source {
                PlayerInput::UseItem { yaw, pitch, .. } => Some((yaw, pitch)),
                _ => None,
            });
    if let Some((yaw, pitch)) = look
        && state.connections.entity_of(authorization.connection) == Some(authorization.actor)
        && let Some(slot) = state.entities.slot_of(authorization.actor)
    {
        state.entities.set_rot(slot, yaw, pitch);
    }
    // Keep the owned native plan and approval inside the existing single-use
    // payload until merge. A carrier adds neither a footprint copy nor a store.
    intents.push(Intent {
        actor: authorization.actor,
        seq: state.next_seq(authorization.actor),
        kind: crate::intent::IntentKind::AuthorizedBuild {
            authorization: authorization.clone(),
        },
        footprint: Default::default(),
    });
}

pub(crate) fn is_placement(intent: &Intent, position: BlockPos) -> bool {
    match &intent.kind {
        crate::intent::IntentKind::PlaceBlock { pos, .. } => *pos == position,
        crate::intent::IntentKind::AuthorizedBuild { authorization } => {
            authorization
                .inner
                .lock()
                .as_ref()
                .is_some_and(|admitted| {
                    matches!(admitted.prepared.intent.kind, crate::intent::IntentKind::PlaceBlock { pos, .. } if pos == position)
                })
        }
        _ => false,
    }
}

pub(crate) fn commit_authorized(
    state: &mut TickState,
    authorization: &AuthorizedBuild,
    sequence: u32,
    validate: impl FnOnce(&TickState, &Intent) -> bool,
    apply: impl FnOnce(&mut TickState, &Intent) -> bool,
) -> bool {
    let Some(mut admitted) = authorization.inner.lock().take() else {
        return false;
    };
    let plan = &admitted.prepared;
    let current = state.connections.entity_of(plan.connection);
    let valid =
        current == Some(plan.intent.actor)
            && state.dimension == plan.dimension
            && state.connections.uuid_of(plan.connection) == Some(plan.uuid)
            && state.players.get(&plan.intent.actor).is_some_and(|player| {
                player.game_mode == plan.game_mode
                    && plan.held.as_ref().is_none_or(|(hand, slot, held)| {
                        crate::tick::held_slot_index(player, *hand) == *slot
                            && player.inventory.slots.get(*slot) == Some(held)
                    })
            })
            && !crate::tick::client_build_refused(state, plan.intent.actor, &plan.source)
            && crate::tick::plan_client_build_at(
                state,
                plan.intent.actor,
                &plan.source,
                plan.planning_tick,
                plan.planning_pose,
            )
            .is_some_and(|current| {
                current.kind == plan.intent.kind && current.footprint == plan.intent.footprint
            })
            && plan
                .intent
                .footprint
                .block_write
                .iter()
                .all(|(pos, _)| crate::plugin_player::allows_block(state, plan.intent.actor, *pos))
            && (!matches!(plan.source, PlayerInput::UseItem { .. })
                || plan.intent.footprint.block_write.iter().all(|(pos, _)| {
                    crate::tick::within_block_reach(state, plan.intent.actor, *pos)
                }))
            && validate(state, &plan.intent);
    if !valid || consume_approval(state, &mut admitted.approval).is_err() {
        resync(state, &admitted.prepared.correction());
        return false;
    }
    let mut plan = admitted.prepared;
    plan.intent.seq = sequence;
    crate::plugin::observe_input(state, &plan.source);
    if !apply(state, &plan.intent) {
        resync(state, &plan.correction());
        return false;
    }
    let correction = BuildResync::for_input(&plan.source).expect("prepared build target");
    state.push_player(
        plan.connection,
        PlayerChange::BlockActionsAcked {
            sequence: correction.sequence,
        },
    );
    true
}

pub(crate) fn requires_approval(state: &TickState, input: &PlayerInput) -> bool {
    state
        .plugin_runtime
        .as_ref()
        .is_some_and(|runtime| runtime.boundary.has_precommit_hooks(HookKind::Build))
        && BuildResync::for_input(input).is_some()
        && state
            .connections
            .entity_of(input.connection())
            .is_some_and(|entity| {
                crate::tick::client_build_refused(state, entity, input)
                    || crate::tick::plan_client_build(state, entity, input).is_some()
            })
}

pub(crate) fn resync_input(state: &mut TickState, input: &PlayerInput) {
    if let Some(request) = BuildResync::for_input(input) {
        apply_resync(state, &request);
    }
}

/// Packet look belongs to input order, never to a later merge-time repair.
pub(crate) fn apply_resync(state: &mut TickState, request: &BuildResync) {
    if let CorrectionTarget::Ray { yaw, pitch, .. } = request.target
        && !state.session_locked(request.connection)
        && let Some(entity) = state.connections.entity_of(request.connection)
        && let Some(slot) = state.entities.slot_of(entity)
    {
        state.entities.set_rot(slot, yaw, pitch);
    }
    resync(state, request);
}

pub(crate) fn resync(state: &mut TickState, request: &BuildResync) {
    let mut positions = request.additional.clone();
    match request.target {
        CorrectionTarget::Block { position, face } => {
            positions.push(position);
            if let Some(face) = face {
                let step = crate::placement::Face::from_code(face).offset();
                positions.push(BlockPos {
                    x: position.x.saturating_add(step[0]),
                    y: position.y.saturating_add(step[1]),
                    z: position.z.saturating_add(step[2]),
                });
            }
        }
        CorrectionTarget::Ray { hand, yaw, pitch } => {
            if let Some(entity) = state.connections.entity_of(request.connection)
                && let Some(slot) = state.entities.slot_of(entity)
                && let Some(plan) =
                    crate::tick::plan_bucket_intent(state, entity, slot, hand, yaw, pitch)
            {
                positions.extend(plan.footprint.block_expect.into_iter().map(|(pos, _)| pos));
            }
        }
    }
    let initial = positions.len();
    for index in 0..initial {
        let pos = positions[index];
        if let Some(current) = state.world.block_at(pos.x, pos.y, pos.z)
            && let Some((other, _)) =
                crate::placement::other_half(&state.registry.blocks, pos, current)
            && !positions.contains(&other)
        {
            positions.push(other);
        }
    }
    positions.sort_unstable_by_key(|pos| (pos.x, pos.y, pos.z));
    positions.dedup();
    for position in positions {
        if let Some(block) = state.world.block_at(position.x, position.y, position.z) {
            state.push_player(
                request.connection,
                PlayerChange::BlockResynced { position, block },
            );
        }
    }
    state.push_player(
        request.connection,
        PlayerChange::BlockActionsAcked {
            sequence: request.sequence,
        },
    );
    if let Some(change) = state
        .connections
        .entity_of(request.connection)
        .and_then(|entity| state.players.get(&entity))
        .map(crate::player::PlayerState::inventory_contents)
    {
        state.push_player(request.connection, change);
    }
}

#[cfg(test)]
#[path = "build_precommit_tests.rs"]
mod tests;
