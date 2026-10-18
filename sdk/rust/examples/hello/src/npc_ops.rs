//! Interactive generic NPC API example. No population, role or housing policy.
//!
//! Commands use actual entity identities, coordinates and current native fences.
//! Read `controls` before changing work or orders; identity and control revisions
//! are independent. Every response is the engine's actual operation outcome.
//! Timed craft admission does not produce items: native `NPC_WORK` callbacks
//! report committed portions under the original admitted operation ID. Suspend
//! unfinished work with an opaque reason; resume only that suspended original ID
//! with its current control fence. Cancellation is terminal. Acknowledge retained
//! receipts only after a durable consumer projection; this example never
//! acknowledges them on the consumer's behalf.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use solaris_plugin_sdk::events::{CommandInvoked, Event, OperationOutcome, OperationPayload};
use solaris_plugin_sdk::inventories::{
    InventoryEndpoint, InventoryExpectedRevision, InventoryFence, InventoryResult,
    OwnedItemTransfer,
};
use solaris_plugin_sdk::npcs::{
    CraftReceipt, CraftWork, FollowOrder, Formation, FormationKind, HaulWork, NpcOrderResult,
    NpcResult, NpcSnapshot, Order, WorkArea, WorkAssignment, WorkOrder,
};
use solaris_plugin_sdk::types::{BlockPosition, Position};
use solaris_plugin_sdk::{
    ack_npc_work_receipts, adopt_npc, assign_npc_work, cancel_npc_order, cancel_npc_work,
    issue_npc_order, log, message_player, query_npc_controls, query_npcs, query_owned_inventory,
    release_npc, resume_npc_work, spawn_npc, suspend_npc_work, transfer_owned_items, Command,
    Config, Failure, LogLevel,
};

pub(crate) const ROOT: &str = "npc";
pub(crate) const FENCE_ROOT: &str = "npc-fence";
const USAGE: &str = "npc query [handles...] | controls <handles...> | spawn <generation> <entity-type> <x> <y> <z> | adopt <uuid> <identity-revision> | release <handle> <identity-revision> | follow <handle> <order-revision> | cancel-order <handle> <order-revision> | cancel-work <handle> <control-revision> | suspend-work <handle> <admitted-operation-id> <reason> <control-revision> | resume-work <handle> <admitted-operation-id> <control-revision> | ack-work <handle> <admitted-operation-id> <through-sequence> <control-revision> (only after durable consumer projection) | craft <handle> <control-revision> <recipe> <count> <x> <y> <z> | inventory player | inventory carry|equipment <handle> | transfer <handle> <player-slot> <carry-slot> <count> <player-revision> <player-hash> <carry-revision> <carry-hash> | haul <handle> <control-revision> <item> <units>";

pub struct Fixture {
    root: &'static str,
    sequence: u64,
    pending: Option<(String, String)>,
    /// One original admitted job and its consumer per physical NPC. Native
    /// callbacks have their own correlation IDs, never the admission request.
    work_consumers: BTreeMap<String, (String, String)>,
}

impl Fixture {
    pub fn configure(config: &Config) -> Result<Self, Failure> {
        let toml = config.toml();
        let mode = toml
            .as_ref()
            .and_then(|value| value.get("mode"))
            .and_then(|value| value.as_str());
        if mode != Some("npc-operations") {
            return Err(Failure::Invalid);
        }
        let role = toml
            .as_ref()
            .and_then(|value| value.get("role"))
            .and_then(|value| value.as_str());
        let root = match role {
            None | Some("owner") => ROOT,
            Some("observer") => FENCE_ROOT,
            Some(_) => return Err(Failure::Invalid),
        };
        Ok(Self {
            root,
            sequence: 0,
            pending: None,
            work_consumers: BTreeMap::new(),
        })
    }

    pub fn init(&mut self) -> Result<Vec<Command>, Failure> {
        log(LogLevel::Info, "NPC_API ready");
        Ok(Vec::new())
    }

    pub fn on_events(&mut self, events: &[Event]) -> Result<Vec<Command>, Failure> {
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::OperationAnswered(answer) => {
                    if self
                        .pending
                        .as_ref()
                        .is_some_and(|(request, _)| request == &answer.request)
                    {
                        let (_, player) = self.pending.take().expect("matched pending request");
                        self.track_work(&answer.outcome, &player);
                        commands.push(message_player(&player, report(&answer.outcome, "NPC_API")));
                    } else if let OperationOutcome::Committed(committed) = &answer.outcome {
                        if let OperationPayload::NpcOrder(NpcOrderResult::Work(work)) =
                            &committed.payload
                        {
                            if let Some((admitted, player)) = self.work_consumers.get(&work.handle)
                            {
                                if work.operation_id.as_deref() == Some(admitted.as_str()) {
                                    commands.push(message_player(
                                        player,
                                        report(&answer.outcome, "NPC_WORK"),
                                    ));
                                }
                            }
                        }
                    }
                }
                Event::CommandInvoked(invoked) if invoked.name == self.root => {
                    if self.pending.is_some() {
                        commands.push(message_player(
                            &invoked.player,
                            "Read the pending native answer before issuing another control.",
                        ));
                        continue;
                    }
                    self.sequence = self.sequence.checked_add(1).ok_or(Failure::Invalid)?;
                    let request = format!("npc-{}-{}", invoked.session, self.sequence);
                    let operation_id = format!("npc-op-{}-{}", invoked.session, self.sequence);
                    match self.operation(invoked, &request, &operation_id) {
                        Some(command) => {
                            self.pending = Some((request, invoked.player.clone()));
                            commands.push(command);
                        }
                        None => commands.push(message_player(&invoked.player, USAGE)),
                    }
                }
                _ => {}
            }
        }
        Ok(commands)
    }

    fn track_work(&mut self, outcome: &OperationOutcome, player: &str) {
        let OperationOutcome::Committed(committed) = outcome else {
            return;
        };
        match &committed.payload {
            OperationPayload::NpcOrder(NpcOrderResult::Work(work)) => {
                self.track_assignment(work, player);
            }
            OperationPayload::NpcOrder(NpcOrderResult::Controls(page)) => {
                for npc in &page.npcs {
                    if let Some(work) = &npc.work {
                        self.track_assignment(work, player);
                    }
                }
            }
            _ => {}
        }
    }

    fn track_assignment(&mut self, work: &WorkAssignment, player: &str) {
        if let Some(admitted) = &work.operation_id {
            self.work_consumers
                .insert(work.handle.clone(), (admitted.clone(), player.to_owned()));
        }
    }

    fn operation(
        &self,
        invoked: &CommandInvoked,
        request: &str,
        operation_id: &str,
    ) -> Option<Command> {
        let args = &invoked.arguments;
        let argument = |index: usize| args.get(index).map(String::as_str);
        match argument(0)? {
            "query" => Some(query_npcs(request, &args[1..], None)),
            "controls" if args.len() > 1 => Some(query_npc_controls(request, &args[1..])),
            "spawn" if args.len() == 6 => Some(spawn_npc(
                request,
                operation_id,
                argument(1)?,
                Position {
                    x: argument(3)?.parse().ok()?,
                    y: argument(4)?.parse().ok()?,
                    z: argument(5)?.parse().ok()?,
                },
                argument(2)?,
            )),
            "adopt" if args.len() == 3 => Some(adopt_npc(
                request,
                operation_id,
                invoked.session,
                argument(1)?,
                argument(2)?.parse().ok()?,
            )),
            "release" if args.len() == 3 => Some(release_npc(
                request,
                operation_id,
                argument(1)?,
                argument(2)?.parse().ok()?,
            )),
            "follow" if args.len() == 3 => Some(issue_npc_order(
                request,
                operation_id,
                vec![argument(1)?.to_owned()],
                vec![argument(2)?.parse().ok()?],
                Order::Follow(FollowOrder {
                    target_player: invoked.session,
                    formation: Formation {
                        kind: FormationKind::Line,
                        spacing: 4,
                    },
                }),
            )),
            "cancel-order" if args.len() == 3 => Some(cancel_npc_order(
                request,
                operation_id,
                vec![argument(1)?.to_owned()],
                vec![argument(2)?.parse().ok()?],
            )),
            "cancel-work" if args.len() == 3 => Some(cancel_npc_work(
                request,
                operation_id,
                argument(1)?,
                argument(2)?.parse().ok()?,
            )),
            "suspend-work" if args.len() == 5 => Some(suspend_npc_work(
                request,
                operation_id,
                argument(1)?,
                argument(2)?,
                argument(3)?,
                argument(4)?.parse().ok()?,
            )),
            "resume-work" if args.len() == 4 => Some(resume_npc_work(
                request,
                operation_id,
                argument(1)?,
                argument(2)?,
                argument(3)?.parse().ok()?,
            )),
            "ack-work" if args.len() == 5 => Some(ack_npc_work_receipts(
                request,
                operation_id,
                argument(1)?,
                argument(2)?,
                argument(3)?.parse().ok()?,
                argument(4)?.parse().ok()?,
            )),
            "inventory" => {
                let endpoint = match (argument(1)?, args.len()) {
                    ("player", 2) => InventoryEndpoint::PlayerInventory(invoked.session),
                    ("carry", 3) => InventoryEndpoint::NpcCarry(argument(2)?.to_owned()),
                    ("equipment", 3) => InventoryEndpoint::NpcEquipment(argument(2)?.to_owned()),
                    _ => return None,
                };
                Some(query_owned_inventory(request, endpoint, None))
            }
            "transfer" if args.len() == 9 => {
                let source = InventoryEndpoint::PlayerInventory(invoked.session);
                let destination = InventoryEndpoint::NpcCarry(argument(1)?.to_owned());
                Some(transfer_owned_items(
                    request,
                    operation_id,
                    invoked.session,
                    None,
                    vec![OwnedItemTransfer {
                        source: source.clone(),
                        source_slot: argument(2)?.parse().ok()?,
                        destination: destination.clone(),
                        destination_slot: argument(3)?.parse().ok()?,
                        count: argument(4)?.parse().ok()?,
                    }],
                    vec![
                        InventoryExpectedRevision {
                            endpoint: source,
                            fence: InventoryFence {
                                revision: argument(5)?.parse().ok()?,
                                snapshot_hash: argument(6)?.to_owned(),
                            },
                        },
                        InventoryExpectedRevision {
                            endpoint: destination,
                            fence: InventoryFence {
                                revision: argument(7)?.parse().ok()?,
                                snapshot_hash: argument(8)?.to_owned(),
                            },
                        },
                    ],
                ))
            }
            "haul" if args.len() == 5 => Some(assign_npc_work(
                request,
                operation_id,
                argument(1)?,
                WorkOrder::Haul(HaulWork {
                    source: InventoryEndpoint::NpcCarry(argument(1)?.to_owned()),
                    destination: InventoryEndpoint::NpcEquipment(argument(1)?.to_owned()),
                    item: Some(argument(3)?.to_owned()),
                }),
                argument(4)?.parse().ok()?,
                argument(2)?.parse().ok()?,
            )),
            "craft" if args.len() == 8 => {
                let at = BlockPosition {
                    x: argument(5)?.parse().ok()?,
                    y: argument(6)?.parse().ok()?,
                    z: argument(7)?.parse().ok()?,
                };
                let count: u32 = argument(4)?.parse().ok()?;
                Some(assign_npc_work(
                    request,
                    operation_id,
                    argument(1)?,
                    WorkOrder::Craft(CraftWork {
                        recipe: argument(3)?.to_owned(),
                        count,
                        station: WorkArea {
                            dimension: "minecraft:overworld".to_owned(),
                            min: at,
                            max: at,
                        },
                    }),
                    u64::from(count),
                    argument(2)?.parse().ok()?,
                ))
            }
            _ => None,
        }
    }
}

/// Report native fields separately from the complete diagnostic, so a component
/// consumer can use actual fences and item images without parsing Rust Debug.
fn report(outcome: &OperationOutcome, marker: &str) -> String {
    let mut text = String::from(marker);
    let payload = match outcome {
        OperationOutcome::Committed(committed) => {
            write!(text, " state=committed revision={}", committed.revision).unwrap();
            &committed.payload
        }
        OperationOutcome::Refused(refused) => {
            write!(
                text,
                " state=refused failure={}",
                vocabulary(&refused.reason)
            )
            .unwrap();
            &refused.payload
        }
    };
    match payload {
        OperationPayload::Npc(NpcResult::Snapshot(npc)) => {
            text.push_str(" kind=npcs npcs=1");
            report_npc(&mut text, 0, npc);
        }
        OperationPayload::Npc(NpcResult::Page(page)) => {
            write!(text, " kind=npcs npcs={}", page.npcs.len()).unwrap();
            for (index, npc) in page.npcs.iter().enumerate() {
                report_npc(&mut text, index, npc);
            }
        }
        OperationPayload::NpcOrder(NpcOrderResult::Controls(page)) => {
            write!(text, " kind=controls npcs={}", page.npcs.len()).unwrap();
            for (index, npc) in page.npcs.iter().enumerate() {
                write!(
                    text,
                    " npc{index}_handle={} npc{index}_revision={} npc{index}_order_revision={} npc{index}_order={}",
                    npc.handle, npc.revision, npc.order_revision, npc.order.is_some(),
                ).unwrap();
                if let Some(work) = &npc.work {
                    write!(
                        text,
                        " npc{index}_work={} npc{index}_reason={} npc{index}_done={} npc{index}_planned={}",
                        vocabulary(&work.state), vocabulary(&work.reason), work.work_units_done, work.work_units_planned,
                    ).unwrap();
                    report_work_ledger(&mut text, &format!("npc{index}_"), work);
                }
                if let Some(pending) = &npc.pending_work {
                    write!(text, " npc{index}_pending={}", pending.work_units).unwrap();
                }
            }
        }
        OperationPayload::NpcOrder(NpcOrderResult::Work(work)) => {
            write!(
                text,
                " kind=work handle={} control_revision={} work_state={} reason={} done={} planned={}",
                work.handle, work.revision, vocabulary(&work.state), vocabulary(&work.reason),
                work.work_units_done, work.work_units_planned,
            ).unwrap();
            report_work_ledger(&mut text, "", work);
        }
        OperationPayload::NpcOrder(NpcOrderResult::WorkCancelled(work)) => {
            write!(
                text,
                " kind=work-cancelled handle={} control_revision={}",
                work.handle, work.revision,
            )
            .unwrap();
        }
        OperationPayload::NpcOrder(NpcOrderResult::Order(order)) => {
            report_order(&mut text, order.order_revision, &order.members);
        }
        OperationPayload::NpcOrder(NpcOrderResult::OrderCancelled(order)) => {
            report_order(&mut text, order.order_revision, &order.members);
        }
        OperationPayload::OwnedInventory(InventoryResult::Snapshot(snapshot)) => {
            write!(
                text,
                " kind=inventory inventory_revision={} hash={}",
                snapshot.fence.revision, snapshot.fence.snapshot_hash,
            )
            .unwrap();
            for slot in &snapshot.slots {
                if let Some(item) = &slot.item {
                    write!(
                        text,
                        " item_{}={}:{}",
                        slot.slot, item.resource_id, item.count,
                    )
                    .unwrap();
                }
            }
        }
        OperationPayload::OwnedInventory(InventoryResult::Transfer(endpoints)) => {
            write!(text, " kind=transfer endpoints={}", endpoints.len()).unwrap();
        }
        _ => {}
    }
    write!(text, " detail={outcome:?}").unwrap();
    text
}

/// A contract enum in the contract's own vocabulary. The generated bindings
/// spell `Debug` as `Type::Variant`; a consumer reads the variant.
fn vocabulary(value: &impl core::fmt::Debug) -> String {
    let text = format!("{value:?}");
    let mut out = String::with_capacity(text.len());
    let mut identifier = 0;
    let mut rest = text.as_str();
    while let Some(next) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("::") {
            out.truncate(identifier);
            rest = after;
            continue;
        }
        out.push(next);
        if !(next.is_alphanumeric() || next == '_') {
            identifier = out.len();
        }
        rest = &rest[next.len_utf8()..];
    }
    out
}

fn report_work_ledger(text: &mut String, prefix: &str, work: &WorkAssignment) {
    if let Some(admitted) = &work.operation_id {
        write!(text, " {prefix}admitted_operation_id={admitted}").unwrap();
    }
    write!(
        text,
        " {prefix}acknowledged_sequence={} {prefix}receipts={}",
        work.acknowledged_sequence,
        work.receipts.len(),
    )
    .unwrap();
    if let Some(due) = work.due_tick {
        write!(text, " {prefix}due_tick={due}").unwrap();
    }
    if let Some(reason) = &work.suspension_reason {
        write!(text, " {prefix}suspension_reason={reason}").unwrap();
    }
    for (index, receipt) in work.receipts.iter().enumerate() {
        report_receipt(text, &format!("{prefix}receipt{index}_"), receipt);
    }
}

fn report_receipt(text: &mut String, prefix: &str, receipt: &CraftReceipt) {
    write!(
        text,
        " {prefix}operation_id={} {prefix}handle={} {prefix}portion_id={} {prefix}sequence={} {prefix}recipe={} {prefix}recipe_revision={} {prefix}seed={} {prefix}simulation_tick={} {prefix}duration_ticks={} {prefix}revision={}",
        receipt.operation_id, receipt.handle, receipt.portion_id, receipt.sequence,
        receipt.recipe, receipt.recipe_revision, receipt.seed, receipt.simulation_tick,
        receipt.duration_ticks, receipt.revision,
    ).unwrap();
    for (kind, stacks) in [("input", &receipt.inputs), ("output", &receipt.outputs)] {
        for (index, stack) in stacks.iter().enumerate() {
            write!(
                text,
                " {prefix}{kind}{index}_slot={} {prefix}{kind}{index}_item={}:{}",
                stack.slot, stack.item.resource_id, stack.item.count,
            )
            .unwrap();
        }
    }
}

fn report_npc(text: &mut String, index: usize, npc: &NpcSnapshot) {
    write!(
        text,
        " npc{index}_handle={} npc{index}_uuid={} npc{index}_revision={} npc{index}_lifecycle={}",
        npc.handle,
        npc.entity_uuid,
        npc.revision,
        vocabulary(&npc.lifecycle),
    )
    .unwrap();
    if let Some(generation) = &npc.generation_id {
        write!(text, " npc{index}_generation={generation}").unwrap();
    }
    if let Some(loaded) = &npc.loaded {
        write!(
            text,
            " npc{index}_x={} npc{index}_y={} npc{index}_z={}",
            loaded.x, loaded.y, loaded.z,
        )
        .unwrap();
    }
}

fn report_order(
    text: &mut String,
    revision: u64,
    members: &[solaris_plugin_sdk::npcs::OrderMemberOutcome],
) {
    write!(
        text,
        " kind=order order_revision={revision} members={}",
        members.len()
    )
    .unwrap();
    for (index, member) in members.iter().enumerate() {
        write!(
            text,
            " member{index}_handle={} member{index}_state={} member{index}_slot={}",
            member.handle,
            vocabulary(&member.state),
            vocabulary(&member.formation_slot),
        )
        .unwrap();
    }
}
