//! DTO-level acceptance tests for npc work orders and squad orders (C4).

use crate::{
    MAX_NPC_ORDER_HANDLES, ScriptBlockPosition, ScriptEngagementPolicy, ScriptFormation,
    ScriptFormationKind, ScriptHostileCategory, ScriptInventoryEndpoint, ScriptNpcOrder,
    ScriptNpcOrderOperation, ScriptNpcWorkOrder, ScriptOperation, ScriptOperationRequest,
    ScriptOrderTargetRef, ScriptWorkArea,
};

fn area(extent: i32) -> ScriptWorkArea {
    ScriptWorkArea::new(
        "minecraft:overworld".to_owned(),
        ScriptBlockPosition::new(0, 64, 0),
        ScriptBlockPosition::new(extent, 64, 0),
    )
}

fn formation() -> ScriptFormation {
    ScriptFormation::new(ScriptFormationKind::Line, 4)
}

fn issue(order: ScriptNpcOrder) -> Result<ScriptOperationRequest, crate::ScriptDtoError> {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::IssueOrder {
                operation_id: "order-1".to_owned(),
                handles: vec!["a".repeat(32), "b".repeat(32)],
                expected_order_revisions: vec![0, 0],
                order,
            },
        },
    )
}

#[test]
fn work_orders_require_a_concrete_bounded_target() {
    // A harvest whose plot exceeds the bounded work axis is refused.
    let oversized = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::AssignWork {
                operation_id: "work-1".to_owned(),
                handle: "a".repeat(32),
                work: ScriptNpcWorkOrder::Harvest {
                    area: area(64),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                work_units: 16,
                expected_revision: 0,
            },
        },
    );
    assert!(oversized.is_err());
    // A zero-count craft, a non-single-cell station, and a zero work budget are refused.
    let invalid: [ScriptNpcWorkOrder; 3] = [
        ScriptNpcWorkOrder::Craft {
            recipe: "minecraft:stick".to_owned(),
            count: 0,
            station: area(0),
        },
        ScriptNpcWorkOrder::Craft {
            recipe: "minecraft:stick".to_owned(),
            count: 1,
            station: area(1),
        },
        ScriptNpcWorkOrder::Mine {
            area: area(4),
            tool: "minecraft:iron_pickaxe".to_owned(),
        },
    ];
    for work in invalid {
        let request = ScriptOperationRequest::try_new(
            "request",
            ScriptOperation::NpcOrder {
                operation: ScriptNpcOrderOperation::AssignWork {
                    operation_id: "work-2".to_owned(),
                    handle: "a".repeat(32),
                    work: work.clone(),
                    work_units: if matches!(work, ScriptNpcWorkOrder::Craft { .. }) {
                        16
                    } else {
                        0
                    },
                    expected_revision: 0,
                },
            },
        );
        assert!(request.is_err(), "invalid work order {work:?} was accepted");
    }
}

#[test]
fn a_haul_names_either_endpoint_and_one_real_item() {
    // A haul is a move between two distinct endpoints, and the item it takes is
    // a resource id - the same contract every other named resource obeys.
    let endpoints = (
        ScriptInventoryEndpoint::NpcCarry {
            handle: "a".repeat(32),
        },
        ScriptInventoryEndpoint::Container {
            dimension: "minecraft:overworld".to_owned(),
            position: ScriptBlockPosition::new(4, 64, 5),
        },
    );
    let haul = |source: ScriptInventoryEndpoint,
                destination: ScriptInventoryEndpoint,
                item: Option<String>| {
        ScriptOperationRequest::try_new(
            "request",
            ScriptOperation::NpcOrder {
                operation: ScriptNpcOrderOperation::AssignWork {
                    operation_id: "work-3".to_owned(),
                    handle: "a".repeat(32),
                    work: ScriptNpcWorkOrder::Haul {
                        source,
                        destination,
                        item,
                    },
                    work_units: 4,
                    expected_revision: 0,
                },
            },
        )
    };
    // Either direction is a real order, with or without a named item.
    assert!(
        haul(endpoints.0.clone(), endpoints.1.clone(), None).is_ok(),
        "a worker's haul into a bound container is a move core can plan"
    );
    assert!(
        haul(
            endpoints.1.clone(),
            endpoints.0.clone(),
            Some("minecraft:iron_hoe".to_owned())
        )
        .is_ok(),
        "a withdrawal of one named item is a move core can plan"
    );
    // The same endpoint on both sides is not a move, and a name that is not a
    // resource id is not an item.
    assert!(haul(endpoints.0.clone(), endpoints.0.clone(), None).is_err());
    for item in ["Iron_Hoe", "minecraft:", "minecraft:iron hoe", ""] {
        assert!(
            haul(
                endpoints.1.clone(),
                endpoints.0.clone(),
                Some(item.to_owned())
            )
            .is_err(),
            "item {item:?} was accepted"
        );
    }
}

#[test]
fn patrol_and_attack_orders_are_bounded_and_closed() {
    let too_few = issue(ScriptNpcOrder::Patrol {
        waypoints: vec![ScriptBlockPosition::new(0, 64, 0)],
        formation: formation(),
        engagement_radius: 8,
    });
    assert!(too_few.is_err());
    let too_many = issue(ScriptNpcOrder::Patrol {
        waypoints: (0..32)
            .map(|index| ScriptBlockPosition::new(index * 2, 64, 0))
            .collect(),
        formation: formation(),
        engagement_radius: 8,
    });
    assert!(too_many.is_err());
    // Player targets are only hostile when the policy explicitly permits them.
    let policy = ScriptEngagementPolicy::new(1, Vec::new(), vec![ScriptHostileCategory::Hostile]);
    let request = issue(ScriptNpcOrder::Attack {
        targets: vec![ScriptOrderTargetRef::new("t1".to_owned(), 1, 64)],
        policy,
    })
    .expect("valid attack order");
    let ScriptOperation::NpcOrder { operation } = request.operation() else {
        panic!("expected a npc order operation");
    };
    let ScriptNpcOrderOperation::IssueOrder { order, .. } = operation else {
        panic!("expected an issue order");
    };
    let ScriptNpcOrder::Attack { policy, .. } = order else {
        panic!("expected an attack order");
    };
    assert!(!policy.permitted.contains(&ScriptHostileCategory::Player));
    assert!(policy.permitted.contains(&ScriptHostileCategory::Hostile));
}

#[test]
fn order_handles_and_revisions_are_canonicalized_for_idempotency() {
    let request = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::IssueOrder {
                operation_id: "order-2".to_owned(),
                handles: vec!["b".repeat(32), "a".repeat(32)],
                expected_order_revisions: vec![5, 3],
                order: ScriptNpcOrder::Move {
                    dimension: "minecraft:overworld".to_owned(),
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 90,
                    formation: formation(),
                },
            },
        },
    )
    .expect("valid move order");
    let ScriptOperation::NpcOrder { operation } = request.operation() else {
        panic!("expected a npc order operation");
    };
    let ScriptNpcOrderOperation::IssueOrder {
        handles,
        expected_order_revisions,
        ..
    } = operation
    else {
        panic!("expected an issue order");
    };
    assert_eq!(handles[0], "a".repeat(32));
    assert_eq!(expected_order_revisions, &[3, 5]);
    assert_eq!(request.operation_id(), Some("order-2"));
}

#[test]
fn squad_handles_are_bounded_to_the_core_limit() {
    let handles = (0..=MAX_NPC_ORDER_HANDLES)
        .map(|index| format!("{index:0>32}"))
        .collect::<Vec<_>>();
    let request = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::CancelOrder {
                operation_id: "cancel-1".to_owned(),
                expected_order_revisions: vec![0; handles.len()],
                handles,
            },
        },
    );
    assert!(request.is_err());
}

#[test]
fn pending_controls_cannot_replay_completed_work_units() {
    let handle = "a".repeat(32);
    let mut control = crate::ScriptNpcControlSnapshot {
        handle: handle.clone(),
        revision: 9,
        order_revision: 0,
        order: None,
        work: Some(crate::ScriptWorkAssignment::new(
            handle,
            crate::ScriptWorkState::Paused,
            Some(crate::ScriptWorkPauseReason::MissingInput),
            2,
            5,
            Vec::new(),
            9,
        )),
        pending_work: Some(crate::ScriptNpcPendingWork {
            work: ScriptNpcWorkOrder::Craft {
                recipe: "minecraft:stick".to_owned(),
                count: 2,
                station: area(0),
            },
            // The recipe caps this continuation below the remaining budget.
            work_units: 2,
        }),
        death_cargo: None,
        combat: None,
        item_use: None,
        targets: Vec::new(),
    };
    control.validate().expect("bounded remaining craft");
    control.pending_work.as_mut().unwrap().work_units = 4;
    assert!(matches!(
        control.validate(),
        Err(crate::ScriptDtoError::InvalidBounds)
    ));
    control.pending_work.as_mut().unwrap().work_units = 2;
    let work = control.work.as_mut().unwrap();
    work.state = crate::ScriptWorkState::Suspended;
    work.reason = None;
    work.operation_id = Some("craft-1".to_owned());
    work.suspension_reason = Some("consumer-handoff".to_owned());
    control
        .validate()
        .expect("suspended intent retains only its remaining budget");
    control.work = None;
    assert!(matches!(
        control.validate(),
        Err(crate::ScriptDtoError::InvalidBounds)
    ));
}

#[test]
fn suspended_work_requires_original_unfinished_intent_without_a_due_tick() {
    let mut work = crate::ScriptWorkAssignment::new(
        "a".repeat(32),
        crate::ScriptWorkState::Suspended,
        None,
        2,
        5,
        Vec::new(),
        9,
    );
    work.operation_id = Some("craft-1".to_owned());
    work.suspension_reason = Some("consumer-handoff".to_owned());
    work.validate().expect("bounded opaque owner suspension");

    let invalid_assignment = Err(crate::ScriptDtoError::InconsistentResult {
        field: "work assignment",
    });
    work.due_tick = Some(10);
    assert_eq!(work.validate(), invalid_assignment);
    work.due_tick = None;
    work.operation_id = None;
    assert_eq!(work.validate(), invalid_assignment);
    work.operation_id = Some("craft-1".to_owned());
    work.work_units_done = 5;
    assert_eq!(work.validate(), invalid_assignment);
    work.work_units_done = 2;
    work.reason = Some(crate::ScriptWorkPauseReason::MissingInput);
    assert_eq!(work.validate(), invalid_assignment);
    work.reason = None;
    work.suspension_reason = None;
    assert_eq!(work.validate(), invalid_assignment);
    work.suspension_reason = Some("consumer-handoff".to_owned());
    work.state = crate::ScriptWorkState::Running;
    assert_eq!(work.validate(), invalid_assignment);
    work.state = crate::ScriptWorkState::Suspended;
    work.suspension_reason = Some("Consumer Handoff".to_owned());
    assert!(
        work.validate().is_err(),
        "owner reasons obey the bounded script-id grammar"
    );
    work.suspension_reason = Some("x".repeat(64));
    work.validate()
        .expect("the maximum opaque reason remains admissible");
    work.suspension_reason = Some("x".repeat(65));
    assert!(
        work.validate().is_err(),
        "oversized reasons cannot enter native records"
    );
}
