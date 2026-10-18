use super::*;
use serde_json::{Value, json};

fn block_transaction_oracle_json() -> Value {
    json!({
        "schema": "solaris.block_transaction.oracle.v1",
        "id": "block-transaction-26-1-2",
        "phases": [
            {"id":"accepted-break","case":"accepted_break","sequence":1},
            {"id":"accepted-place","case":"accepted_place","sequence":2},
            {"id":"occupied-place-rejection","case":"occupied_place_rejection","sequence":3},
            {"id":"out-of-reach-break-rejection","case":"out_of_reach_break_rejection","sequence":4},
            {"id":"early-stop-break-rejection","case":"early_stop_break_rejection","sequence":5}
        ]
    })
}

#[test]
fn block_transaction_oracle_manifest_requires_complete_unique_case_matrix() {
    let manifest =
        BlockTransactionOracleManifest::from_json(&block_transaction_oracle_json().to_string())
            .expect("valid block transaction oracle manifest");
    assert_eq!(manifest.phases.len(), 5);
    assert_eq!(
        BlockTransactionOracleManifest::from_json(
            &manifest.to_pretty_json().expect("encode manifest")
        )
        .expect("decode manifest"),
        manifest
    );

    let mut duplicate_case = block_transaction_oracle_json();
    duplicate_case["phases"][4]["case"] = json!("accepted_break");
    assert!(BlockTransactionOracleManifest::from_json(&duplicate_case.to_string()).is_err());

    let mut duplicate_sequence = block_transaction_oracle_json();
    duplicate_sequence["phases"][4]["sequence"] = json!(4);
    assert!(BlockTransactionOracleManifest::from_json(&duplicate_sequence.to_string()).is_err());

    let mut unknown_field = block_transaction_oracle_json();
    unknown_field["phases"][0]["expected_state"] = json!(0);
    assert!(BlockTransactionOracleManifest::from_json(&unknown_field.to_string()).is_err());
}

#[test]
fn block_transaction_trace_requires_authoritative_update_before_matching_ack() {
    let manifest =
        BlockTransactionOracleManifest::from_json(&block_transaction_oracle_json().to_string())
            .expect("valid manifest");
    let phases = manifest
        .phases
        .iter()
        .map(|phase| BlockTransactionOraclePhaseTrace {
            id: phase.id.clone(),
            events: vec![
                BlockTransactionOracleEvent::TargetUpdate { state_id: 1 },
                BlockTransactionOracleEvent::Ack {
                    sequence: phase.sequence,
                },
            ],
        })
        .collect();
    let mut trace = BlockTransactionOracleTrace {
        manifest_id: manifest.id.clone(),
        phases,
    };
    trace.phases[0].events = vec![BlockTransactionOracleEvent::Ack { sequence: 1 }];
    trace.phases[1].events = vec![BlockTransactionOracleEvent::Ack { sequence: 2 }];
    trace
        .validate_against(&manifest)
        .expect("accepted phases may rely on prediction while rejections resync before ack");

    let mut ack_first = trace.clone();
    ack_first.phases[2].events.swap(0, 1);
    assert!(ack_first.validate_against(&manifest).is_err());

    let mut wrong_ack = trace;
    wrong_ack.phases[3].events[1] = BlockTransactionOracleEvent::Ack { sequence: 99 };
    assert!(wrong_ack.validate_against(&manifest).is_err());
}

fn container_state_oracle_json() -> Value {
    json!({
        "schema": "solaris.container_state.oracle.v1",
        "id": "inventory-container-26-1-2",
        "phases": [
            {"id":"chest-initial","menu":"chest","case":"chest_initial"},
            {"id":"chest-quick-move-in","menu":"chest","case":"chest_quick_move_in"},
            {"id":"chest-quick-move-out","menu":"chest","case":"chest_quick_move_out"},
            {"id":"chest-stale-click","menu":"chest","case":"chest_stale_click"},
            {"id":"chest-reopen","menu":"chest","case":"chest_reopen"},
            {"id":"craft-initial","menu":"crafting_table","case":"craft_initial"},
            {"id":"craft-prepared","menu":"crafting_table","case":"craft_prepared"},
            {"id":"craft-quick-move","menu":"crafting_table","case":"craft_quick_move"},
            {"id":"craft-stale-click","menu":"crafting_table","case":"craft_stale_click"},
            {"id":"craft-reopen","menu":"crafting_table","case":"craft_reopen"}
        ]
    })
}

fn empty_container_snapshot(menu: ContainerStateOracleMenu) -> ContainerStateOracleSnapshot {
    ContainerStateOracleSnapshot {
        menu,
        state_id_delta: 0,
        slots: vec![ContainerStateOracleSlot {
            slot: 0,
            stack: ContainerStateOracleStack {
                item_id: 0,
                count: 0,
            },
        }],
        cursor: ContainerStateOracleStack {
            item_id: 0,
            count: 0,
        },
    }
}

#[test]
fn container_state_oracle_manifest_requires_complete_menu_case_matrix() {
    let manifest =
        ContainerStateOracleManifest::from_json(&container_state_oracle_json().to_string())
            .expect("valid container state oracle manifest");
    assert_eq!(manifest.phases.len(), 10);
    assert_eq!(
        ContainerStateOracleManifest::from_json(
            &manifest
                .to_pretty_json()
                .expect("encode container manifest")
        )
        .expect("decode container manifest"),
        manifest
    );

    let mut wrong_menu = container_state_oracle_json();
    wrong_menu["phases"][0]["menu"] = json!("crafting_table");
    assert!(ContainerStateOracleManifest::from_json(&wrong_menu.to_string()).is_err());

    let mut duplicate_case = container_state_oracle_json();
    duplicate_case["phases"][9]["case"] = json!("craft_initial");
    assert!(ContainerStateOracleManifest::from_json(&duplicate_case.to_string()).is_err());

    let mut unknown_field = container_state_oracle_json();
    unknown_field["phases"][0]["expected_state_id"] = json!(0);
    assert!(ContainerStateOracleManifest::from_json(&unknown_field.to_string()).is_err());
}

#[test]
fn container_state_trace_rejects_unsorted_slots_and_nonzero_reopen_baseline() {
    let manifest =
        ContainerStateOracleManifest::from_json(&container_state_oracle_json().to_string())
            .expect("valid container manifest");
    let phases = manifest
        .phases
        .iter()
        .map(|phase| ContainerStateOraclePhaseTrace {
            id: phase.id.clone(),
            snapshot: empty_container_snapshot(phase.menu),
        })
        .collect();
    let trace = ContainerStateOracleTrace {
        manifest_id: manifest.id.clone(),
        phases,
    };
    trace.validate_against(&manifest).expect("valid trace");

    let mut unsorted = trace.clone();
    unsorted.phases[1].snapshot.slots = vec![
        ContainerStateOracleSlot {
            slot: 2,
            stack: ContainerStateOracleStack {
                item_id: 1,
                count: 1,
            },
        },
        ContainerStateOracleSlot {
            slot: 1,
            stack: ContainerStateOracleStack {
                item_id: 1,
                count: 1,
            },
        },
    ];
    assert!(unsorted.validate_against(&manifest).is_err());

    let mut bad_reopen = trace;
    bad_reopen.phases[9].snapshot.state_id_delta = 1;
    assert!(bad_reopen.validate_against(&manifest).is_err());
}

fn core_gate_manifest_json() -> Value {
    json!({
        "schema": "solaris.core_gate.manifest.v1",
        "phases": [
            {
                "id": "solid-edit",
                "scenario_id": "m94-02a-solid-place-break-drop",
                "ledger_rows": ["B1"],
                "evidence_legs": ["unit", "wire", "real_client"]
            },
            {
                "id": "rejected-resync",
                "scenario_id": "m94-02b-rejected-block-resync",
                "ledger_rows": ["B1"],
                "evidence_legs": ["wire", "oracle", "replay_negative"]
            }
        ],
        "rows": [{
            "row": "B1",
            "scope": "broad",
            "required_phases": ["solid-edit", "rejected-resync"],
            "required_evidence_legs": [
                "unit", "wire", "oracle", "real_client", "replay_negative"
            ]
        }]
    })
}

#[test]
fn core_gate_manifest_accepts_focused_phase_and_evidence_matrix() {
    let manifest = CoreGateManifest::from_json(&core_gate_manifest_json().to_string())
        .expect("valid core gate manifest");
    manifest
        .validate_completion(&[
            CoreGatePhaseEvidence {
                phase_id: "solid-edit".to_owned(),
                passed_evidence_legs: [
                    CoreGateEvidenceLeg::Unit,
                    CoreGateEvidenceLeg::Wire,
                    CoreGateEvidenceLeg::RealClient,
                ]
                .into_iter()
                .collect(),
            },
            CoreGatePhaseEvidence {
                phase_id: "rejected-resync".to_owned(),
                passed_evidence_legs: [
                    CoreGateEvidenceLeg::Wire,
                    CoreGateEvidenceLeg::Oracle,
                    CoreGateEvidenceLeg::ReplayNegative,
                ]
                .into_iter()
                .collect(),
            },
        ])
        .expect("complete focused evidence matrix");
}

#[test]
fn broad_core_gate_row_rejects_single_phase_manifest() {
    let mut manifest = core_gate_manifest_json();
    manifest["rows"][0]["required_phases"] = json!(["solid-edit"]);
    CoreGateManifest::from_json(&manifest.to_string())
        .expect_err("broad row must require multiple focused phases");
}

#[test]
fn core_gate_completion_rejects_missing_focused_phase() {
    let manifest = CoreGateManifest::from_json(&core_gate_manifest_json().to_string())
        .expect("valid core gate manifest");
    manifest
        .validate_completion(&[CoreGatePhaseEvidence {
            phase_id: "solid-edit".to_owned(),
            passed_evidence_legs: [
                CoreGateEvidenceLeg::Unit,
                CoreGateEvidenceLeg::Wire,
                CoreGateEvidenceLeg::RealClient,
            ]
            .into_iter()
            .collect(),
        }])
        .expect_err("missing focused phase must reject broad row completion");
}

#[test]
fn core_gate_completion_rejects_missing_declared_evidence() {
    let manifest = CoreGateManifest::from_json(&core_gate_manifest_json().to_string())
        .expect("valid core gate manifest");
    manifest
        .validate_completion(&[
            CoreGatePhaseEvidence {
                phase_id: "solid-edit".to_owned(),
                passed_evidence_legs: [CoreGateEvidenceLeg::Unit, CoreGateEvidenceLeg::Wire]
                    .into_iter()
                    .collect(),
            },
            CoreGatePhaseEvidence {
                phase_id: "rejected-resync".to_owned(),
                passed_evidence_legs: [
                    CoreGateEvidenceLeg::Wire,
                    CoreGateEvidenceLeg::Oracle,
                    CoreGateEvidenceLeg::ReplayNegative,
                ]
                .into_iter()
                .collect(),
            },
        ])
        .expect_err("missing phase evidence must reject completion");
}

fn scenario_json() -> Value {
    json!({
        "schema": "solaris.core_replay.scenario.v1",
        "id": "core-actions-seed-81",
        "seed": 81,
        "actions": [
            { "type": "wait_ticks", "ticks": 2 },
            { "type": "move_by", "dx_cm": 100, "dz_cm": -50 },
            { "type": "look", "yaw_deg": 90, "pitch_deg": 0 },
            { "type": "reconnect" }
        ],
        "lanes": [
            {
                "driver": "solaris_protocol",
                "required_gates": [
                    { "id": "protocol-session", "evidence_kind": "harness" }
                ]
            },
            {
                "driver": "vanilla_oracle",
                "required_gates": [
                    { "id": "oracle-comparison", "evidence_kind": "oracle" }
                ]
            },
            {
                "driver": "real_client",
                "required_gates": [
                    { "id": "real-client-observation", "evidence_kind": "real_client" }
                ]
            }
        ],
        "expected_invariants": [
            {
                "id": "post-action-liveness",
                "description": "The client remains responsive after the ordered actions."
            },
            {
                "id": "deterministic-normalized-state",
                "description": "Repeated Solaris runs produce the same normalized observations."
            }
        ]
    })
}

fn result_json() -> Value {
    json!({
        "schema": "solaris.core_replay.result.v1",
        "scenario_id": "core-actions-seed-81",
        "seed": 81,
        "driver": "solaris_protocol",
        "outcome": "passed",
        "actions": [
            { "type": "wait_ticks", "ticks": 2 },
            { "type": "move_by", "dx_cm": 100, "dz_cm": -50 },
            { "type": "look", "yaw_deg": 90, "pitch_deg": 0 },
            { "type": "reconnect" }
        ],
        "provenance": {
            "git_commit": "0123456789abcdef0123456789abcdef01234567",
            "config_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "build_profile": "debug",
            "sidecar_version": "embedded:26.1.2",
            "hardware": {
                "os": "linux",
                "arch": "x86_64",
                "cpu_model": "fixture-cpu",
                "logical_cpus": 8,
                "memory_mib": 16384
            }
        },
        "gates": [
            {
                "id": "protocol-session",
                "evidence_kind": "harness",
                "status": "passed",
                "artifacts": [".analysis/core-replay/protocol-session.json"]
            }
        ],
        "invariants": [
            { "id": "post-action-liveness", "status": "passed" },
            { "id": "deterministic-normalized-state", "status": "passed" }
        ],
        "observations": [
            {
                "subject": "solaris",
                "phase": "core-actions-seed-81",
                "facts": [
                    { "type": "note", "key": "post_action_liveness", "value": "true" }
                ]
            }
        ]
    })
}

fn concurrent_scenario_json() -> Value {
    json!({
        "schema": "solaris.core_replay.scenario.v1",
        "id": "multiplayer-transactions-seed-8102",
        "seed": 8102,
        "actions": [],
        "concurrent_groups": [
            {
                "id": "same-target-placement",
                "repetitions": 8,
                "fixture": { "type": "same_target_placement" },
                "actions": [
                    {
                        "type": "place_block",
                        "actor": "dirt-player",
                        "item": "minecraft:dirt"
                    },
                    {
                        "type": "place_block",
                        "actor": "stone-player",
                        "item": "minecraft:stone"
                    }
                ]
            }
        ],
        "state_expectations": [
            {
                "id": "placement-state",
                "after_group": "same-target-placement",
                "invariant_id": "placement-conservation",
                "values": [
                    { "key": "rounds", "value": 8 },
                    { "key": "authoritative_blocks", "value": 8 },
                    { "key": "consumed_items", "value": 8 },
                    { "key": "conservation_failures", "value": 0 }
                ]
            }
        ],
        "lanes": [
            {
                "driver": "solaris_protocol",
                "required_gates": [
                    { "id": "concurrent-protocol-session", "evidence_kind": "harness" }
                ]
            }
        ],
        "expected_invariants": [
            {
                "id": "placement-conservation",
                "description": "Every concurrent placement round commits one block and consumes one item."
            },
            {
                "id": "deterministic-normalized-state",
                "description": "Repeated runs produce the same symmetric conservation observations."
            }
        ]
    })
}

fn concurrent_result_json() -> Value {
    let scenario = concurrent_scenario_json();
    json!({
        "schema": "solaris.core_replay.result.v1",
        "scenario_id": "multiplayer-transactions-seed-8102",
        "seed": 8102,
        "driver": "solaris_protocol",
        "outcome": "passed",
        "actions": [],
        "concurrent_groups": scenario["concurrent_groups"].clone(),
        "state_observations": [
            {
                "id": "placement-state",
                "values": scenario["state_expectations"][0]["values"].clone()
            }
        ],
        "provenance": {
            "git_commit": "0123456789abcdef0123456789abcdef01234567",
            "config_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "build_profile": "debug",
            "sidecar_version": "embedded:26.1.2",
            "hardware": {
                "os": "linux",
                "arch": "x86_64",
                "cpu_model": "fixture-cpu",
                "logical_cpus": 8,
                "memory_mib": 16384
            }
        },
        "gates": [
            {
                "id": "concurrent-protocol-session",
                "evidence_kind": "harness",
                "status": "passed",
                "artifacts": ["target/replay-results/multiplayer-transactions-seed-8102.json"]
            }
        ],
        "invariants": [
            { "id": "placement-conservation", "status": "passed" },
            { "id": "deterministic-normalized-state", "status": "passed" }
        ],
        "observations": [
            {
                "subject": "solaris",
                "phase": "multiplayer-transactions-seed-8102",
                "facts": [
                    { "type": "note", "key": "conservation", "value": "passed" }
                ]
            }
        ]
    })
}

#[test]
fn minimal_complete_scenario_parses() {
    let input = serde_json::to_string(&scenario_json()).expect("scenario json");
    let scenario = ReplayScenarioManifest::from_json(&input).expect("valid scenario");

    assert_eq!(scenario.seed, 81);
    assert_eq!(scenario.actions.len(), 4);
    assert_eq!(scenario.lanes.len(), 3);
    assert_eq!(scenario.expected_invariants.len(), 2);
}

#[test]
fn concurrent_scenario_parses_without_serial_actions() {
    let input = serde_json::to_string(&concurrent_scenario_json()).expect("scenario json");

    ReplayScenarioManifest::from_json(&input).expect("valid concurrent scenario");
}

#[test]
fn concurrent_result_cross_validates_state_observations() {
    let scenario = ReplayScenarioManifest::from_json(&concurrent_scenario_json().to_string())
        .expect("valid concurrent scenario");
    let result = ReplayRunResult::from_json(&concurrent_result_json().to_string())
        .expect("valid concurrent result");

    result
        .validate_against(&scenario)
        .expect("concurrent result matches scenario");
}

#[test]
fn concurrent_result_cannot_report_mismatched_state_as_passed() {
    let scenario = ReplayScenarioManifest::from_json(&concurrent_scenario_json().to_string())
        .expect("valid concurrent scenario");
    let mut value = concurrent_result_json();
    let consumed = value["state_observations"][0]["values"]
        .as_array_mut()
        .expect("state values")
        .iter_mut()
        .find(|value| value["key"] == "consumed_items")
        .expect("consumed item value");
    consumed["value"] = json!(7);

    let falsely_passed = ReplayRunResult::from_json(&value.to_string())
        .expect("standalone result is structurally valid");
    assert!(
        falsely_passed.validate_against(&scenario).is_err(),
        "scenario cross-validation must reject false conservation pass"
    );

    value["outcome"] = json!("failed");
    value["invariants"][0]["status"] = json!("failed");
    value["invariants"][0]["reason"] = json!("consumed_items expected 8, observed 7");
    let recorded_failure = ReplayRunResult::from_json(&value.to_string())
        .expect("failed replay result is structurally valid");
    recorded_failure
        .validate_against(&scenario)
        .expect("failed conservation result remains replayable evidence");
}

#[test]
fn concurrent_failure_manifest_keeps_only_the_failing_group_contract() {
    let mut value = concurrent_scenario_json();
    value["concurrent_groups"]
        .as_array_mut()
        .expect("concurrent groups")
        .push(json!({
            "id": "other-placement",
            "repetitions": 1,
            "fixture": { "type": "same_target_placement" },
            "actions": [
                {
                    "type": "place_block",
                    "actor": "other-a",
                    "item": "minecraft:dirt"
                },
                {
                    "type": "place_block",
                    "actor": "other-b",
                    "item": "minecraft:stone"
                }
            ]
        }));
    value["state_expectations"]
        .as_array_mut()
        .expect("state expectations")
        .push(json!({
            "id": "other-state",
            "after_group": "other-placement",
            "invariant_id": "other-conservation",
            "values": [{ "key": "rounds", "value": 1 }]
        }));
    value["expected_invariants"]
        .as_array_mut()
        .expect("expected invariants")
        .push(json!({
            "id": "other-conservation",
            "description": "The unrelated group conserves its state."
        }));
    let scenario =
        ReplayScenarioManifest::from_json(&value.to_string()).expect("multi-group scenario parses");

    let failure = scenario
        .minimal_concurrent_failure("same-target-placement")
        .expect("shrink failing concurrent group");

    assert_eq!(failure.seed, scenario.seed);
    assert_eq!(failure.concurrent_groups.len(), 1);
    assert_eq!(failure.concurrent_groups[0].id, "same-target-placement");
    assert_eq!(failure.state_expectations.len(), 1);
    assert_eq!(failure.state_expectations[0].id, "placement-state");
    assert_eq!(failure.expected_invariants.len(), 1);
    assert_eq!(failure.expected_invariants[0].id, "placement-conservation");
    failure
        .validate()
        .expect("minimal failure remains replayable");
}

#[test]
fn concurrent_scenario_accepts_shared_chest_group() {
    let mut value = concurrent_scenario_json();
    value["concurrent_groups"]
        .as_array_mut()
        .expect("concurrent groups")
        .push(json!({
            "id": "shared-chest-pickup",
            "repetitions": 1,
            "fixture": {
                "type": "shared_chest",
                "item": "minecraft:dirt",
                "initial_count": 2
            },
            "actions": [
                { "type": "chest_pickup", "actor": "left-player", "slot": 0 },
                { "type": "chest_pickup", "actor": "right-player", "slot": 0 }
            ]
        }));
    value["state_expectations"]
        .as_array_mut()
        .expect("state expectations")
        .push(json!({
            "id": "chest-state",
            "after_group": "shared-chest-pickup",
            "invariant_id": "chest-conservation",
            "values": [
                { "key": "container_items", "value": 1 },
                { "key": "cursor_items", "value": 1 },
                { "key": "total_items", "value": 2 },
                { "key": "winning_cursors", "value": 1 }
            ]
        }));
    value["expected_invariants"]
        .as_array_mut()
        .expect("expected invariants")
        .push(json!({
            "id": "chest-conservation",
            "description": "One shared chest click commits and the seeded stack is conserved."
        }));

    ReplayScenarioManifest::from_json(&value.to_string())
        .expect("valid shared chest concurrent group");
}

#[test]
fn concurrent_contract_rejects_group_shapes_the_executor_cannot_replay() {
    let checked = include_str!(
        "../../../tools/core-replay-scenarios/multiplayer-transactions-seed-8102.json"
    );

    let mut three_contenders: Value = serde_json::from_str(checked).expect("checked JSON");
    three_contenders["concurrent_groups"][0]["actions"]
        .as_array_mut()
        .expect("placement actions")
        .push(json!({
            "type": "place_block",
            "actor": "third-player",
            "item": "minecraft:cobblestone"
        }));
    assert!(
        ReplayScenarioManifest::from_json(&three_contenders.to_string()).is_err(),
        "same-target fixture currently supports exactly two contenders"
    );

    let mut repeated_chest: Value = serde_json::from_str(checked).expect("checked JSON");
    repeated_chest["concurrent_groups"][1]["repetitions"] = json!(2);
    assert!(
        ReplayScenarioManifest::from_json(&repeated_chest.to_string()).is_err(),
        "shared chest fixture currently represents one state-id race"
    );

    let mut split_slots: Value = serde_json::from_str(checked).expect("checked JSON");
    split_slots["concurrent_groups"][1]["actions"][1]["slot"] = json!(1);
    assert!(
        ReplayScenarioManifest::from_json(&split_slots.to_string()).is_err(),
        "shared chest contenders must target the same slot"
    );
}

#[test]
fn scenario_rejects_unknown_fields_and_actions() {
    let mut unknown_field = scenario_json();
    unknown_field["unexpected"] = json!(true);
    assert!(
        ReplayScenarioManifest::from_json(&unknown_field.to_string()).is_err(),
        "unknown scenario fields must fail closed"
    );

    let mut unknown_action = scenario_json();
    unknown_action["actions"][0]["type"] = json!("teleport");
    assert!(
        ReplayScenarioManifest::from_json(&unknown_action.to_string()).is_err(),
        "unknown action variants must fail closed"
    );
}

#[test]
fn scenario_rejects_empty_required_collections() {
    for field in ["actions", "lanes", "expected_invariants"] {
        let mut value = scenario_json();
        value[field] = json!([]);
        assert!(
            ReplayScenarioManifest::from_json(&value.to_string()).is_err(),
            "empty {field} must fail"
        );
    }

    let mut no_lane_gates = scenario_json();
    no_lane_gates["lanes"][0]["required_gates"] = json!([]);
    assert!(
        ReplayScenarioManifest::from_json(&no_lane_gates.to_string()).is_err(),
        "every lane must name its required gates"
    );

    let mut wrong_primary_evidence = scenario_json();
    wrong_primary_evidence["lanes"][1]["required_gates"][0]["evidence_kind"] = json!("harness");
    assert!(
        ReplayScenarioManifest::from_json(&wrong_primary_evidence.to_string()).is_err(),
        "oracle and real-client lanes must require their own evidence kind"
    );
}

#[test]
fn scenario_rejects_compact_manifest_with_rows_but_no_evidence_legs() {
    let mut value = scenario_json();
    value["ledger_rows"] = json!(["Q1", "Q2", "Q3"]);
    assert!(
        ReplayScenarioManifest::from_json(&value.to_string()).is_err(),
        "scenario with ledger rows but no evidence legs must fail"
    );
}

#[test]
fn scenario_rejects_compact_manifest_with_unknown_evidence_leg() {
    let mut value = scenario_json();
    value["ledger_rows"] = json!(["Q1"]);
    value["evidence_legs"] = json!(["protocol-session", "unknown-leg"]);
    assert!(
        ReplayScenarioManifest::from_json(&value.to_string()).is_err(),
        "scenario evidence legs must reference declared required gates"
    );
}

#[test]
fn scenario_rejects_compact_manifest_with_row_leg_count_mismatch() {
    let mut value = scenario_json();
    value["ledger_rows"] = json!(["Q1", "Q2"]);
    value["evidence_legs"] = json!([
        "protocol-session",
        "oracle-comparison",
        "real-client-observation"
    ]);
    assert!(
        ReplayScenarioManifest::from_json(&value.to_string()).is_err(),
        "scenario row and evidence leg counts must match"
    );
}

#[test]
fn scenario_accepts_compact_manifest_with_rows_and_legs() {
    let mut value = scenario_json();
    value["ledger_rows"] = json!(["Q1", "Q2", "Q3"]);
    value["evidence_legs"] = json!([
        "protocol-session",
        "oracle-comparison",
        "real-client-observation"
    ]);

    ReplayScenarioManifest::from_json(&value.to_string())
        .expect("compact scenario with compact ledger/evidence manifest");
}

#[test]
fn result_requires_provenance_and_every_lane_gate() {
    let mut missing_provenance = result_json();
    missing_provenance
        .as_object_mut()
        .expect("result object")
        .remove("provenance");
    assert!(ReplayRunResult::from_json(&missing_provenance.to_string()).is_err());

    let scenario =
        ReplayScenarioManifest::from_json(&scenario_json().to_string()).expect("valid scenario");
    let mut missing_gate = result_json();
    missing_gate["gates"] = json!([]);
    assert!(ReplayRunResult::from_json(&missing_gate.to_string()).is_err());

    let mut missing_invariants = result_json();
    missing_invariants["invariants"] = json!([]);
    assert!(ReplayRunResult::from_json(&missing_invariants.to_string()).is_err());

    let mut wrong_gate = result_json();
    wrong_gate["gates"][0]["id"] = json!("unrequired-gate");
    let result = ReplayRunResult::from_json(&wrong_gate.to_string())
        .expect("non-empty standalone gate is structurally valid");
    assert!(result.validate_against(&scenario).is_err());
}

#[test]
fn result_requires_reasons_for_every_non_passing_check() {
    let mut value = result_json();
    value["outcome"] = json!("degraded");
    value["gates"][0]["status"] = json!("skipped");
    assert!(ReplayRunResult::from_json(&value.to_string()).is_err());

    value["gates"][0]["reason"] = json!("local sidecar was not installed");
    assert!(ReplayRunResult::from_json(&value.to_string()).is_ok());
}

#[test]
fn result_rejects_falsely_passed_aggregate_outcome() {
    let mut value = result_json();
    value["invariants"][0]["status"] = json!("failed");
    value["invariants"][0]["reason"] = json!("client stopped responding");
    assert!(ReplayRunResult::from_json(&value.to_string()).is_err());
}

#[test]
fn result_must_match_scenario_seed_actions_and_lane() {
    let scenario =
        ReplayScenarioManifest::from_json(&scenario_json().to_string()).expect("valid scenario");

    for mutation in ["seed", "actions", "driver"] {
        let mut value = result_json();
        match mutation {
            "seed" => value["seed"] = json!(82),
            "actions" => value["actions"].as_array_mut().expect("actions").reverse(),
            "driver" => value["driver"] = json!("vanilla_oracle"),
            _ => unreachable!(),
        }
        match ReplayRunResult::from_json(&value.to_string()) {
            Ok(result) => assert!(
                result.validate_against(&scenario).is_err(),
                "{mutation} mismatch must fail"
            ),
            Err(err) => panic!("{mutation} should parse before cross-validation: {err:#}"),
        }
    }
}

#[test]
fn checked_core_action_fixture_is_strict_and_complete() {
    let fixture = include_str!("../../../tools/core-replay-scenarios/core-actions-seed-81.json");
    let scenario = ReplayScenarioManifest::from_json(fixture).expect("checked fixture parses");

    assert_eq!(scenario.id, "core-actions-seed-81");
    assert_eq!(scenario.seed, 81);
    assert_eq!(scenario.actions.len(), 4);
    assert_eq!(
        scenario
            .lanes
            .iter()
            .map(|lane| lane.driver)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            ReplayDriver::SolarisProtocol,
            ReplayDriver::VanillaOracle,
            ReplayDriver::RealClient,
        ])
    );
    assert_eq!(scenario.expected_invariants.len(), 2);
    assert!(
        scenario
            .lanes
            .iter()
            .all(|lane| !lane.required_gates.is_empty())
    );
    let canonical = scenario
        .to_pretty_json()
        .expect("serialize checked fixture");
    assert_eq!(
        ReplayScenarioManifest::from_json(&canonical).expect("parse canonical fixture"),
        scenario
    );

    let mut wrong_schema: Value = serde_json::from_str(fixture).expect("fixture JSON");
    wrong_schema["schema"] = json!("solaris.core_replay.scenario.v2");
    assert!(ReplayScenarioManifest::from_json(&wrong_schema.to_string()).is_err());

    let mut missing_seed: Value = serde_json::from_str(fixture).expect("fixture JSON");
    missing_seed
        .as_object_mut()
        .expect("fixture object")
        .remove("seed");
    assert!(ReplayScenarioManifest::from_json(&missing_seed.to_string()).is_err());
}

#[test]
fn checked_multiplayer_transaction_fixture_is_strict_and_complete() {
    let fixture = include_str!(
        "../../../tools/core-replay-scenarios/multiplayer-transactions-seed-8102.json"
    );
    let scenario = ReplayScenarioManifest::from_json(fixture)
        .expect("checked multiplayer transaction fixture parses");

    assert_eq!(scenario.id, "multiplayer-transactions-seed-8102");
    assert_eq!(scenario.seed, 8102);
    assert!(scenario.actions.is_empty());
    assert_eq!(scenario.concurrent_groups.len(), 2);
    assert_eq!(scenario.state_expectations.len(), 2);
    assert_eq!(scenario.lanes.len(), 1);
    assert_eq!(scenario.lanes[0].driver, ReplayDriver::SolarisProtocol);
    assert_eq!(scenario.expected_invariants.len(), 3);
    let canonical = scenario
        .to_pretty_json()
        .expect("serialize checked multiplayer fixture");
    assert_eq!(
        ReplayScenarioManifest::from_json(&canonical).expect("parse canonical multiplayer fixture"),
        scenario
    );
}
