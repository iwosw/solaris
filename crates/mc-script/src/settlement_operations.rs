//! Native settlement query/action DTOs: S1 records, S2 households, S3 districts.
//!
//! Bounded by the native registry fields and the parent-owned settlement
//! helpers: a query reads owned-or-visible records, an action names one closed
//! mutation (bind/unbind/rename/form-household/relocate-household/plan-district)
//! fenced by the monotonic record id AND the revision the caller read. Name plus
//! revision alone is not enough: after remove/recreate the name and revision 1
//! repeat while the id never does (ABA), so every action targets the stable id.
//! Household snapshots are derived occupancy projections (living current-dim
//! member UUIDs), never a second membership authority. A district plan binds
//! caller owner plus requested branch/level and building refs; the observation
//! derives live housing/population/certified-container food/working purposes.

use serde::{Deserialize, Serialize};

use crate::{MAX_SCRIPT_WORLD_TIME, ScriptDtoError, validate_entity_uuid, validate_script_id};

/// Maximum byte length of one native settlement name (mirrors the registry).
pub const MAX_SETTLEMENT_NAME_BYTES: usize = 64;
/// Maximum byte length of one `<settlement id>-<sequence>` household id.
pub const MAX_HOUSEHOLD_ID_BYTES: usize = 64;
/// Maximum byte length of one household needs/policy reference.
pub const MAX_HOUSEHOLD_POLICY_BYTES: usize = 128;
/// Maximum building refs bound to one district plan or observation.
pub const MAX_SETTLEMENT_DISTRICT_BUILDINGS: usize = 64;
/// Maximum byte length of one optional district observation reason.
pub const MAX_SETTLEMENT_DISTRICT_REASON_BYTES: usize = 512;

fn validate_settlement_name(value: &str) -> Result<(), ScriptDtoError> {
    if value.is_empty() || value.len() > MAX_SETTLEMENT_NAME_BYTES {
        return Err(ScriptDtoError::InvalidBounds);
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ScriptDtoError::InvalidId {
            field: "settlement name",
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

fn validate_settlement_id(id: u64) -> Result<(), ScriptDtoError> {
    if id == 0 || id > MAX_SCRIPT_WORLD_TIME {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_household_id(id: &str) -> Result<(), ScriptDtoError> {
    if id.len() > MAX_HOUSEHOLD_ID_BYTES {
        return Err(ScriptDtoError::InvalidBounds);
    }
    let (founder, sequence) = id.split_once('-').ok_or(ScriptDtoError::InvalidBounds)?;
    for part in [founder, sequence] {
        if part.starts_with('0')
            || part.parse::<u64>().is_err()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
    }
    Ok(())
}
fn validate_district_branch(branch: &str, level: u8) -> Result<(), ScriptDtoError> {
    match (branch, level) {
        ("none", 0)
        | ("estate", 1)
        | ("estate", 2)
        | ("fortress", 1)
        | ("fortress", 2)
        | ("town", 1)
        | ("town", 2) => Ok(()),
        _ => Err(ScriptDtoError::InvalidBounds),
    }
}

fn validate_structure_id(value: &str) -> Result<(), ScriptDtoError> {
    if value.is_empty() || value.len() > crate::MAX_STRUCTURE_ID_BYTES {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_district_purpose(value: &str) -> Result<(), ScriptDtoError> {
    crate::check_contract_resource_id(value)
}

/// Native household policy plus a derived living, current-dimension roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementHouseholdSnapshot {
    pub id: String,
    pub revision: u64,
    pub origin: String,
    pub home: Option<[i32; 3]>,
    pub needs: Option<String>,
    pub policy: Option<String>,
    pub members: Vec<String>,
}

impl ScriptSettlementHouseholdSnapshot {
    #[must_use]
    pub fn new(
        id: String,
        revision: u64,
        origin: String,
        home: Option<[i32; 3]>,
        needs: Option<String>,
        policy: Option<String>,
        members: Vec<String>,
    ) -> Self {
        Self {
            id,
            revision,
            origin,
            home,
            needs,
            policy,
            members,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_household_id(&self.id)?;
        if self.revision == 0
            || self.revision > MAX_SCRIPT_WORLD_TIME
            || !matches!(
                self.origin.as_str(),
                "adoption" | "migration" | "split" | "refugee"
            )
            || [&self.needs, &self.policy]
                .into_iter()
                .flatten()
                .any(|value| value.is_empty() || value.len() > MAX_HOUSEHOLD_POLICY_BYTES)
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut previous: Option<&str> = None;
        for member in &self.members {
            validate_entity_uuid(member)?;
            if previous.is_some_and(|prev| prev >= member.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
            previous = Some(member);
        }
        Ok(())
    }
}
/// One native building certificate: `kind` names the certificate family
/// (`structure` or `worksite` plugin-ledger certificates bound to a district
/// plan, or `building` for a player-built native `BuildingId` certificate),
/// `id` the durable native id, and `purpose` the certified resource id it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementBuildingRef {
    pub kind: String,
    pub id: String,
    pub purpose: String,
}

impl ScriptSettlementBuildingRef {
    #[must_use]
    pub fn new(kind: String, id: String, purpose: String) -> Self {
        Self { kind, id, purpose }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if !matches!(self.kind.as_str(), "structure" | "worksite" | "building") {
            return Err(ScriptDtoError::InvalidBounds);
        }
        validate_structure_id(&self.id)?;
        validate_district_purpose(&self.purpose)?;
        Ok(())
    }
}

/// One live district observation for a settlement record: the requested
/// branch/level the caller planned plus the certified building refs, and the
/// derived observation (stage, certified branch/level, live
/// housing/population, certified-container food, working purposes, optional
/// reason, observation ticks).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementDistrictSnapshot {
    pub requested_branch: String,
    pub requested_level: u8,
    pub stage: Option<String>,
    pub branch: String,
    pub level: u8,
    pub population: u32,
    pub housed: u32,
    pub food: u64,
    pub working: Vec<String>,
    pub reason: Option<String>,
    pub observed_tick: u64,
    pub world_revision: u64,
    pub buildings: Vec<ScriptSettlementBuildingRef>,
}

impl ScriptSettlementDistrictSnapshot {
    #[must_use]
    pub fn new(
        requested_branch: String,
        requested_level: u8,
        buildings: Vec<ScriptSettlementBuildingRef>,
    ) -> Self {
        Self {
            requested_branch,
            requested_level,
            stage: None,
            branch: "none".into(),
            level: 0,
            population: 0,
            housed: 0,
            food: 0,
            working: Vec::new(),
            reason: None,
            observed_tick: 0,
            world_revision: 0,
            buildings,
        }
    }

    /// Zero-copy plan-shape check the native owner reuses: requested
    /// branch/level plus the certified building refs, without cloning them
    /// into an observation first.
    pub fn validate_plan(
        branch: &str,
        level: u8,
        buildings: &[ScriptSettlementBuildingRef],
    ) -> Result<(), ScriptDtoError> {
        validate_district_branch(branch, level)?;
        validate_district_buildings(buildings)
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        Self::validate_plan(
            &self.requested_branch,
            self.requested_level,
            &self.buildings,
        )?;
        validate_district_branch(&self.branch, self.level)?;
        if let Some(stage) = &self.stage
            && !matches!(stage.as_str(), "hamlet" | "village" | "developed")
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if self.working.len() > MAX_SETTLEMENT_DISTRICT_BUILDINGS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut previous: Option<&str> = None;
        for purpose in &self.working {
            validate_district_purpose(purpose)?;
            if previous.is_some_and(|prev: &str| prev >= purpose.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
            previous = Some(purpose.as_str());
        }
        if self.housed > self.population {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(reason) = &self.reason
            && (reason.is_empty() || reason.len() > MAX_SETTLEMENT_DISTRICT_REASON_BYTES)
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if self.observed_tick > MAX_SCRIPT_WORLD_TIME || self.world_revision > MAX_SCRIPT_WORLD_TIME
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

fn validate_district_buildings(
    buildings: &[ScriptSettlementBuildingRef],
) -> Result<(), ScriptDtoError> {
    if buildings.is_empty() || buildings.len() > MAX_SETTLEMENT_DISTRICT_BUILDINGS {
        return Err(ScriptDtoError::InvalidBounds);
    }
    let mut seen = std::collections::BTreeSet::new();
    for building in buildings {
        building.validate()?;
        if !seen.insert((building.kind.as_str(), building.id.as_str())) {
            return Err(ScriptDtoError::InvalidBounds);
        }
    }
    Ok(())
}

/// Maximum settlements named in one query page request.
pub const MAX_SETTLEMENT_QUERY_NAMES: usize = 64;
/// Maximum settlements returned in one query page.
pub const MAX_SETTLEMENT_PAGE: usize = 64;

/// One bounded settlement query inside the durable operation envelope.
///
/// `names` empty reads one bounded page of visible records; explicit names
/// must each be valid registry names. This read carries no durable operation
/// id and observes the server's current revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementQuery {
    pub names: Vec<String>,
    pub cursor: Option<String>,
}

impl ScriptSettlementQuery {
    #[must_use]
    pub fn new(names: Vec<String>, cursor: Option<String>) -> Self {
        Self { names, cursor }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if self.names.len() > MAX_SETTLEMENT_QUERY_NAMES {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut previous: Option<&str> = None;
        for name in &self.names {
            validate_settlement_name(name)?;
            if previous.is_some_and(|prev: &str| prev >= name.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
            previous = Some(name.as_str());
        }
        if let Some(cursor) = &self.cursor {
            validate_settlement_name(cursor)?;
        }
        Ok(())
    }

    pub fn canonicalize(&mut self) {
        self.names.sort_unstable();
        self.names.dedup();
    }
}

/// One detached settlement snapshot: the monotonic native id, the registry
/// fields, plus the derived local member count. Counts are occupancy
/// projections, never a second membership authority. The id is what later
/// actions fence on: names are reusable, ids never are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementSnapshot {
    pub id: u64,
    pub name: String,
    pub owner: String,
    pub anchor: [i32; 3],
    pub radius: u32,
    pub revision: u64,
    pub member_count: u32,
    pub households: Vec<ScriptSettlementHouseholdSnapshot>,
    pub district: Option<ScriptSettlementDistrictSnapshot>,
    pub economy: Option<crate::ScriptSettlementEconomySnapshot>,
}

impl ScriptSettlementSnapshot {
    #[must_use]
    pub fn new(
        id: u64,
        name: String,
        owner: String,
        anchor: [i32; 3],
        radius: u32,
        revision: u64,
        member_count: u32,
    ) -> Self {
        Self {
            id,
            name,
            owner,
            anchor,
            radius,
            revision,
            member_count,
            households: Vec::new(),
            district: None,
            economy: None,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_settlement_id(self.id)?;
        validate_settlement_name(&self.name)?;
        if self.owner.is_empty() || self.owner.len() > 128 {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if self.revision == 0 || self.revision > MAX_SCRIPT_WORLD_TIME {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if self.radius == 0 || self.radius > 256 {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut previous: Option<&str> = None;
        for household in &self.households {
            household.validate()?;
            if previous.is_some_and(|prev| prev >= household.id.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
            previous = Some(&household.id);
        }
        if let Some(district) = &self.district {
            district.validate()?;
        }
        if let Some(economy) = &self.economy {
            economy.validate()?;
        }
        Ok(())
    }
}

/// One bounded page of settlement snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementPage {
    pub settlements: Vec<ScriptSettlementSnapshot>,
    pub cursor: Option<String>,
}

impl ScriptSettlementPage {
    #[must_use]
    pub fn new(settlements: Vec<ScriptSettlementSnapshot>, cursor: Option<String>) -> Self {
        Self {
            settlements,
            cursor,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if self.settlements.len() > MAX_SETTLEMENT_PAGE {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut previous: Option<&str> = None;
        for snapshot in &self.settlements {
            snapshot.validate()?;
            if previous.is_some_and(|prev: &str| prev >= snapshot.name.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
            previous = Some(snapshot.name.as_str());
        }
        if let Some(cursor) = &self.cursor {
            validate_settlement_name(cursor)?;
        }
        Ok(())
    }
}

/// One closed settlement mutation. Every member carries the durable operation
/// id the server records the commit under, the monotonic settlement id it
/// targets, plus the record revision the caller read; an unknown id or a
/// stale revision refuses without effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptSettlementOperation {
    Bind {
        operation_id: String,
        settlement: u64,
        entity_uuid: String,
        expected_revision: u64,
    },
    Unbind {
        operation_id: String,
        settlement: u64,
        entity_uuid: String,
        expected_revision: u64,
    },
    Rename {
        operation_id: String,
        settlement: u64,
        new_name: String,
        expected_revision: u64,
    },
    FormHousehold {
        operation_id: String,
        settlement: u64,
        entity_uuids: Vec<String>,
        expected_revision: u64,
    },
    RelocateHousehold {
        operation_id: String,
        settlement: u64,
        expected_revision: u64,
        household: String,
        destination: u64,
        expected_destination_revision: u64,
    },
    PlanDistrict {
        operation_id: String,
        settlement: u64,
        expected_revision: u64,
        branch: String,
        level: u8,
        buildings: Vec<ScriptSettlementBuildingRef>,
    },
    Economy {
        operation_id: String,
        settlement: u64,
        expected_revision: u64,
        action: Box<crate::ScriptSettlementEconomyAction>,
    },
}

impl ScriptSettlementOperation {
    #[must_use]
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Bind { operation_id, .. }
            | Self::Unbind { operation_id, .. }
            | Self::Rename { operation_id, .. }
            | Self::FormHousehold { operation_id, .. }
            | Self::RelocateHousehold { operation_id, .. }
            | Self::PlanDistrict { operation_id, .. }
            | Self::Economy { operation_id, .. } => operation_id,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_script_id(self.operation_id())?;
        match self {
            Self::Bind {
                settlement,
                entity_uuid,
                expected_revision,
                ..
            }
            | Self::Unbind {
                settlement,
                entity_uuid,
                expected_revision,
                ..
            } => {
                validate_settlement_id(*settlement)?;
                validate_entity_uuid(entity_uuid)?;
                if *expected_revision == 0 || *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Rename {
                settlement,
                new_name,
                expected_revision,
                ..
            } => {
                validate_settlement_id(*settlement)?;
                validate_settlement_name(new_name)?;
                if *expected_revision == 0 || *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::FormHousehold {
                settlement,
                entity_uuids,
                expected_revision,
                ..
            } => {
                validate_settlement_id(*settlement)?;
                if *expected_revision == 0
                    || *expected_revision > MAX_SCRIPT_WORLD_TIME
                    || entity_uuids.is_empty()
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut distinct = std::collections::BTreeSet::new();
                for uuid in entity_uuids {
                    validate_entity_uuid(uuid)?;
                    if !distinct.insert(uuid) {
                        return Err(ScriptDtoError::InvalidBounds);
                    }
                }
                Ok(())
            }
            Self::RelocateHousehold {
                settlement,
                expected_revision,
                household,
                destination,
                expected_destination_revision,
                ..
            } => {
                validate_settlement_id(*settlement)?;
                validate_settlement_id(*destination)?;
                validate_household_id(household)?;
                if settlement == destination
                    || *expected_revision == 0
                    || *expected_revision > MAX_SCRIPT_WORLD_TIME
                    || *expected_destination_revision == 0
                    || *expected_destination_revision > MAX_SCRIPT_WORLD_TIME
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::PlanDistrict {
                settlement,
                expected_revision,
                branch,
                level,
                buildings,
                ..
            } => {
                validate_settlement_id(*settlement)?;
                ScriptSettlementDistrictSnapshot::validate_plan(branch, *level, buildings)?;
                if *expected_revision == 0 || *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Economy {
                settlement,
                expected_revision,
                action,
                ..
            } => {
                validate_settlement_id(*settlement)?;
                if *expected_revision == 0 || *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                action.validate()
            }
        }
    }
}

/// The server's one settlement snapshot page, or the affected record's stable
/// id plus its current name (the name survives renames; the id is stable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptSettlementResult {
    Page(ScriptSettlementPage),
    Changed { id: u64, name: String },
}

impl ScriptSettlementResult {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Page(page) => page.validate(),
            Self::Changed { id, name } => {
                validate_settlement_id(*id)?;
                validate_settlement_name(name)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn household_observations_validate_members_and_origins() {
        let person = "00000000-0000-0000-0000-000000000001".to_owned();
        let mut family = ScriptSettlementHouseholdSnapshot::new(
            "1-1".into(),
            1,
            "adoption".into(),
            Some([0, 64, 0]),
            None,
            None,
            vec![person.clone()],
        );
        family.validate().unwrap();
        family.members.push(person);
        assert!(family.validate().is_err());
        family.members.pop();
        family.origin = "invented".into();
        assert!(family.validate().is_err());
        family.origin = "adoption".into();
        family.id = "1-01".into();
        assert!(family.validate().is_err());
    }

    #[test]
    fn native_city_dtos_do_not_reintroduce_plugin_household_population_caps() {
        let mut snapshot = ScriptSettlementSnapshot::new(
            1,
            "city".into(),
            "owner".into(),
            [0, 64, 0],
            48,
            1,
            10_000,
        );
        for family in 1..=500 {
            let people: Vec<_> = (0..20)
                .map(|person| format!("00000000-0000-0000-0000-{:012x}", family * 20 + person))
                .collect();
            ScriptSettlementOperation::FormHousehold {
                operation_id: format!("form-{family}"),
                settlement: 1,
                entity_uuids: people.clone(),
                expected_revision: 1,
            }
            .validate()
            .unwrap();
            snapshot
                .households
                .push(ScriptSettlementHouseholdSnapshot::new(
                    format!("1-{family}"),
                    1,
                    "adoption".into(),
                    Some([0, 64, 0]),
                    None,
                    None,
                    people,
                ));
        }
        snapshot.households.sort_by(|a, b| a.id.cmp(&b.id));
        snapshot.validate().unwrap();
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        let restored: ScriptSettlementSnapshot = serde_json::from_slice(&encoded).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.households.len(), 500);
        assert_eq!(restored.households[0].members.len(), 20);
    }

    #[test]
    fn household_mutations_reject_duplicate_people_and_unfenced_destinations() {
        let person = "00000000-0000-0000-0000-000000000001".to_owned();
        let form = ScriptSettlementOperation::FormHousehold {
            operation_id: "form".into(),
            settlement: 1,
            entity_uuids: vec![person.clone(), person],
            expected_revision: 1,
        };
        assert!(form.validate().is_err());
        let movement = ScriptSettlementOperation::RelocateHousehold {
            operation_id: "move".into(),
            settlement: 1,
            expected_revision: 1,
            household: "1-1".into(),
            destination: 2,
            expected_destination_revision: 0,
        };
        assert!(movement.validate().is_err());
    }

    fn district_ref(purpose: &str) -> ScriptSettlementBuildingRef {
        ScriptSettlementBuildingRef::new("structure".into(), "granary-one".into(), purpose.into())
    }

    fn district_plan(branch: &str, level: u8) -> ScriptSettlementOperation {
        ScriptSettlementOperation::PlanDistrict {
            operation_id: "plan".into(),
            settlement: 1,
            expected_revision: 1,
            branch: branch.into(),
            level,
            buildings: vec![district_ref("solaris:warehouse")],
        }
    }

    #[test]
    fn district_plans_reject_invalid_branch_levels() {
        assert!(district_plan("estate", 1).validate().is_ok());
        assert!(district_plan("none", 1).validate().is_err());
        assert!(district_plan("estate", 0).validate().is_err());
        assert!(district_plan("town", 3).validate().is_err());
        assert!(district_plan("keep", 1).validate().is_err());
        assert!(district_plan("fortress", 2).validate().is_ok());
    }

    #[test]
    fn district_plans_reject_duplicate_buildings() {
        let duplicate = ScriptSettlementOperation::PlanDistrict {
            operation_id: "plan".into(),
            settlement: 1,
            expected_revision: 1,
            branch: "estate".into(),
            level: 1,
            buildings: vec![
                district_ref("solaris:warehouse"),
                district_ref("solaris:warehouse"),
            ],
        };
        assert!(duplicate.validate().is_err());
        let empty = ScriptSettlementOperation::PlanDistrict {
            operation_id: "plan".into(),
            settlement: 1,
            expected_revision: 1,
            branch: "estate".into(),
            level: 1,
            buildings: Vec::new(),
        };
        assert!(empty.validate().is_err());
    }

    #[test]
    fn district_observations_validate_typed_fields() {
        let mut observation = ScriptSettlementDistrictSnapshot::new(
            "estate".into(),
            1,
            vec![district_ref("solaris:warehouse")],
        );
        observation.stage = Some("hamlet".into());
        observation.branch = "estate".into();
        observation.level = 1;
        observation.population = 30;
        observation.housed = 24;
        observation.food = 40;
        observation.working = vec!["solaris:warehouse".into()];
        observation.observed_tick = 7;
        observation.world_revision = 9;
        observation.validate().unwrap();
        observation.stage = Some("metropolis".into());
        assert!(observation.validate().is_err());
        observation.stage = Some("hamlet".into());
        observation.housed = 31;
        assert!(observation.validate().is_err());
        observation.housed = 24;
        observation.working = vec!["solaris:warehouse".into(), "solaris:warehouse".into()];
        assert!(observation.validate().is_err());
        let mut snapshot =
            ScriptSettlementSnapshot::new(1, "town".into(), "owner".into(), [0, 64, 0], 48, 1, 30);
        assert!(snapshot.district.is_none());
        assert!(snapshot.economy.is_none());
        snapshot.district = Some(observation);
        assert!(snapshot.validate().is_err());
        if let Some(district) = snapshot.district.as_mut() {
            district.working = vec!["solaris:warehouse".into()];
        }
        snapshot.validate().unwrap();
        snapshot.economy = Some(crate::ScriptSettlementEconomySnapshot::new());
        snapshot.validate().unwrap();
    }

    #[test]
    fn economy_operations_fence_on_id_and_revision() {
        let action = crate::ScriptSettlementEconomyAction::ProductionPause {
            job: "job-one".into(),
        };
        let operation = ScriptSettlementOperation::Economy {
            operation_id: "op-one".into(),
            settlement: 1,
            expected_revision: 2,
            action: Box::new(action),
        };
        assert_eq!(operation.operation_id(), "op-one");
        operation.validate().unwrap();
        let encoded = serde_json::to_vec(&operation).unwrap();
        let restored: ScriptSettlementOperation = serde_json::from_slice(&encoded).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored, operation);
        let stale = ScriptSettlementOperation::Economy {
            operation_id: "op-one".into(),
            settlement: 1,
            expected_revision: 0,
            action: Box::new(crate::ScriptSettlementEconomyAction::Pay {
                contract: "1".into(),
            }),
        };
        assert!(stale.validate().is_err());
        let bad_id = ScriptSettlementOperation::Economy {
            operation_id: "BAD ID".into(),
            settlement: 1,
            expected_revision: 1,
            action: Box::new(crate::ScriptSettlementEconomyAction::Pay {
                contract: "1".into(),
            }),
        };
        assert!(bad_id.validate().is_err());
    }
}
