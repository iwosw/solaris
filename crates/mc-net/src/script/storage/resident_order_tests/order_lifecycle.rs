//! Order lifecycle contracts: squad formation, admission replay, demobilisation and per-member refusals.
//!
//! Moved out of `resident_order_tests.rs` by CP-REFACTOR-040 sub-slice 5.
//! Child of `resident_order_tests`: explicit imports only, no glob.
//!
//! Order-family builders (`order_request`, `order_of`, `formation`, `demobilize_request`) stay in shared support because the combat module uses them too.

use crate::play::EntityPhysicsStep;
use crate::play::ResidentGoal;
use crate::script::storage::PluginStorage;
use mc_data::Identifier;
use mc_data::ItemStack;
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_entity::Vec3;
use mc_protocol::codec;
use mc_script::ScriptBlockPosition;
use mc_script::ScriptOperation;
use mc_script::ScriptOperationFailure;
use mc_script::ScriptOperationOutcome;
use mc_script::ScriptOperationPayload;
use mc_script::ScriptOperationRequest;
use mc_script::ScriptOrderMemberState;
use mc_script::ScriptResidentOrder;
use mc_script::ScriptResidentOrderOperation;
use mc_script::ScriptResidentOrderResult;
use mc_script::ScriptWorkPauseReason;
use mc_world::BlockPos;
use mc_world::ChunkPos;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::{
    Fixture, OWNER, demobilize_request, formation, gear, members_revision, order_of, order_request,
    seed_gear, state_of,
};
/// (A09) A squad ordered through open terrain reforms into distinct standable
/// slots. A closed passage or one unplaceable slot blocks the formation without
/// teleportation; clearing that slot and reissuing the order reforms the squad.
#[tokio::test]
async fn squad_reforms_through_an_open_passage_and_refuses_a_closed_one() {
    for (wall, blocked_slot) in [(false, false), (true, false), (false, true)] {
        let standable_queries = Arc::new(AtomicUsize::new(0));
        let route_queries = Arc::new(AtomicUsize::new(0));
        let fixture = Fixture::with_query_counts(
            wall,
            Arc::clone(&standable_queries),
            Arc::clone(&route_queries),
        );
        let mut storage = fixture.storage();
        let mut members = Vec::new();
        for slot in 3..6 {
            let member = fixture
                .resident(
                    &mut storage,
                    slot,
                    Vec3::new(2.5 + f64::from(slot), 64.0, 4.5),
                )
                .await;
            members.push(member);
        }
        if blocked_slot {
            fixture
                .world
                .lock()
                .await
                .set_block_at(
                    BlockPos { x: 13, y: 64, z: 8 },
                    state_of(&fixture.blocks, "stone"),
                )
                .expect("block one formation slot");
        }
        standable_queries.store(0, Ordering::Relaxed);
        route_queries.store(0, Ordering::Relaxed);
        let handles = members
            .iter()
            .map(|(handle, _)| handle.clone())
            .collect::<Vec<_>>();
        let request = order_request(
            if wall {
                "move-closed"
            } else if blocked_slot {
                "move-blocked-slot"
            } else {
                "move-open"
            },
            &handles,
            &vec![0; handles.len()],
            ScriptResidentOrder::Move {
                dimension: "minecraft:overworld".to_owned(),
                anchor: ScriptBlockPosition::new(13, 64, 8),
                heading_degrees: 0,
                formation: formation(),
            },
        );
        let outcome = fixture.execute(&mut storage, &request).await;
        assert_eq!(outcome.failure(), None, "batch admitted: {outcome:?}");
        let (first_revision, outcomes, _) = order_of(&outcome);
        assert_eq!(outcomes.len(), 3);
        let placement_queries = standable_queries.load(Ordering::Relaxed);
        let member_routes = route_queries.load(Ordering::Relaxed);
        if blocked_slot {
            assert!(
                placement_queries <= handles.len() * 2,
                "one bounded formation scan, not one scan per member"
            );
            assert_eq!(member_routes, 0, "unplaceable squad does not query paths");
        } else {
            assert_eq!(
                placement_queries,
                handles.len(),
                "one flat-ground placement query per squad member"
            );
            assert_eq!(member_routes, 3, "one route decision per member");
        }
        eprintln!(
            "squad wall={wall} blocked_slot={blocked_slot} size=3 standable_queries={placement_queries} route_queries={member_routes}"
        );
        let repeated = fixture.execute(&mut storage, &request).await;
        assert_eq!(members_revision(&repeated), first_revision);
        assert_eq!(standable_queries.load(Ordering::Relaxed), placement_queries);
        assert_eq!(route_queries.load(Ordering::Relaxed), member_routes);
        if wall || blocked_slot {
            assert!(
                outcomes
                    .iter()
                    .all(|member| member.state == ScriptOrderMemberState::BlockedRoute),
                "unreachable formation: {outcomes:?}"
            );
            assert!(
                outcomes
                    .iter()
                    .all(|member| member.formation_slot.is_none())
            );
            for (_, uuid) in &members {
                // No teleport and no permanent shared coordinate.
                let position = fixture.position(*uuid).await;
                assert!(
                    position.x < 10.0,
                    "the member stayed near side: {position:?}"
                );
                assert_eq!(
                    fixture.goal(*uuid).await,
                    mc_entity::GoalState::Idle,
                    "no goal was pushed"
                );
            }
            if blocked_slot {
                fixture
                    .world
                    .lock()
                    .await
                    .set_block_at(
                        BlockPos { x: 13, y: 64, z: 8 },
                        state_of(&fixture.blocks, "air"),
                    )
                    .expect("clear the obstructed slot");
                standable_queries.store(0, Ordering::Relaxed);
                route_queries.store(0, Ordering::Relaxed);
                let cleared = fixture
                    .execute(
                        &mut storage,
                        &order_request(
                            "move-cleared-slot",
                            &handles,
                            &[first_revision; 3],
                            ScriptResidentOrder::Move {
                                dimension: "minecraft:overworld".to_owned(),
                                anchor: ScriptBlockPosition::new(13, 64, 8),
                                heading_degrees: 0,
                                formation: formation(),
                            },
                        ),
                    )
                    .await;
                let (_, clear_members, _) = order_of(&cleared);
                assert!(
                    clear_members
                        .iter()
                        .all(|member| member.state == ScriptOrderMemberState::Applied)
                );
                assert_eq!(standable_queries.load(Ordering::Relaxed), 3);
                assert_eq!(route_queries.load(Ordering::Relaxed), 3);
            }
        } else {
            assert!(
                outcomes
                    .iter()
                    .all(|member| member.state == ScriptOrderMemberState::Applied),
                "open passage: {outcomes:?}"
            );
            let slots = outcomes
                .iter()
                .filter_map(|member| member.formation_slot)
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(slots.len(), 3, "every member has a distinct slot");
            let mut goals = Vec::new();
            for (_, uuid) in &members {
                let goal = fixture.goal(*uuid).await;
                assert!(
                    matches!(goal, mc_entity::GoalState::FollowPosition { .. }),
                    "a real movement goal was pushed: {goal:?}"
                );
                goals.push(goal);
            }
            let records = handles
                .iter()
                .map(|handle| {
                    storage
                        .resident_orders()
                        .record(handle)
                        .cloned()
                        .expect("accepted member record")
                })
                .collect::<Vec<_>>();
            storage
                .force_pending_admission_for_test(OWNER, "reform-replay", &records)
                .expect("one group replay is durable");
            fixture
                .sessions
                .apply_resident_goals(
                    members
                        .iter()
                        .map(|(_, uuid)| ResidentGoal {
                            uuid: *uuid,
                            goal: mc_entity::GoalState::Idle,
                        })
                        .collect(),
                )
                .await;
            let mut reopened =
                PluginStorage::open(fixture.storage_root.path()).expect("reopen squad");
            fixture
                .runtime
                .recover_resident_orders(&mut reopened)
                .await
                .unwrap();
            for ((_, uuid), expected) in members.iter().zip(&goals) {
                assert_eq!(
                    fixture.goal(*uuid).await,
                    expected.clone(),
                    "replay preserves each member's distinct formation destination"
                );
            }
            let mut storage = reopened;
            standable_queries.store(0, Ordering::Relaxed);
            route_queries.store(0, Ordering::Relaxed);
            let moving_roster = handles[..2].to_vec();
            let second = fixture
                .execute(
                    &mut storage,
                    &order_request(
                        "move-smaller-roster",
                        &moving_roster,
                        &[first_revision; 2],
                        ScriptResidentOrder::Move {
                            dimension: "minecraft:overworld".to_owned(),
                            anchor: ScriptBlockPosition::new(12, 64, 11),
                            heading_degrees: 0,
                            formation: formation(),
                        },
                    ),
                )
                .await;
            let (second_revision, next_members, _) = order_of(&second);
            assert!(
                next_members
                    .iter()
                    .all(|member| member.state == ScriptOrderMemberState::Applied)
            );
            let reformed_queries = standable_queries.load(Ordering::Relaxed);
            assert_eq!(route_queries.load(Ordering::Relaxed), 2);
            assert_eq!(reformed_queries, moving_roster.len());
            eprintln!(
                "squad wall=false size=2 standable_queries={reformed_queries} route_queries=2"
            );
            assert_eq!(next_members.len(), 2);
            assert_ne!(
                fixture.goal(members[0].1).await,
                fixture.goal(members[1].1).await
            );
            assert_eq!(
                fixture.goal(members[2].1).await,
                goals[2],
                "a member omitted from a new roster retains its previous order"
            );
            let moving_goals = vec![
                ResidentGoal {
                    uuid: members[0].1,
                    goal: fixture.goal(members[0].1).await,
                },
                ResidentGoal {
                    uuid: members[1].1,
                    goal: fixture.goal(members[1].1).await,
                },
            ];
            let cancel = ScriptOperationRequest::try_new(
                "cancel-smaller-roster",
                ScriptOperation::ResidentOrder {
                    operation: ScriptResidentOrderOperation::CancelOrder {
                        operation_id: "cancel-smaller-roster".to_owned(),
                        handles: moving_roster,
                        expected_order_revisions: vec![second_revision; 2],
                    },
                },
            )
            .expect("fenced group cancellation");
            let cancelled = fixture.execute(&mut storage, &cancel).await;
            assert_eq!(cancelled.failure(), None);
            for (_, uuid) in members.iter().take(2) {
                assert_eq!(fixture.goal(*uuid).await, mc_entity::GoalState::Idle);
            }
            assert_eq!(fixture.goal(members[2].1).await, goals[2]);
            let cancelled_records = handles[..2]
                .iter()
                .map(|handle| {
                    storage
                        .resident_orders()
                        .record(handle)
                        .cloned()
                        .expect("cancelled member remains durable")
                })
                .collect::<Vec<_>>();
            assert!(
                cancelled_records
                    .iter()
                    .all(|record| record.order.is_none())
            );
            storage
                .force_pending_admission_for_test(OWNER, "cancel-replay", &cancelled_records)
                .expect("pending group cancellation is durable");
            fixture.sessions.apply_resident_goals(moving_goals).await;
            let mut reopened =
                PluginStorage::open(fixture.storage_root.path()).expect("reopen cancellation");
            fixture
                .runtime
                .recover_resident_orders(&mut reopened)
                .await
                .unwrap();
            assert!(reopened.resident_orders().pending_admissions().is_empty());
            for (_, uuid) in members.iter().take(2) {
                assert_eq!(
                    fixture.goal(*uuid).await,
                    mc_entity::GoalState::Idle,
                    "pending cancellation must replay Idle before acknowledgement"
                );
            }
            assert_eq!(fixture.goal(members[2].1).await, goals[2]);
        }
    }
}

/// (CP-047) A resident that dies while demobilising publishes the loot its
/// configured mob table declares, keeps exactly one owner of its issued gear
/// (the durable record) and cannot finish the demobilisation, before or after a
/// restart. No corpse loot can carry the same gear.
#[tokio::test]
async fn a_demobilising_resident_killed_outright_keeps_one_gear_owner_and_configured_loot() {
    let fixture = Fixture::new(false);
    use crate::play::ResidentAttack;
    use mc_data::items::{ItemRegistry, ItemReport};
    use mc_data::loot::{LootDrop, LootTables};

    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(5.5, 64.0, 4.5))
        .await;
    seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_sword", 1), ("minecraft:arrow", 8)],
    );
    let leather = Identifier::parse("minecraft:leather").expect("leather identifier");
    let villager = Identifier::parse("minecraft:villager").expect("villager identifier");
    fixture.sessions.configure_arrow_kill_rewards(
        Some(98),
        None,
        None,
        Arc::new(ItemRegistry::from_report(&[ItemReport {
            id: leather.clone(),
            protocol_id: 41,
        }])),
        Arc::new(solaris_required_item_facts()),
        Arc::new(LootTables::from_drop_lists(
            std::collections::BTreeMap::from([(villager, vec![LootDrop::single(leather)])]),
            std::collections::BTreeMap::new(),
        )),
    );
    let before = gear(&storage, &handle);
    // Death must be handled on a resident that is already demobilising.
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("the seeded gear carries a record");
    let outcome = fixture
        .execute(
            &mut storage,
            &demobilize_request("demob-before-death", &handle, record.revision),
        )
        .await;
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!("expected a resident order payload");
    };
    let ScriptResidentOrderResult::Demobilized { resident } = &**result else {
        panic!("expected a demobilisation result, got {result:?}");
    };
    assert_eq!(
        resident.state,
        mc_script::ScriptDemobilizeState::Demobilizing,
        "a resident with no warehouse to hand its gear to stays demobilising"
    );
    assert_eq!(resident.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert!(resident.returned.is_empty());
    assert_eq!(
        gear(&storage, &handle),
        before,
        "an unfinished demobilisation keeps the gear on the resident"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("the demobilising record survives");
    let expected = fixture.sessions.resident_entity_snapshots(&[uuid]).await[0]
        .clone()
        .expect("the resident is native and alive");
    let entity = expected.id;
    let hit = fixture
        .sessions
        .commit_resident_damage(
            OWNER,
            vec![ResidentAttack {
                uuid,
                amount: 1_000.0,
                expected,
            }],
        )
        .await;
    assert!(
        hit[0].as_ref().is_some_and(|hit| hit.killed),
        "the resident must really die: {hit:?}"
    );
    assert_eq!(
        fixture.sessions.published_entity_health_for_test(entity),
        Some(0.0),
        "an accepted death reaches the session projection every other source uses"
    );
    let drops = fixture
        .sessions
        .persisted_entity_records()
        .into_iter()
        .filter(|record| record.snapshot.type_name == "minecraft:item")
        .collect::<Vec<_>>();
    assert_eq!(
        drops.len(),
        1,
        "the death publishes exactly the loot its configured mob table declares"
    );
    let stack = drops[0]
        .snapshot
        .item_stack
        .as_ref()
        .expect("a dropped item entity carries its stack");
    assert_eq!(
        (stack.item_id, stack.count),
        (41, 1),
        "the published loot is the configured drop, never the issued gear"
    );
    assert_eq!(
        gear(&storage, &handle),
        before,
        "the durable record stays the only owner of the issued gear"
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .expect("the record survives the death")
            .assignment,
        crate::script::storage::resident_orders::DurableAssignment::Demobilizing,
        "a death does not undo the pending demobilisation"
    );
    for attempt in ["demob-after-real-death", "demob-after-real-death-repeat"] {
        let outcome = fixture
            .execute(
                &mut storage,
                &demobilize_request(attempt, &handle, record.revision),
            )
            .await;
        assert_eq!(
            outcome.failure(),
            Some(ScriptOperationFailure::NotFound),
            "a dead resident cannot finish demobilisation ({attempt})"
        );
    }
    assert_eq!(gear(&storage, &handle), before);

    // A restart keeps the same single owner and the same refusal.
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).expect("reopen orders");
    assert_eq!(gear(&reopened, &handle), before);
    let outcome = fixture
        .execute(
            &mut reopened,
            &demobilize_request("demob-after-restart", &handle, record.revision),
        )
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::NotFound),
        "a restart does not let a dead resident finish demobilisation"
    );
    assert_eq!(gear(&reopened, &handle), before);
}

/// (A11) A member that dies between prepare and commit invalidates the whole
/// batch: no member's order changes.
#[tokio::test]
async fn a_member_dying_between_prepare_and_commit_changes_no_order() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (first_handle, _first) = fixture
        .resident(&mut storage, 3, Vec3::new(3.5, 64.0, 3.5))
        .await;
    let (second_handle, second) = fixture
        .resident(&mut storage, 4, Vec3::new(4.5, 64.0, 3.5))
        .await;
    let handles = vec![first_handle.clone(), second_handle.clone()];

    let move_order = ScriptResidentOrder::Move {
        dimension: "minecraft:overworld".to_owned(),
        anchor: ScriptBlockPosition::new(3, 64, 6),
        heading_degrees: 0,
        formation: formation(),
    };
    // An accepted order first, so the stale batch has an old order to preserve.
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("first", &handles, &[0, 0], move_order.clone()),
        )
        .await;
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    let revisions = [members_revision(&outcome), members_revision(&outcome)];
    let before = storage
        .resident_orders()
        .record(&first_handle)
        .cloned()
        .expect("first record");
    let before_second = storage
        .resident_orders()
        .record(&second_handle)
        .cloned()
        .expect("second record");

    let records = crate::script::storage::resident_order_execution::resident_order_records(
        &storage, OWNER, &handles, &revisions,
    )
    .expect("records load");
    let request = order_request("second", &handles, &revisions, move_order);
    let prepared = fixture
        .runtime
        .prepare_resident_order_batch(&mut storage, &request, &records)
        .await
        .expect("prepared");
    let Some(mut admission) = prepared else {
        panic!("the batch prepared against live members");
    };

    // The member dies between prepare and commit.
    let entity = fixture
        .sessions
        .resident_entity_snapshots(&[second])
        .await
        .into_iter()
        .next()
        .flatten()
        .expect("second is live")
        .id;
    assert!(fixture.sessions.remove_tracked_entity_for_test(entity));

    assert!(
        !crate::script::storage::resident_order_execution::commit_resident_order_batch(
            &fixture.sessions,
            &records,
            &mut admission,
        )
        .await,
        "the drifted batch is refused wholesale"
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&first_handle)
            .cloned()
            .expect("first record"),
        before,
        "a surviving member keeps its old order"
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&second_handle)
            .cloned()
            .expect("second record"),
        before_second,
    );
}

/// (A12) A committed admission replays exactly once; the same operation id with
/// a new payload conflicts.
#[tokio::test]
async fn a_committed_admission_replays_exactly_once() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, _uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let handles = vec![handle.clone()];
    let order = ScriptResidentOrder::Move {
        dimension: "minecraft:overworld".to_owned(),
        anchor: ScriptBlockPosition::new(4, 64, 8),
        heading_degrees: 0,
        formation: formation(),
    };
    let request = order_request("replay", &handles, &[0], order.clone());
    let outcome = fixture.execute(&mut storage, &request).await;
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    let revision = members_revision(&outcome);
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("record");
    assert!(storage.resident_orders().pending_admissions().is_empty());

    // A crash between the durable group commit and its engine application leaves
    // a committed admission with a pending member.
    let admission_id = storage
        .force_pending_admission_for_test(OWNER, "replay", std::slice::from_ref(&record))
        .expect("pending admission is durable");
    assert_eq!(
        storage.resident_orders().pending_admissions(),
        vec![(admission_id, handles.clone())]
    );

    // Re-open: the pending member is applied once and never again.
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).unwrap();
    assert_eq!(
        reopened.resident_orders().pending_admissions(),
        vec![(admission_id, handles.clone())]
    );
    fixture
        .runtime
        .recover_resident_orders(&mut reopened)
        .await
        .unwrap();
    assert!(
        reopened.resident_orders().pending_admissions().is_empty(),
        "the accepted batch is applied exactly once"
    );
    fixture
        .runtime
        .recover_resident_orders(&mut reopened)
        .await
        .unwrap();
    assert!(
        reopened.resident_orders().pending_admissions().is_empty(),
        "a second replay has nothing left to apply"
    );

    // A repeated operation id with a new payload conflicts without any effect.
    let conflicting = order_request(
        "replay",
        &handles,
        &[revision],
        ScriptResidentOrder::Move {
            dimension: "minecraft:overworld".to_owned(),
            anchor: ScriptBlockPosition::new(2, 64, 2),
            heading_degrees: 0,
            formation: formation(),
        },
    );
    let outcome = fixture.execute(&mut reopened, &conflicting).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
    assert_eq!(
        reopened
            .resident_orders()
            .record(&handle)
            .expect("record")
            .order
            .as_ref()
            .expect("order")
            .order,
        order,
        "the conflicting call changed nothing"
    );
}

/// (f) Demobilising a serving resident interrupts movement durably, even when
/// no warehouse can yet take its issued gear.
#[tokio::test]
async fn demobilisation_without_a_warehouse_keeps_handle_and_gear() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(5.5, 64.0, 4.5))
        .await;
    seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_sword", 1), ("minecraft:arrow", 8)],
    );
    let march = fixture
        .execute(
            &mut storage,
            &order_request(
                "march-before-demob",
                std::slice::from_ref(&handle),
                &[0],
                ScriptResidentOrder::Move {
                    dimension: "minecraft:overworld".to_owned(),
                    anchor: ScriptBlockPosition::new(13, 64, 8),
                    heading_degrees: 0,
                    formation: formation(),
                },
            ),
        )
        .await;
    assert_eq!(march.failure(), None);
    let prior_goal = fixture.goal(uuid).await;
    assert!(matches!(
        prior_goal,
        mc_entity::GoalState::FollowPosition { .. }
    ));
    let before = gear(&storage, &handle);
    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("record")
        .revision;

    let outcome = fixture
        .execute(
            &mut storage,
            &demobilize_request("demob-1", &handle, revision),
        )
        .await;
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!("expected a resident order payload");
    };
    let ScriptResidentOrderResult::Demobilized { resident } = &**result else {
        panic!("expected a demobilisation result, got {result:?}");
    };
    assert_eq!(
        resident.state,
        mc_script::ScriptDemobilizeState::Demobilizing
    );
    assert_eq!(resident.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert!(resident.returned.is_empty());
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("record");
    assert_eq!(record.entity_uuid, uuid.to_string(), "same resident handle");
    assert_eq!(gear(&storage, &handle), before, "no item was lost");
    assert_eq!(fixture.goal(uuid).await, mc_entity::GoalState::Idle);
    let admission_id = storage
        .force_pending_admission_for_test(OWNER, "demob-replay", std::slice::from_ref(&record))
        .expect("committed demobilization requires Idle on replay");
    fixture
        .sessions
        .apply_resident_goals(vec![ResidentGoal {
            uuid,
            goal: prior_goal,
        }])
        .await;

    // Everything is durable: the reopened journal still shows the same state.
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).unwrap();
    fixture
        .runtime
        .recover_resident_orders(&mut reopened)
        .await
        .unwrap();
    assert_eq!(fixture.goal(uuid).await, mc_entity::GoalState::Idle);
    assert!(
        reopened.resident_orders().pending_admissions().is_empty(),
        "demobilization admission {admission_id} is acknowledged after Idle"
    );
    let record = reopened
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("record survives the reopen");
    assert_eq!(
        record.assignment,
        crate::script::storage::resident_orders::DurableAssignment::Demobilizing
    );
    assert_eq!(gear(&reopened, &handle), before);
    let entity_id = fixture.sessions.resident_entity_snapshots(&[uuid]).await[0]
        .as_ref()
        .expect("resident still present before conversion")
        .id;
    assert!(fixture.sessions.convert_resident_entity_for_test(entity_id));
    let deceased = fixture
        .execute(
            &mut reopened,
            &demobilize_request("demob-after-death", &handle, record.revision),
        )
        .await;
    assert_eq!(
        deceased.failure(),
        Some(ScriptOperationFailure::NotFound),
        "a converted or dead villager cannot finish demobilization"
    );
    assert_eq!(
        reopened
            .resident_orders()
            .record(&handle)
            .unwrap()
            .assignment,
        crate::script::storage::resident_orders::DurableAssignment::Demobilizing
    );
    assert_eq!(
        gear(&reopened, &handle),
        before,
        "death does not mint a second gear owner"
    );
}

#[tokio::test]
async fn civilian_with_issued_gear_demobilizes_without_a_prior_military_order() {
    let fixture = Fixture::new(false);
    use crate::script::storage::resident_orders::DurableAssignment;

    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 4, Vec3::new(5.5, 64.0, 4.5))
        .await;
    seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_sword", 1)]);
    let before = storage.resident_orders().record(&handle).unwrap().clone();
    assert_eq!(before.assignment, DurableAssignment::Civilian);
    let first = fixture
        .execute(
            &mut storage,
            &demobilize_request("civilian-stop", &handle, before.revision),
        )
        .await;
    assert_eq!(first.failure(), None, "{first:?}");
    let stopped = storage.resident_orders().record(&handle).unwrap().clone();
    assert_eq!(stopped.assignment, DurableAssignment::Demobilizing);
    assert!(stopped.work.is_none());
    assert_eq!(fixture.goal(uuid).await, mc_entity::GoalState::Idle);
    let finished = fixture
        .execute(
            &mut storage,
            &demobilize_request("civilian-finish", &handle, stopped.revision),
        )
        .await;
    assert_eq!(finished.failure(), None, "{finished:?}");
    let restored = storage.resident_orders().record(&handle).unwrap();
    assert_eq!(restored.assignment, DurableAssignment::Civilian);
    assert_eq!(restored.entity_uuid, uuid.to_string());
    assert_eq!(
        gear(&storage, &handle),
        vec![("minecraft:iron_sword".to_owned(), 1)]
    );
}

/// Ambulatory evacuation moves the original wounded patient and medic on the
/// regional physics path; injuries, UUID and gear stay on the original entity.
#[tokio::test]
async fn wounded_patient_and_medic_walk_without_replacing_patient() {
    let fixture = Fixture::new(false);
    use mc_physics::{BlockMaterial, BlockMaterialIds, BlockSampler, EntityBody, PhysicsConfig};

    struct Terrain<'a> {
        chunks: &'a mc_world::WorldReadSnapshot,
        materials: &'a BlockMaterialIds,
    }
    impl BlockSampler for Terrain<'_> {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            self.chunks
                .get_cached_block(BlockPos { x, y, z })
                .map_or(BlockMaterial::Air, |state| self.materials.classify(state.0))
        }
    }
    let mut storage = fixture.storage();
    let (medic, medic_uuid) = fixture
        .resident(&mut storage, 31, Vec3::new(3.5, 64.0, 10.5))
        .await;
    let (patient, patient_uuid) = fixture
        .resident(&mut storage, 32, Vec3::new(4.5, 64.0, 10.5))
        .await;
    seed_gear(
        &mut storage,
        &patient,
        patient_uuid,
        &[("minecraft:emerald", 1)],
    );
    let before_hit = fixture
        .sessions
        .resident_entity_snapshots(&[patient_uuid])
        .await[0]
        .clone()
        .expect("same living patient");
    let patient_id = before_hit.id;
    let injury = fixture
        .sessions
        .commit_resident_damage(
            OWNER,
            vec![crate::play::ResidentAttack {
                uuid: patient_uuid,
                amount: 4.0,
                expected: before_hit,
            }],
        )
        .await;
    assert!(injury[0].is_some());
    let injured_health = fixture.health(patient_uuid).await;
    assert!(injured_health > 0.0 && injured_health < 20.0);
    let mut handles = vec![medic, patient.clone()];
    handles.sort_unstable();
    let result = fixture
        .execute(
            &mut storage,
            &order_request(
                "wounded-escort",
                &handles,
                &[0, 0],
                ScriptResidentOrder::Move {
                    dimension: "minecraft:overworld".to_owned(),
                    anchor: ScriptBlockPosition::new(7, 64, 10),
                    heading_degrees: 0,
                    formation: formation(),
                },
            ),
        )
        .await;
    let (_, outcomes, _) = order_of(&result);
    assert!(
        outcomes
            .iter()
            .all(|member| member.state == ScriptOrderMemberState::Applied),
        "{result:?}"
    );
    for uuid in [medic_uuid, patient_uuid] {
        assert!(
            matches!(
                fixture.goal(uuid).await,
                mc_entity::GoalState::FollowPosition { .. }
            ),
            "both members receive native walking goals without a teleport"
        );
    }
    assert_eq!(
        fixture.position(patient_uuid).await,
        Vec3::new(4.5, 64.0, 10.5)
    );
    let current = fixture
        .sessions
        .resident_entity_snapshots(&[patient_uuid])
        .await[0]
        .clone()
        .expect("patient remains native");
    assert_eq!(current.id, patient_id);
    assert_eq!(fixture.health(patient_uuid).await, injured_health);
    let original_positions = [
        fixture.position(medic_uuid).await,
        fixture.position(patient_uuid).await,
    ];
    let medic_id = fixture
        .sessions
        .resident_entity_snapshots(&[medic_uuid])
        .await[0]
        .as_ref()
        .expect("living medic")
        .id;
    fixture
        .sessions
        .register_loaded_for_server_test("EvacuationObserver", (0, 0));
    let world_read = fixture.world.lock().await.read_view();
    let materials = BlockMaterialIds::new(
        state_of(&fixture.blocks, "air").0,
        Some(state_of(&fixture.blocks, "water").0),
        None,
    );
    let chunks = world_read.snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let terrain = Terrain {
        chunks: &chunks,
        materials: &materials,
    };
    for tick in 1..=30 {
        let queries = fixture
            .sessions
            .tick_entities_and_collect_physics_queries_with_terrain(tick, &world_read, &materials);
        if tick == 1 {
            assert!(
                queries.iter().any(|query| query.id == medic_id)
                    && queries.iter().any(|query| query.id == patient_id),
                "both residents must enter native simulation"
            );
        }
        let steps = queries
            .iter()
            .filter(|query| [medic_id, patient_id].contains(&query.id))
            .map(|query| {
                let stepped = mc_physics::step_entity(
                    EntityBody {
                        position: mc_physics::Vec3::new(
                            query.position.x,
                            query.position.y,
                            query.position.z,
                        ),
                        velocity: mc_physics::Vec3::new(
                            query.velocity.x,
                            query.velocity.y,
                            query.velocity.z,
                        ),
                        aabb: query.aabb,
                        on_ground: query.on_ground,
                    },
                    &terrain,
                    PhysicsConfig::living_entity(),
                );
                EntityPhysicsStep {
                    id: query.id,
                    position: Vec3::new(
                        stepped.body.position.x,
                        stepped.body.position.y,
                        stepped.body.position.z,
                    ),
                    velocity: Vec3::new(
                        stepped.body.velocity.x,
                        stepped.body.velocity.y,
                        stepped.body.velocity.z,
                    ),
                    on_ground: stepped.body.on_ground,
                    horizontal_collision: stepped.horizontal_collision,
                }
            })
            .collect::<Vec<_>>();
        fixture
            .sessions
            .apply_entity_physics_if_current_and_dispatch(tick, &queries, &steps);
    }
    for (uuid, before) in [
        (medic_uuid, original_positions[0]),
        (patient_uuid, original_positions[1]),
    ] {
        let after = fixture.position(uuid).await;
        let dx = after.x - before.x;
        let dz = after.z - before.z;
        assert!(
            dx * dx + dz * dz > 0.04,
            "{uuid} failed to walk from {before:?} to {after:?}"
        );
    }
    assert_eq!(fixture.health(patient_uuid).await, injured_health);
    let reopened = PluginStorage::open(fixture.storage_root.path()).unwrap();
    let same_patient = reopened.resident_orders().record(&patient).unwrap();
    assert_eq!(same_patient.entity_uuid, patient_uuid.to_string());
    assert_eq!(
        gear(&reopened, &patient),
        vec![("minecraft:emerald".to_owned(), 1)]
    );
}

/// The focused refusal shape the batch tests rely on.
#[test]
fn refusals_carry_per_member_reasons() {
    let outcome =
        crate::script::storage::resident_order_execution::resident_order_batch_refusal(&[
            "resident-a".to_owned(),
            "resident-b".to_owned(),
        ]);
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
    let members = refused_members(&outcome);
    assert_eq!(members.len(), 2);
    assert!(
        members
            .iter()
            .all(|member| member.state == ScriptOrderMemberState::StaleRevision)
    );
}

/// Keep the unused import honest: the canonical item stack type is exercised by
/// the seeded gear.
#[test]
fn canonical_item_stack_is_wired_to_the_registry() {
    let items = solaris_required_items();
    let name = codec::Identifier::parse("minecraft:oak_log").unwrap();
    let item_id = items.id_of(&name).expect("oak log item");
    let stack = ItemStack::new(item_id, 3);
    assert_eq!(stack.count, 3);
}
fn refused_members(outcome: &ScriptOperationOutcome) -> Vec<mc_script::ScriptOrderMemberOutcome> {
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!("expected a refusal payload, got {:?}", outcome.payload());
    };
    match &**result {
        ScriptResidentOrderResult::Order { members, .. } => members.clone(),
        other => panic!("expected an order refusal, got {other:?}"),
    }
}
