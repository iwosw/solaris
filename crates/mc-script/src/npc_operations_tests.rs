use crate::{
    MAX_NPC_QUERY_HANDLES, ScriptDtoError, ScriptNpcLifecycle, ScriptNpcLoadedState,
    ScriptNpcOperation, ScriptNpcResult, ScriptNpcSnapshot, ScriptOperation,
    ScriptOperationPayload, ScriptOperationRequest,
};

const UUID: &str = "12345678-1234-5678-1234-567812345678";

fn adopt(operation_id: &str) -> ScriptNpcOperation {
    ScriptNpcOperation::Adopt {
        operation_id: operation_id.to_owned(),
        actor_id: 7,
        entity_uuid: UUID.to_owned(),
        expected_entity_revision: 0,
    }
}

#[test]
fn query_handles_are_bounded_sorted_and_deduplicated() {
    let handles = vec!["b2".to_owned(), "a1".to_owned(), "a1".to_owned()];
    let request = ScriptOperationRequest::try_new(
        "query",
        ScriptOperation::Npc {
            operation: ScriptNpcOperation::Query {
                handles,
                cursor: None,
            },
        },
    )
    .unwrap();
    let ScriptOperation::Npc {
        operation: ScriptNpcOperation::Query { handles, .. },
    } = request.operation()
    else {
        panic!("npc query envelope");
    };
    assert_eq!(handles, &["a1".to_owned(), "b2".to_owned()]);

    let oversized = (0..=MAX_NPC_QUERY_HANDLES)
        .map(|index| format!("h{index:02}"))
        .collect::<Vec<_>>();
    assert!(matches!(
        ScriptOperationRequest::try_new(
            "query",
            ScriptOperation::Npc {
                operation: ScriptNpcOperation::Query {
                    handles: oversized,
                    cursor: None,
                },
            },
        ),
        Err(ScriptDtoError::TooManyEntries {
            field: "npc query handles",
            max: MAX_NPC_QUERY_HANDLES,
        })
    ));
}

#[test]
fn npc_mutations_require_ids_and_physical_identity_fences() {
    for rejected in [
        adopt("Operation"),
        adopt(&"x".repeat(crate::MAX_SCRIPT_ID_BYTES + 1)),
        ScriptNpcOperation::Adopt {
            operation_id: "op".to_owned(),
            actor_id: 0,
            entity_uuid: UUID.to_owned(),
            expected_entity_revision: 0,
        },
        ScriptNpcOperation::Adopt {
            operation_id: "op".to_owned(),
            actor_id: 7,
            entity_uuid: "not-a-uuid".to_owned(),
            expected_entity_revision: 0,
        },
        ScriptNpcOperation::Spawn {
            operation_id: "op".to_owned(),
            generation_id: String::new(),
            position: crate::ScriptPosition::try_new(2.0, 64.0, 3.0).unwrap(),
            entity_type: "minecraft:villager".to_owned(),
        },
        ScriptNpcOperation::Release {
            operation_id: "op".to_owned(),
            handle: "r1".to_owned(),
            expected_revision: crate::MAX_SCRIPT_WORLD_TIME + 1,
        },
    ] {
        assert!(
            matches!(
                ScriptOperationRequest::try_new(
                    "request",
                    ScriptOperation::Npc {
                        operation: rejected.clone(),
                    },
                ),
                Err(ScriptDtoError::InvalidId { .. }
                    | ScriptDtoError::InvalidBounds
                    | ScriptDtoError::EmptyValue { .. }
                    | ScriptDtoError::ValueTooLong { .. })
            ),
            "accepted {rejected:?}"
        );
    }
}

#[test]
fn npc_snapshots_bind_lifecycle_to_loaded_state() {
    let record = ScriptNpcSnapshot::new(
        "a1".to_owned(),
        UUID.to_owned(),
        ScriptNpcLifecycle::AliveLoaded,
        9,
        None,
        Some(ScriptNpcLoadedState::new(
            "minecraft:overworld".to_owned(),
            crate::ScriptPosition::try_new(1.0, 64.0, -2.0).unwrap(),
            20.0,
            20.0,
            0.0,
            Vec::new(),
        )),
    );
    record.validate().unwrap();
    let mut inconsistent = record.clone();
    inconsistent.loaded = None;
    assert!(matches!(
        inconsistent.validate(),
        Err(ScriptDtoError::InconsistentResult { .. })
    ));
    let unloaded = ScriptNpcSnapshot::new(
        "a1".to_owned(),
        UUID.to_owned(),
        ScriptNpcLifecycle::AliveUnloaded,
        9,
        None,
        None,
    );
    unloaded.validate().unwrap();
    let mut nan = record;
    nan.loaded = Some(ScriptNpcLoadedState::new(
        "minecraft:overworld".to_owned(),
        crate::ScriptPosition::try_new(0.0, 0.0, 0.0).unwrap(),
        f32::NAN,
        20.0,
        0.0,
        Vec::new(),
    ));
    assert!(matches!(nan.validate(), Err(ScriptDtoError::InvalidBounds)));

    assert!(matches!(
        crate::ScriptOperationOutcome::committed(
            4,
            ScriptOperationPayload::Npc {
                result: Box::new(ScriptNpcResult::Page {
                    npcs: vec![unloaded.clone(), unloaded],
                    cursor: None,
                }),
            },
        ),
        Err(ScriptDtoError::InconsistentResult { .. })
    ));
}

#[test]
fn npc_spawn_codec_enforces_native_coordinate_bounds() {
    let position =
        crate::ScriptPosition::try_new(-0.0, -64.25, crate::SCRIPT_HORIZONTAL_COORDINATE_LIMIT)
            .unwrap();
    let operation = ScriptNpcOperation::Spawn {
        operation_id: "spawn-position".to_owned(),
        generation_id: "native-fixture".to_owned(),
        position,
        entity_type: "minecraft:villager".to_owned(),
    };
    let mut wire = serde_json::to_value(&operation).unwrap();
    assert_eq!(wire["position"]["x"], serde_json::json!(-0.0));
    assert_eq!(wire["position"]["y"], serde_json::json!(-64.25));
    assert_eq!(
        serde_json::from_value::<ScriptNpcOperation>(wire.clone()).unwrap(),
        operation,
    );
    for (axis, limit) in [
        ("x", crate::SCRIPT_HORIZONTAL_COORDINATE_LIMIT),
        ("y", crate::SCRIPT_VERTICAL_COORDINATE_LIMIT),
        ("z", crate::SCRIPT_HORIZONTAL_COORDINATE_LIMIT),
    ] {
        for value in [limit + 1.0, -limit - 1.0] {
            let mut invalid = wire.clone();
            invalid["position"][axis] = serde_json::json!(value);
            assert!(serde_json::from_value::<ScriptNpcOperation>(invalid).is_err());
        }
    }
    wire["position"]["x_bits"] = serde_json::json!(0);
    assert!(serde_json::from_value::<ScriptNpcOperation>(wire).is_err());
}
