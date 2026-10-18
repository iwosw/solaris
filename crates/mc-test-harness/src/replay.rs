use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::parity::{
    CoreAction, ObservationSet, ScenarioContext, ServerKind, observe_core_action_sequence,
};

pub const REPLAY_SCENARIO_SCHEMA: &str = "solaris.core_replay.scenario.v1";
pub const REPLAY_RESULT_SCHEMA: &str = "solaris.core_replay.result.v1";
pub const CORE_GATE_MANIFEST_SCHEMA: &str = "solaris.core_gate.manifest.v1";
pub const BLOCK_TRANSACTION_ORACLE_SCHEMA: &str = "solaris.block_transaction.oracle.v1";
pub const CONTAINER_STATE_ORACLE_SCHEMA: &str = "solaris.container_state.oracle.v1";

const MAX_REPLAY_ACTIONS: usize = 10_000;
const MAX_REPLAY_CHECKS: usize = 128;
const MAX_CONCURRENT_GROUP_REPETITIONS: u16 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockTransactionOracleCase {
    AcceptedBreak,
    AcceptedPlace,
    OccupiedPlaceRejection,
    OutOfReachBreakRejection,
    EarlyStopBreakRejection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionOraclePhase {
    pub id: String,
    pub case: BlockTransactionOracleCase,
    pub sequence: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionOracleManifest {
    pub schema: String,
    pub id: String,
    pub phases: Vec<BlockTransactionOraclePhase>,
}

impl BlockTransactionOracleManifest {
    pub fn from_json(input: &str) -> Result<Self> {
        let manifest: Self =
            serde_json::from_str(input).context("parse block transaction oracle manifest JSON")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_pretty_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .context("serialize block transaction oracle manifest JSON")
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == BLOCK_TRANSACTION_ORACLE_SCHEMA,
            "unsupported block transaction oracle schema: {}",
            self.schema
        );
        validate_identifier("block transaction oracle id", &self.id)?;
        ensure!(
            self.phases.len() == 5,
            "block transaction oracle must declare exactly five focused phases"
        );
        let mut ids = BTreeSet::new();
        let mut sequences = BTreeSet::new();
        let mut cases = BTreeSet::new();
        for phase in &self.phases {
            validate_identifier("block transaction phase id", &phase.id)?;
            ensure!(
                ids.insert(phase.id.as_str()),
                "duplicate block transaction phase id: {}",
                phase.id
            );
            ensure!(
                phase.sequence > 0,
                "block transaction phase {} has non-positive sequence",
                phase.id
            );
            ensure!(
                sequences.insert(phase.sequence),
                "duplicate block transaction sequence: {}",
                phase.sequence
            );
            ensure!(
                cases.insert(phase.case),
                "duplicate block transaction oracle case: {:?}",
                phase.case
            );
        }
        let required = BTreeSet::from([
            BlockTransactionOracleCase::AcceptedBreak,
            BlockTransactionOracleCase::AcceptedPlace,
            BlockTransactionOracleCase::OccupiedPlaceRejection,
            BlockTransactionOracleCase::OutOfReachBreakRejection,
            BlockTransactionOracleCase::EarlyStopBreakRejection,
        ]);
        ensure!(
            cases == required,
            "block transaction oracle phases do not cover the required case matrix"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockTransactionOracleEvent {
    TargetUpdate { state_id: i32 },
    Ack { sequence: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionOracleTrace {
    pub manifest_id: String,
    pub phases: Vec<BlockTransactionOraclePhaseTrace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionOraclePhaseTrace {
    pub id: String,
    pub events: Vec<BlockTransactionOracleEvent>,
}

impl BlockTransactionOracleTrace {
    pub fn validate_against(&self, manifest: &BlockTransactionOracleManifest) -> Result<()> {
        manifest.validate()?;
        ensure!(
            self.manifest_id == manifest.id,
            "block transaction trace manifest id {} does not match {}",
            self.manifest_id,
            manifest.id
        );
        ensure!(
            self.phases.len() == manifest.phases.len(),
            "block transaction trace phase count does not match manifest"
        );
        for (expected, actual) in manifest.phases.iter().zip(&self.phases) {
            ensure!(
                actual.id == expected.id,
                "block transaction trace phase order/id mismatch: expected {}, got {}",
                expected.id,
                actual.id
            );
            ensure!(
                !actual.events.is_empty(),
                "block transaction trace phase {} has no events",
                actual.id
            );
            let ack_index = actual
                .events
                .iter()
                .position(|event| matches!(event, BlockTransactionOracleEvent::Ack { sequence } if *sequence == expected.sequence))
                .with_context(|| format!("block transaction phase {} has no matching ack", actual.id))?;
            if matches!(
                expected.case,
                BlockTransactionOracleCase::OccupiedPlaceRejection
            ) {
                ensure!(
                    actual.events[..ack_index].iter().any(|event| matches!(
                        event,
                        BlockTransactionOracleEvent::TargetUpdate { .. }
                    )),
                    "rejected block transaction phase {} has no authoritative resync before ack",
                    actual.id
                );
            }
            ensure!(
                actual.events[ack_index + 1..]
                    .iter()
                    .all(|event| !matches!(event, BlockTransactionOracleEvent::Ack { sequence } if *sequence == expected.sequence)),
                "block transaction phase {} repeats its ack",
                actual.id
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerStateOracleMenu {
    Chest,
    CraftingTable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerStateOracleCase {
    ChestInitial,
    ChestQuickMoveIn,
    ChestStaleClick,
    ChestQuickMoveOut,
    ChestReopen,
    CraftInitial,
    CraftPrepared,
    CraftQuickMove,
    CraftStaleClick,
    CraftReopen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerStateOraclePhase {
    pub id: String,
    pub menu: ContainerStateOracleMenu,
    pub case: ContainerStateOracleCase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerStateOracleManifest {
    pub schema: String,
    pub id: String,
    pub phases: Vec<ContainerStateOraclePhase>,
}

impl ContainerStateOracleManifest {
    pub fn from_json(input: &str) -> Result<Self> {
        let manifest: Self =
            serde_json::from_str(input).context("parse container state oracle manifest JSON")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_pretty_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string_pretty(self).context("serialize container state oracle manifest JSON")
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == CONTAINER_STATE_ORACLE_SCHEMA,
            "unsupported container state oracle schema: {}",
            self.schema
        );
        validate_identifier("container state oracle id", &self.id)?;
        ensure!(
            self.phases.len() == 10,
            "container state oracle must declare exactly ten focused phases"
        );
        let mut ids = BTreeSet::new();
        let mut cases = BTreeSet::new();
        for phase in &self.phases {
            validate_identifier("container state phase id", &phase.id)?;
            ensure!(
                ids.insert(phase.id.as_str()),
                "duplicate container state phase id: {}",
                phase.id
            );
            ensure!(
                cases.insert(phase.case),
                "duplicate container state oracle case: {:?}",
                phase.case
            );
            let expected_menu = match phase.case {
                ContainerStateOracleCase::ChestInitial
                | ContainerStateOracleCase::ChestQuickMoveIn
                | ContainerStateOracleCase::ChestStaleClick
                | ContainerStateOracleCase::ChestQuickMoveOut
                | ContainerStateOracleCase::ChestReopen => ContainerStateOracleMenu::Chest,
                ContainerStateOracleCase::CraftInitial
                | ContainerStateOracleCase::CraftPrepared
                | ContainerStateOracleCase::CraftQuickMove
                | ContainerStateOracleCase::CraftStaleClick
                | ContainerStateOracleCase::CraftReopen => ContainerStateOracleMenu::CraftingTable,
            };
            ensure!(
                phase.menu == expected_menu,
                "container state phase {} has menu {:?}, expected {:?}",
                phase.id,
                phase.menu,
                expected_menu
            );
        }
        let required = BTreeSet::from([
            ContainerStateOracleCase::ChestInitial,
            ContainerStateOracleCase::ChestQuickMoveIn,
            ContainerStateOracleCase::ChestStaleClick,
            ContainerStateOracleCase::ChestQuickMoveOut,
            ContainerStateOracleCase::ChestReopen,
            ContainerStateOracleCase::CraftInitial,
            ContainerStateOracleCase::CraftPrepared,
            ContainerStateOracleCase::CraftQuickMove,
            ContainerStateOracleCase::CraftStaleClick,
            ContainerStateOracleCase::CraftReopen,
        ]);
        ensure!(
            cases == required,
            "container state oracle phases do not cover the required case matrix"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerStateOracleStack {
    pub item_id: u32,
    pub count: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerStateOracleSlot {
    pub slot: u16,
    pub stack: ContainerStateOracleStack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerStateOracleSnapshot {
    pub menu: ContainerStateOracleMenu,
    pub state_id_delta: i32,
    pub slots: Vec<ContainerStateOracleSlot>,
    pub cursor: ContainerStateOracleStack,
}

impl ContainerStateOracleSnapshot {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.state_id_delta >= 0,
            "container state snapshot has negative state-id delta"
        );
        ensure!(
            self.cursor.count >= 0,
            "container state snapshot has negative cursor count"
        );
        let mut previous = None;
        for slot in &self.slots {
            ensure!(slot.stack.count >= 0, "container slot has negative count");
            ensure!(
                previous.is_none_or(|previous| slot.slot > previous),
                "container slots must be strictly sorted and unique"
            );
            previous = Some(slot.slot);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerStateOraclePhaseTrace {
    pub id: String,
    pub snapshot: ContainerStateOracleSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerStateOracleTrace {
    pub manifest_id: String,
    pub phases: Vec<ContainerStateOraclePhaseTrace>,
}

impl ContainerStateOracleTrace {
    pub fn validate_against(&self, manifest: &ContainerStateOracleManifest) -> Result<()> {
        manifest.validate()?;
        ensure!(
            self.manifest_id == manifest.id,
            "container state trace manifest id {} does not match {}",
            self.manifest_id,
            manifest.id
        );
        ensure!(
            self.phases.len() == manifest.phases.len(),
            "container state trace phase count does not match manifest"
        );
        for (expected, actual) in manifest.phases.iter().zip(&self.phases) {
            ensure!(
                actual.id == expected.id,
                "container state trace phase mismatch: expected {}, got {}",
                expected.id,
                actual.id
            );
            ensure!(
                actual.snapshot.menu == expected.menu,
                "container state trace phase {} has wrong menu",
                actual.id
            );
            actual.snapshot.validate()?;
        }
        for index in [0_usize, 4, 5, 9] {
            ensure!(
                self.phases[index].snapshot.state_id_delta == 0,
                "initial/reopen phase {} must normalize state-id to zero",
                self.phases[index].id
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayDriver {
    SolarisProtocol,
    VanillaOracle,
    RealClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayEvidenceKind {
    Unit,
    Harness,
    Oracle,
    RealClient,
    Performance,
    Soak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayCheckStatus {
    Passed,
    Failed,
    Degraded,
    Blocked,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayOutcome {
    Passed,
    Failed,
    Degraded,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreGateEvidenceLeg {
    Unit,
    Wire,
    Oracle,
    RealClient,
    Performance,
    Persistence,
    ReplayNegative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreGateRowScope {
    Focused,
    Broad,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreGatePhase {
    pub id: String,
    pub scenario_id: String,
    pub ledger_rows: Vec<String>,
    pub evidence_legs: BTreeSet<CoreGateEvidenceLeg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreGateRow {
    pub row: String,
    pub scope: CoreGateRowScope,
    pub required_phases: Vec<String>,
    pub required_evidence_legs: BTreeSet<CoreGateEvidenceLeg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreGatePhaseEvidence {
    pub phase_id: String,
    pub passed_evidence_legs: BTreeSet<CoreGateEvidenceLeg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreGateManifest {
    pub schema: String,
    pub phases: Vec<CoreGatePhase>,
    pub rows: Vec<CoreGateRow>,
}

impl CoreGateManifest {
    pub fn from_json(input: &str) -> Result<Self> {
        let manifest: Self =
            serde_json::from_str(input).context("parse core gate manifest JSON")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == CORE_GATE_MANIFEST_SCHEMA,
            "unsupported core gate manifest schema: {}",
            self.schema
        );
        ensure!(
            !self.phases.is_empty(),
            "core gate manifest phases are empty"
        );
        ensure!(
            self.phases.len() <= MAX_REPLAY_CHECKS,
            "core gate manifest has too many phases"
        );
        ensure!(!self.rows.is_empty(), "core gate manifest rows are empty");
        ensure!(
            self.rows.len() <= MAX_REPLAY_CHECKS,
            "core gate manifest has too many rows"
        );

        let mut phases_by_id = BTreeMap::new();
        for phase in &self.phases {
            validate_identifier("core gate phase id", &phase.id)?;
            validate_identifier("core gate scenario id", &phase.scenario_id)?;
            ensure!(
                phases_by_id.insert(phase.id.as_str(), phase).is_none(),
                "duplicate core gate phase id: {}",
                phase.id
            );
            ensure!(
                !phase.ledger_rows.is_empty(),
                "core gate phase {} has no ledger rows",
                phase.id
            );
            ensure!(
                !phase.evidence_legs.is_empty(),
                "core gate phase {} has no evidence legs",
                phase.id
            );
            let mut phase_rows = BTreeSet::new();
            for row in &phase.ledger_rows {
                validate_ledger_row(row)?;
                ensure!(
                    phase_rows.insert(row.as_str()),
                    "core gate phase {} repeats ledger row {}",
                    phase.id,
                    row
                );
            }
        }

        let mut row_ids = BTreeSet::new();
        for row in &self.rows {
            validate_ledger_row(&row.row)?;
            ensure!(
                row_ids.insert(row.row.as_str()),
                "duplicate core gate ledger row: {}",
                row.row
            );
            ensure!(
                !row.required_phases.is_empty(),
                "core gate row {} has no required phases",
                row.row
            );
            if row.scope == CoreGateRowScope::Broad {
                ensure!(
                    row.required_phases.len() >= 2,
                    "broad core gate row {} requires at least two focused phases",
                    row.row
                );
            }
            ensure!(
                !row.required_evidence_legs.is_empty(),
                "core gate row {} has no required evidence legs",
                row.row
            );
            let mut required_phase_ids = BTreeSet::new();
            for phase_id in &row.required_phases {
                ensure!(
                    required_phase_ids.insert(phase_id.as_str()),
                    "core gate row {} repeats required phase {}",
                    row.row,
                    phase_id
                );
                let phase = phases_by_id.get(phase_id.as_str()).with_context(|| {
                    format!(
                        "core gate row {} requires unknown phase {phase_id}",
                        row.row
                    )
                })?;
                ensure!(
                    phase
                        .ledger_rows
                        .iter()
                        .any(|candidate| candidate == &row.row),
                    "core gate phase {} does not cover required row {}",
                    phase.id,
                    row.row
                );
            }
        }
        Ok(())
    }

    pub fn validate_completion(&self, completed: &[CoreGatePhaseEvidence]) -> Result<()> {
        self.validate()?;
        let phases_by_id = self
            .phases
            .iter()
            .map(|phase| (phase.id.as_str(), phase))
            .collect::<BTreeMap<_, _>>();
        let mut completed_by_id = BTreeMap::new();
        for evidence in completed {
            ensure!(
                phases_by_id.contains_key(evidence.phase_id.as_str()),
                "completion references unknown core gate phase {}",
                evidence.phase_id
            );
            ensure!(
                completed_by_id
                    .insert(evidence.phase_id.as_str(), evidence)
                    .is_none(),
                "duplicate core gate completion for phase {}",
                evidence.phase_id
            );
        }

        for row in &self.rows {
            let mut row_evidence = BTreeSet::new();
            for phase_id in &row.required_phases {
                let phase = phases_by_id
                    .get(phase_id.as_str())
                    .expect("validated core gate phase exists");
                let evidence = completed_by_id.get(phase_id.as_str()).with_context(|| {
                    format!(
                        "core gate row {} is missing focused phase {phase_id}",
                        row.row
                    )
                })?;
                ensure!(
                    evidence
                        .passed_evidence_legs
                        .is_superset(&phase.evidence_legs),
                    "core gate phase {} is missing declared evidence legs",
                    phase.id
                );
                row_evidence.extend(evidence.passed_evidence_legs.iter().copied());
            }
            ensure!(
                row_evidence.is_superset(&row.required_evidence_legs),
                "core gate row {} is missing required evidence legs",
                row.row
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayGateRequirement {
    pub id: String,
    pub evidence_kind: ReplayEvidenceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayLane {
    pub driver: ReplayDriver,
    pub required_gates: Vec<ReplayGateRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayExpectedInvariant {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplayConcurrentFixture {
    SameTargetPlacement,
    SharedChest { item: String, initial_count: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplayConcurrentAction {
    PlaceBlock { actor: String, item: String },
    ChestPickup { actor: String, slot: u8 },
}

impl ReplayConcurrentAction {
    fn actor(&self) -> &str {
        match self {
            Self::PlaceBlock { actor, .. } | Self::ChestPickup { actor, .. } => actor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayConcurrentGroup {
    pub id: String,
    pub repetitions: u16,
    pub fixture: ReplayConcurrentFixture,
    pub actions: Vec<ReplayConcurrentAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayStateValue {
    pub key: String,
    pub value: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayStateExpectation {
    pub id: String,
    pub after_group: String,
    pub invariant_id: String,
    pub values: Vec<ReplayStateValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayStateObservation {
    pub id: String,
    pub values: Vec<ReplayStateValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayScenarioManifest {
    pub schema: String,
    pub id: String,
    pub seed: u64,
    pub actions: Vec<CoreAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concurrent_groups: Vec<ReplayConcurrentGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_expectations: Vec<ReplayStateExpectation>,
    pub lanes: Vec<ReplayLane>,
    pub expected_invariants: Vec<ReplayExpectedInvariant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ledger_rows: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_legs: Vec<String>,
}

impl ReplayScenarioManifest {
    pub fn from_json(input: &str) -> Result<Self> {
        let manifest: Self =
            serde_json::from_str(input).context("parse core replay scenario JSON")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_pretty_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string_pretty(self).context("serialize core replay scenario JSON")
    }

    pub fn minimal_concurrent_failure(&self, group_id: &str) -> Result<Self> {
        self.validate()?;
        let group = self
            .concurrent_groups
            .iter()
            .find(|group| group.id == group_id)
            .with_context(|| format!("unknown failing concurrent group: {group_id}"))?
            .clone();
        let state_expectations = self
            .state_expectations
            .iter()
            .filter(|expectation| expectation.after_group == group_id)
            .cloned()
            .collect::<Vec<_>>();
        ensure!(
            !state_expectations.is_empty(),
            "failing concurrent group {group_id} has no state expectation"
        );
        let invariant_ids = state_expectations
            .iter()
            .map(|expectation| expectation.invariant_id.as_str())
            .collect::<BTreeSet<_>>();
        let expected_invariants = self
            .expected_invariants
            .iter()
            .filter(|invariant| invariant_ids.contains(invariant.id.as_str()))
            .cloned()
            .collect();
        let failure = Self {
            schema: self.schema.clone(),
            id: self.id.clone(),
            seed: self.seed,
            actions: self.actions.clone(),
            concurrent_groups: vec![group],
            state_expectations,
            lanes: self.lanes.clone(),
            expected_invariants,
            ledger_rows: Vec::new(),
            evidence_legs: Vec::new(),
        };
        failure.validate()?;
        Ok(failure)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == REPLAY_SCENARIO_SCHEMA,
            "unsupported replay scenario schema: {}",
            self.schema
        );
        validate_identifier("scenario id", &self.id)?;
        ensure!(
            !self.actions.is_empty() || !self.concurrent_groups.is_empty(),
            "replay scenario has no serial actions or concurrent groups"
        );
        ensure!(
            self.actions.len()
                + self
                    .concurrent_groups
                    .iter()
                    .map(|group| group.actions.len() * usize::from(group.repetitions))
                    .sum::<usize>()
                <= MAX_REPLAY_ACTIONS,
            "replay scenario expands beyond {MAX_REPLAY_ACTIONS} actions"
        );
        for action in &self.actions {
            validate_action(action)?;
        }

        let group_ids = validate_concurrent_groups(&self.concurrent_groups)?;

        ensure!(!self.lanes.is_empty(), "replay scenario lanes are empty");
        let mut required_gate_ids = BTreeSet::new();
        let mut drivers = BTreeSet::new();
        for lane in &self.lanes {
            ensure!(
                drivers.insert(lane.driver),
                "duplicate replay lane for {:?}",
                lane.driver
            );
            ensure!(
                !lane.required_gates.is_empty(),
                "replay lane {:?} has no required gates",
                lane.driver
            );
            ensure!(
                lane.required_gates.len() <= MAX_REPLAY_CHECKS,
                "replay lane {:?} has too many required gates",
                lane.driver
            );
            let mut gate_ids = BTreeSet::new();
            for gate in &lane.required_gates {
                validate_identifier("required gate id", &gate.id)?;
                ensure!(
                    gate_ids.insert(gate.id.as_str()),
                    "duplicate required gate id in {:?}: {}",
                    lane.driver,
                    gate.id
                );
                required_gate_ids.insert(gate.id.as_str());
            }
            let primary_evidence = match lane.driver {
                ReplayDriver::SolarisProtocol => ReplayEvidenceKind::Harness,
                ReplayDriver::VanillaOracle => ReplayEvidenceKind::Oracle,
                ReplayDriver::RealClient => ReplayEvidenceKind::RealClient,
            };
            ensure!(
                lane.required_gates
                    .iter()
                    .any(|gate| gate.evidence_kind == primary_evidence),
                "replay lane {:?} does not require {:?} evidence",
                lane.driver,
                primary_evidence
            );
        }

        let mut rows = BTreeSet::new();
        for row in &self.ledger_rows {
            validate_ledger_row(row)?;
            ensure!(rows.insert(row.as_str()), "duplicate ledger row {row}");
        }
        ensure!(
            rows.len() <= MAX_REPLAY_CHECKS,
            "replay scenario has too many ledger rows"
        );

        let mut evidence_legs = BTreeSet::new();
        for evidence in &self.evidence_legs {
            validate_identifier("evidence leg", evidence)?;
            ensure!(
                required_gate_ids.contains(evidence.as_str()),
                "scenario evidence leg {evidence} is not declared by a replay lane"
            );
            ensure!(
                evidence_legs.insert(evidence.as_str()),
                "duplicate evidence leg {evidence}"
            );
        }
        if self.ledger_rows.is_empty() {
            ensure!(
                self.evidence_legs.is_empty(),
                "scenario evidence legs provided without ledger rows"
            );
        } else {
            ensure!(
                !self.evidence_legs.is_empty(),
                "scenario ledger rows provided without evidence legs"
            );
            ensure!(
                self.ledger_rows.len() == self.evidence_legs.len(),
                "scenario evidence legs count does not match ledger rows"
            );
        }

        ensure!(
            !self.expected_invariants.is_empty(),
            "replay scenario expected invariants are empty"
        );
        ensure!(
            self.expected_invariants.len() <= MAX_REPLAY_CHECKS,
            "replay scenario has too many expected invariants"
        );
        let mut invariant_ids = BTreeSet::new();
        for invariant in &self.expected_invariants {
            validate_identifier("expected invariant id", &invariant.id)?;
            validate_non_empty("expected invariant description", &invariant.description)?;
            ensure!(
                invariant_ids.insert(invariant.id.as_str()),
                "duplicate expected invariant id: {}",
                invariant.id
            );
        }

        if self.concurrent_groups.is_empty() {
            ensure!(
                self.state_expectations.is_empty(),
                "state expectations require concurrent groups"
            );
        } else {
            ensure!(
                !self.state_expectations.is_empty(),
                "concurrent replay scenario has no state expectations"
            );
        }
        ensure!(
            self.state_expectations.len() <= MAX_REPLAY_CHECKS,
            "replay scenario has too many state expectations"
        );
        let mut expectation_ids = BTreeSet::new();
        let mut expectation_invariants = BTreeSet::new();
        let mut covered_groups = BTreeSet::new();
        for expectation in &self.state_expectations {
            validate_identifier("state expectation id", &expectation.id)?;
            ensure!(
                expectation_ids.insert(expectation.id.as_str()),
                "duplicate state expectation id: {}",
                expectation.id
            );
            validate_identifier("state expectation group id", &expectation.after_group)?;
            ensure!(
                group_ids.contains(expectation.after_group.as_str()),
                "state expectation {} references unknown group {}",
                expectation.id,
                expectation.after_group
            );
            covered_groups.insert(expectation.after_group.as_str());
            validate_identifier("state expectation invariant id", &expectation.invariant_id)?;
            ensure!(
                invariant_ids.contains(expectation.invariant_id.as_str()),
                "state expectation {} references unknown invariant {}",
                expectation.id,
                expectation.invariant_id
            );
            ensure!(
                expectation_invariants.insert(expectation.invariant_id.as_str()),
                "state invariant {} is reused by multiple expectations",
                expectation.invariant_id
            );
            ensure!(
                !expectation.values.is_empty(),
                "state expectation {} has no values",
                expectation.id
            );
            ensure!(
                expectation.values.len() <= MAX_REPLAY_CHECKS,
                "state expectation {} has too many values",
                expectation.id
            );
            let mut value_keys = BTreeSet::new();
            for value in &expectation.values {
                validate_identifier("state expectation value key", &value.key)?;
                ensure!(
                    value_keys.insert(value.key.as_str()),
                    "state expectation {} repeats value key {}",
                    expectation.id,
                    value.key
                );
            }
        }
        ensure!(
            covered_groups.len() == group_ids.len(),
            "every concurrent group must have a state expectation"
        );
        Ok(())
    }
}

pub async fn run_protocol_replay(
    manifest: &ReplayScenarioManifest,
    context: ScenarioContext,
) -> Result<ObservationSet> {
    manifest.validate()?;
    let (driver, lane_name) = match context.kind {
        ServerKind::Solaris => (ReplayDriver::SolarisProtocol, "solaris protocol lane"),
        ServerKind::Vanilla => (ReplayDriver::VanillaOracle, "vanilla oracle lane"),
    };
    ensure!(
        manifest.lanes.iter().any(|lane| lane.driver == driver),
        "replay scenario {} has no {lane_name}",
        manifest.id
    );
    ensure!(
        manifest.concurrent_groups.is_empty(),
        "single-client protocol replay adapter does not support concurrent groups"
    );
    ensure!(
        !manifest
            .actions
            .iter()
            .any(|action| matches!(action, CoreAction::Reconnect)),
        "protocol replay adapter does not support reconnect actions"
    );
    observe_core_action_sequence(context, &manifest.id, &manifest.actions).await
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayHardwareProvenance {
    pub os: String,
    pub arch: String,
    pub cpu_model: String,
    pub logical_cpus: usize,
    pub memory_mib: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayProvenance {
    pub git_commit: String,
    pub config_sha256: String,
    pub build_profile: String,
    pub sidecar_version: String,
    pub hardware: ReplayHardwareProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayGateResult {
    pub id: String,
    pub evidence_kind: ReplayEvidenceKind,
    pub status: ReplayCheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayInvariantResult {
    pub id: String,
    pub status: ReplayCheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRunResult {
    pub schema: String,
    pub scenario_id: String,
    pub seed: u64,
    pub driver: ReplayDriver,
    pub outcome: ReplayOutcome,
    pub actions: Vec<CoreAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concurrent_groups: Vec<ReplayConcurrentGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_observations: Vec<ReplayStateObservation>,
    pub provenance: ReplayProvenance,
    pub gates: Vec<ReplayGateResult>,
    pub invariants: Vec<ReplayInvariantResult>,
    pub observations: Vec<ObservationSet>,
}

impl ReplayRunResult {
    pub fn from_json(input: &str) -> Result<Self> {
        let result: Self = serde_json::from_str(input).context("parse core replay result JSON")?;
        result.validate()?;
        Ok(result)
    }

    pub fn to_pretty_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string_pretty(self).context("serialize core replay result JSON")
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == REPLAY_RESULT_SCHEMA,
            "unsupported replay result schema: {}",
            self.schema
        );
        validate_identifier("result scenario id", &self.scenario_id)?;
        ensure!(
            !self.actions.is_empty() || !self.concurrent_groups.is_empty(),
            "replay result has no serial actions or concurrent groups"
        );
        ensure!(
            self.actions.len()
                + self
                    .concurrent_groups
                    .iter()
                    .map(|group| group.actions.len() * usize::from(group.repetitions))
                    .sum::<usize>()
                <= MAX_REPLAY_ACTIONS,
            "replay result expands beyond {MAX_REPLAY_ACTIONS} actions"
        );
        for action in &self.actions {
            validate_action(action)?;
        }
        validate_concurrent_groups(&self.concurrent_groups)?;
        if self.concurrent_groups.is_empty() {
            ensure!(
                self.state_observations.is_empty(),
                "state observations require concurrent groups"
            );
        } else {
            ensure!(
                !self.state_observations.is_empty(),
                "concurrent replay result has no state observations"
            );
        }
        ensure!(
            self.state_observations.len() <= MAX_REPLAY_CHECKS,
            "replay result has too many state observations"
        );
        let mut state_observation_ids = BTreeSet::new();
        for observation in &self.state_observations {
            validate_identifier("state observation id", &observation.id)?;
            ensure!(
                state_observation_ids.insert(observation.id.as_str()),
                "duplicate state observation id: {}",
                observation.id
            );
            validate_state_values("state observation", &observation.id, &observation.values)?;
        }
        validate_provenance(&self.provenance)?;
        ensure!(!self.gates.is_empty(), "replay result gates are empty");
        ensure!(
            self.gates.len() <= MAX_REPLAY_CHECKS,
            "replay result has too many gates"
        );
        ensure!(
            !self.invariants.is_empty(),
            "replay result invariants are empty"
        );
        ensure!(
            self.invariants.len() <= MAX_REPLAY_CHECKS,
            "replay result has too many invariants"
        );

        let mut gate_ids = BTreeSet::new();
        for gate in &self.gates {
            validate_identifier("gate result id", &gate.id)?;
            ensure!(
                gate_ids.insert(gate.id.as_str()),
                "duplicate gate result id: {}",
                gate.id
            );
            validate_check_reason(gate.status, gate.reason.as_deref(), "gate", &gate.id)?;
            if gate.status == ReplayCheckStatus::Passed {
                ensure!(
                    !gate.artifacts.is_empty(),
                    "passed gate {} has no evidence artifacts",
                    gate.id
                );
            }
            for artifact in &gate.artifacts {
                validate_relative_artifact_path(artifact)?;
            }
        }

        let mut invariant_ids = BTreeSet::new();
        for invariant in &self.invariants {
            validate_identifier("invariant result id", &invariant.id)?;
            ensure!(
                invariant_ids.insert(invariant.id.as_str()),
                "duplicate invariant result id: {}",
                invariant.id
            );
            validate_check_reason(
                invariant.status,
                invariant.reason.as_deref(),
                "invariant",
                &invariant.id,
            )?;
        }

        let derived = derive_outcome(
            self.gates
                .iter()
                .map(|gate| gate.status)
                .chain(self.invariants.iter().map(|invariant| invariant.status)),
        )?;
        ensure!(
            self.outcome == derived,
            "replay result outcome {:?} does not match check outcome {:?}",
            self.outcome,
            derived
        );

        if self.outcome == ReplayOutcome::Passed {
            ensure!(
                !self.observations.is_empty(),
                "passed replay result has no observations"
            );
        }
        let mut observation_keys = BTreeSet::new();
        for observation in &self.observations {
            validate_non_empty("observation subject", &observation.subject)?;
            validate_non_empty("observation phase", &observation.phase)?;
            ensure!(
                !observation.facts().is_empty(),
                "observation {}/{} has no facts",
                observation.subject,
                observation.phase
            );
            ensure!(
                observation_keys.insert((observation.subject.as_str(), observation.phase.as_str())),
                "duplicate observation subject/phase: {}/{}",
                observation.subject,
                observation.phase
            );
        }
        Ok(())
    }

    pub fn validate_against(&self, scenario: &ReplayScenarioManifest) -> Result<()> {
        scenario.validate()?;
        self.validate()?;
        ensure!(
            self.scenario_id == scenario.id,
            "result scenario id {} does not match {}",
            self.scenario_id,
            scenario.id
        );
        ensure!(
            self.seed == scenario.seed,
            "result seed {} does not match {}",
            self.seed,
            scenario.seed
        );
        ensure!(
            self.actions == scenario.actions,
            "result action order does not match scenario"
        );
        ensure!(
            self.concurrent_groups == scenario.concurrent_groups,
            "result concurrent groups do not match scenario"
        );

        ensure!(
            self.state_observations.len() == scenario.state_expectations.len(),
            "result state observation count {} does not match expected count {}",
            self.state_observations.len(),
            scenario.state_expectations.len()
        );
        for expected in &scenario.state_expectations {
            let actual = self
                .state_observations
                .iter()
                .find(|observation| observation.id == expected.id)
                .with_context(|| format!("missing state observation: {}", expected.id))?;
            let expected_values = state_values_map(&expected.values);
            let actual_values = state_values_map(&actual.values);
            ensure!(
                expected_values.keys().eq(actual_values.keys()),
                "state observation {} keys do not match scenario",
                expected.id
            );
            let invariant = self
                .invariants
                .iter()
                .find(|invariant| invariant.id == expected.invariant_id)
                .with_context(|| {
                    format!(
                        "state observation {} is missing invariant result {}",
                        expected.id, expected.invariant_id
                    )
                })?;
            let expected_status = if actual_values == expected_values {
                ReplayCheckStatus::Passed
            } else {
                ReplayCheckStatus::Failed
            };
            ensure!(
                invariant.status == expected_status,
                "state observation {} requires invariant {} status {:?}, got {:?}",
                expected.id,
                expected.invariant_id,
                expected_status,
                invariant.status
            );
        }

        let lane = scenario
            .lanes
            .iter()
            .find(|lane| lane.driver == self.driver)
            .with_context(|| format!("scenario has no {:?} lane", self.driver))?;
        ensure!(
            self.gates.len() == lane.required_gates.len(),
            "result gate count {} does not match required count {}",
            self.gates.len(),
            lane.required_gates.len()
        );
        for required in &lane.required_gates {
            let actual = self
                .gates
                .iter()
                .find(|gate| gate.id == required.id)
                .with_context(|| format!("missing required gate result: {}", required.id))?;
            ensure!(
                actual.evidence_kind == required.evidence_kind,
                "gate {} evidence kind does not match scenario",
                required.id
            );
        }

        ensure!(
            self.invariants.len() == scenario.expected_invariants.len(),
            "result invariant count {} does not match expected count {}",
            self.invariants.len(),
            scenario.expected_invariants.len()
        );
        for expected in &scenario.expected_invariants {
            ensure!(
                self.invariants
                    .iter()
                    .any(|invariant| invariant.id == expected.id),
                "missing expected invariant result: {}",
                expected.id
            );
        }
        Ok(())
    }
}

fn validate_concurrent_groups(groups: &[ReplayConcurrentGroup]) -> Result<BTreeSet<String>> {
    let mut group_ids = BTreeSet::new();
    for group in groups {
        validate_identifier("concurrent group id", &group.id)?;
        ensure!(
            group_ids.insert(group.id.clone()),
            "duplicate concurrent group id: {}",
            group.id
        );
        ensure!(
            (1..=MAX_CONCURRENT_GROUP_REPETITIONS).contains(&group.repetitions),
            "concurrent group {} repetitions must be in 1..={MAX_CONCURRENT_GROUP_REPETITIONS}",
            group.id
        );
        ensure!(
            group.actions.len() >= 2,
            "concurrent group {} must contain at least two actions",
            group.id
        );
        let mut actors = BTreeSet::new();
        for action in &group.actions {
            validate_identifier("concurrent actor id", action.actor())?;
            ensure!(
                actors.insert(action.actor()),
                "concurrent group {} repeats actor {}",
                group.id,
                action.actor()
            );
            match action {
                ReplayConcurrentAction::PlaceBlock { item, .. } => {
                    mc_protocol::codec::Identifier::parse(item).with_context(|| {
                        format!("concurrent group {} has invalid item {item}", group.id)
                    })?;
                }
                ReplayConcurrentAction::ChestPickup { slot, .. } => ensure!(
                    *slot < 27,
                    "concurrent group {} chest slot is outside 0..27: {slot}",
                    group.id
                ),
            }
        }
        match &group.fixture {
            ReplayConcurrentFixture::SameTargetPlacement => {
                ensure!(
                    group.actions.len() == 2,
                    "same_target_placement group {} requires exactly two actions",
                    group.id
                );
                ensure!(
                    group
                        .actions
                        .iter()
                        .all(|action| matches!(action, ReplayConcurrentAction::PlaceBlock { .. })),
                    "same_target_placement group {} contains an incompatible action",
                    group.id
                );
            }
            ReplayConcurrentFixture::SharedChest {
                item,
                initial_count,
            } => {
                mc_protocol::codec::Identifier::parse(item).with_context(|| {
                    format!(
                        "concurrent group {} has invalid fixture item {item}",
                        group.id
                    )
                })?;
                ensure!(
                    *initial_count > 0,
                    "shared_chest group {} initial_count must be positive",
                    group.id
                );
                ensure!(
                    group.repetitions == 1,
                    "shared_chest group {} requires exactly one repetition",
                    group.id
                );
                ensure!(
                    group.actions.len() == 2,
                    "shared_chest group {} requires exactly two actions",
                    group.id
                );
                ensure!(
                    group
                        .actions
                        .iter()
                        .all(|action| matches!(action, ReplayConcurrentAction::ChestPickup { .. })),
                    "shared_chest group {} contains an incompatible action",
                    group.id
                );
                let slots = group
                    .actions
                    .iter()
                    .filter_map(|action| match action {
                        ReplayConcurrentAction::ChestPickup { slot, .. } => Some(slot),
                        ReplayConcurrentAction::PlaceBlock { .. } => None,
                    })
                    .collect::<BTreeSet<_>>();
                ensure!(
                    slots.len() == 1,
                    "shared_chest group {} actions must target the same slot",
                    group.id
                );
            }
        }
    }
    Ok(group_ids)
}

fn validate_state_values(label: &str, id: &str, values: &[ReplayStateValue]) -> Result<()> {
    ensure!(!values.is_empty(), "{label} {id} has no values");
    ensure!(
        values.len() <= MAX_REPLAY_CHECKS,
        "{label} {id} has too many values"
    );
    let mut keys = BTreeSet::new();
    for value in values {
        validate_identifier("state value key", &value.key)?;
        ensure!(
            keys.insert(value.key.as_str()),
            "{label} {id} repeats value key {}",
            value.key
        );
    }
    Ok(())
}

fn state_values_map(values: &[ReplayStateValue]) -> BTreeMap<&str, i64> {
    values
        .iter()
        .map(|value| (value.key.as_str(), value.value))
        .collect()
}

fn validate_action(action: &CoreAction) -> Result<()> {
    match action {
        CoreAction::WaitTicks { ticks } => {
            ensure!(*ticks > 0, "wait_ticks action must wait at least one tick");
        }
        CoreAction::Look { yaw_deg, pitch_deg } => {
            ensure!(
                (-180..=180).contains(yaw_deg),
                "look yaw is outside [-180, 180]: {yaw_deg}"
            );
            ensure!(
                (-90..=90).contains(pitch_deg),
                "look pitch is outside [-90, 90]: {pitch_deg}"
            );
        }
        CoreAction::MoveBy { .. } | CoreAction::Reconnect => {}
    }
    Ok(())
}

fn validate_provenance(provenance: &ReplayProvenance) -> Result<()> {
    ensure!(
        matches!(provenance.git_commit.len(), 40 | 64)
            && provenance
                .git_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "git_commit must be a full 40- or 64-character hexadecimal object id"
    );
    ensure!(
        provenance.config_sha256.len() == 64
            && provenance
                .config_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "config_sha256 must be a 64-character hexadecimal digest"
    );
    validate_non_empty("build profile", &provenance.build_profile)?;
    validate_non_empty("sidecar version", &provenance.sidecar_version)?;
    validate_non_empty("hardware os", &provenance.hardware.os)?;
    validate_non_empty("hardware arch", &provenance.hardware.arch)?;
    validate_non_empty("hardware cpu_model", &provenance.hardware.cpu_model)?;
    ensure!(
        provenance.hardware.logical_cpus > 0,
        "hardware logical_cpus must be positive"
    );
    ensure!(
        provenance.hardware.memory_mib > 0,
        "hardware memory_mib must be positive"
    );
    Ok(())
}

fn validate_check_reason(
    status: ReplayCheckStatus,
    reason: Option<&str>,
    kind: &str,
    id: &str,
) -> Result<()> {
    if status == ReplayCheckStatus::Passed {
        ensure!(
            reason.is_none(),
            "passed {kind} {id} must not carry a failure reason"
        );
    } else {
        let reason =
            reason.with_context(|| format!("non-passing {kind} {id} is missing reason"))?;
        validate_non_empty("non-passing check reason", reason)?;
    }
    Ok(())
}

fn derive_outcome(statuses: impl IntoIterator<Item = ReplayCheckStatus>) -> Result<ReplayOutcome> {
    let mut saw_any = false;
    let mut saw_failed = false;
    let mut saw_blocked = false;
    let mut saw_degraded = false;
    for status in statuses {
        saw_any = true;
        match status {
            ReplayCheckStatus::Passed => {}
            ReplayCheckStatus::Failed => saw_failed = true,
            ReplayCheckStatus::Blocked => saw_blocked = true,
            ReplayCheckStatus::Degraded | ReplayCheckStatus::Skipped => saw_degraded = true,
        }
    }
    ensure!(saw_any, "replay result has no gate or invariant checks");
    Ok(if saw_failed {
        ReplayOutcome::Failed
    } else if saw_blocked {
        ReplayOutcome::Blocked
    } else if saw_degraded {
        ReplayOutcome::Degraded
    } else {
        ReplayOutcome::Passed
    })
}

fn validate_ledger_row(row: &str) -> Result<()> {
    ensure!(!row.is_empty(), "core gate ledger row is empty");
    ensure!(row.len() <= 16, "core gate ledger row is too long");
    ensure!(
        row.bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()),
        "invalid core gate ledger row: {row}"
    );
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{label} is empty");
    ensure!(value.len() <= 128, "{label} is longer than 128 bytes");
    let mut bytes = value.bytes();
    let first = bytes.next().expect("non-empty identifier");
    ensure!(
        first.is_ascii_lowercase() || first.is_ascii_digit(),
        "{label} must start with lowercase ASCII or a digit: {value}"
    );
    ensure!(
        bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        }),
        "{label} contains unsupported characters: {value}"
    );
    Ok(())
}

fn validate_non_empty(label: &str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{label} is empty");
    ensure!(value.len() <= 1024, "{label} is longer than 1024 bytes");
    Ok(())
}

fn validate_relative_artifact_path(value: &str) -> Result<()> {
    validate_non_empty("artifact path", value)?;
    ensure!(
        !value.contains('\\'),
        "artifact path must use repository-style forward slashes: {value}"
    );
    let path = Path::new(value);
    ensure!(!path.is_absolute(), "artifact path is absolute: {value}");
    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_normal = true,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                bail!("artifact path is not repository-relative: {value}");
            }
        }
    }
    ensure!(
        saw_normal,
        "artifact path has no repository component: {value}"
    );
    Ok(())
}

#[cfg(test)]
#[path = "replay_tests.rs"]
mod tests;
