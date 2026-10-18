//! Native settlement economic customization DTOs (D006 S4).
//!
//! Closed typed actions over the existing settlement query/action surface:
//! production jobs, employment payroll, quote exchange, money-escrow paid
//! shipments and physical caravan shipments. Shape-only validation here;
//! native stock, bodies and poses live in the parent owner.

use serde::{Deserialize, Serialize};

use crate::{MAX_SCRIPT_WORLD_TIME, ScriptDtoError, validate_entity_uuid, validate_script_id};

/// Largest single caravan shipment, in native units.
pub const MAX_SETTLEMENT_CARAVAN_QUANTITY: u32 = 4096;
/// Finite route cells per caravan shipment: 2..=16 waypoints.
pub const MAX_SETTLEMENT_CARAVAN_WAYPOINTS: usize = 16;
/// Total caravan route budget, in ticks.
pub const MAX_SETTLEMENT_CARAVAN_TICKS: u64 = 72_000;
/// Maximum byte length of one terminal caravan block reason.
pub const MAX_SETTLEMENT_CARAVAN_REASON_BYTES: usize = 192;

fn validate_position(position: [i32; 3]) -> Result<(), ScriptDtoError> {
    if position
        .iter()
        .any(|coordinate| coordinate.unsigned_abs() > crate::MAX_SCRIPT_BLOCK_COORDINATE as u32)
    {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_sequence_id(id: &str) -> Result<(), ScriptDtoError> {
    if id.is_empty() || id.len() > crate::MAX_SCRIPT_ID_BYTES {
        return Err(ScriptDtoError::InvalidBounds);
    }
    let bytes = id.as_bytes();
    if bytes[0] == b'0' || !bytes.iter().all(|byte| byte.is_ascii_digit()) {
        return Err(ScriptDtoError::InvalidBounds);
    }
    if id.parse::<u64>().is_err() {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_job_id(id: &str) -> Result<(), ScriptDtoError> {
    validate_script_id(id).map(|_| ())
}

/// One native economic inventory endpoint: exact living carry/equipment
/// bodies by UUID, exact native containers by position (the server infers
/// the current dimension), or the native owner player by UUID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptSettlementInventoryEndpoint {
    Carry { uuid: String },
    Equipment { uuid: String },
    Container { position: [i32; 3] },
    Player { uuid: String },
}

impl ScriptSettlementInventoryEndpoint {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Carry { uuid } | Self::Equipment { uuid } | Self::Player { uuid } => {
                validate_entity_uuid(uuid)
            }
            Self::Container { position } => validate_position(*position),
        }
    }
}

/// Closed typed economic customization actions. Field shapes validate here;
/// stock/building policy stays in the native owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptSettlementEconomyAction {
    ProductionStart {
        job: String,
        worker: String,
        recipe: String,
        count: u32,
        building: crate::ScriptSettlementBuildingRef,
        station: [i32; 3],
        stand_at: [i32; 3],
    },
    ProductionPause {
        job: String,
    },
    ProductionResume {
        job: String,
    },
    ProductionCancel {
        job: String,
    },
    Employ {
        contract: String,
        worker: String,
        treasury: ScriptSettlementInventoryEndpoint,
        wage: u32,
        period: u64,
        budget: u64,
    },
    StopEmployment {
        contract: String,
    },
    DismissEmployment {
        contract: String,
    },
    Pay {
        contract: String,
    },
    Quote {
        id: String,
        seller: ScriptSettlementInventoryEndpoint,
        buyer: ScriptSettlementInventoryEndpoint,
        source_slot: u16,
        quantity: u32,
        price: u32,
    },
    AcceptQuote {
        id: String,
    },
    CancelQuote {
        id: String,
    },
    EscrowReserve {
        id: String,
        shipment: String,
        payer: ScriptSettlementInventoryEndpoint,
        escrow: ScriptSettlementInventoryEndpoint,
        payee: ScriptSettlementInventoryEndpoint,
        price: u32,
    },
    EscrowPayout {
        id: String,
    },
    CaravanOpen {
        id: String,
        carrier: String,
        source_building: crate::ScriptSettlementBuildingRef,
        source_position: [i32; 3],
        destination_building: crate::ScriptSettlementBuildingRef,
        destination_position: [i32; 3],
        source_slot: u16,
        quantity: u32,
        waypoints: Vec<[i32; 3]>,
        max_ticks: u64,
        paid_mandate: Option<String>,
    },
    CaravanPickup {
        id: String,
    },
    CaravanUnload {
        id: String,
    },
    CaravanCancel {
        id: String,
    },
    CaravanBlock {
        id: String,
        reason: String,
    },
}

fn validate_route(waypoints: &[[i32; 3]]) -> Result<(), ScriptDtoError> {
    if waypoints.len() < 2 || waypoints.len() > MAX_SETTLEMENT_CARAVAN_WAYPOINTS {
        return Err(ScriptDtoError::InvalidBounds);
    }
    for waypoint in waypoints {
        validate_position(*waypoint)?;
    }
    for pair in waypoints.windows(2) {
        if pair[0] == pair[1] {
            return Err(ScriptDtoError::InvalidBounds);
        }
        for (&from, &to) in pair[0].iter().zip(&pair[1]) {
            if (i64::from(from) - i64::from(to)).abs() > 128 {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
    }
    Ok(())
}

impl ScriptSettlementEconomyAction {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::ProductionStart {
                job,
                worker,
                recipe,
                count,
                building,
                station,
                stand_at,
            } => {
                validate_job_id(job)?;
                validate_entity_uuid(worker)?;
                crate::check_contract_resource_id(recipe)?;
                if *count == 0 {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                building.validate()?;
                validate_position(*station)?;
                validate_position(*stand_at)?;
                Ok(())
            }
            Self::ProductionPause { job }
            | Self::ProductionResume { job }
            | Self::ProductionCancel { job } => validate_job_id(job),
            Self::Employ {
                contract,
                worker,
                treasury,
                wage,
                period,
                budget,
            } => {
                validate_sequence_id(contract)?;
                validate_entity_uuid(worker)?;
                treasury.validate()?;
                if *wage == 0 || *period == 0 || *budget == 0 {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::StopEmployment { contract }
            | Self::DismissEmployment { contract }
            | Self::Pay { contract } => validate_sequence_id(contract),
            Self::Quote {
                id,
                seller,
                buyer,
                quantity,
                price,
                ..
            } => {
                validate_sequence_id(id)?;
                seller.validate()?;
                buyer.validate()?;
                if seller == buyer || *quantity == 0 || *price == 0 {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::AcceptQuote { id } | Self::CancelQuote { id } => validate_sequence_id(id),
            Self::EscrowReserve {
                id,
                shipment,
                payer,
                escrow,
                payee,
                price,
                ..
            } => {
                validate_sequence_id(id)?;
                validate_job_id(shipment)?;
                payer.validate()?;
                escrow.validate()?;
                payee.validate()?;
                if escrow == payer || escrow == payee || *price == 0 {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::EscrowPayout { id } => validate_sequence_id(id),
            Self::CaravanOpen {
                id,
                carrier,
                source_building,
                source_position,
                destination_building,
                destination_position,
                quantity,
                waypoints,
                max_ticks,
                paid_mandate,
                ..
            } => {
                validate_job_id(id)?;
                validate_entity_uuid(carrier)?;
                source_building.validate()?;
                destination_building.validate()?;
                validate_position(*source_position)?;
                validate_position(*destination_position)?;
                if source_position == destination_position
                    && source_building == destination_building
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                if *quantity == 0 || *quantity > MAX_SETTLEMENT_CARAVAN_QUANTITY {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                validate_route(waypoints)?;
                if *max_ticks == 0 || *max_ticks > MAX_SETTLEMENT_CARAVAN_TICKS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                if let Some(mandate) = paid_mandate {
                    validate_sequence_id(mandate)?;
                }
                Ok(())
            }
            Self::CaravanPickup { id }
            | Self::CaravanUnload { id }
            | Self::CaravanCancel { id } => validate_job_id(id),
            Self::CaravanBlock { id, reason } => {
                validate_job_id(id)?;
                if reason.is_empty() || reason.len() > MAX_SETTLEMENT_CARAVAN_REASON_BYTES {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
        }
    }
}

/// One live production job observation: requested/completed portions plus
/// the native pause reason (absent while running), the cancel flag and the
/// next due tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementProductionSnapshot {
    pub id: String,
    pub worker: String,
    pub recipe: String,
    pub recipe_revision: String,
    pub requested: u32,
    pub completed: u32,
    pub paused: Option<String>,
    pub cancelled: bool,
    pub next_due: u64,
}

impl ScriptSettlementProductionSnapshot {
    #[must_use]
    pub fn new(
        id: String,
        worker: String,
        recipe: String,
        recipe_revision: String,
        requested: u32,
    ) -> Self {
        Self {
            id,
            worker,
            recipe,
            recipe_revision,
            requested,
            completed: 0,
            paused: None,
            cancelled: false,
            next_due: 0,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_job_id(&self.id)?;
        validate_entity_uuid(&self.worker)?;
        crate::check_contract_resource_id(&self.recipe)?;
        if self.requested == 0 || self.completed > self.requested {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if self.next_due > MAX_SCRIPT_WORLD_TIME {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// One live employment observation: the treasury endpoint, wage/period,
/// remaining budget, earned/paid counters, the active flag and next due.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementEmploymentSnapshot {
    pub id: String,
    pub worker: String,
    pub treasury: ScriptSettlementInventoryEndpoint,
    pub wage: u32,
    pub period: u64,
    pub budget: u64,
    pub earned: u64,
    pub paid: u64,
    pub active: bool,
    pub next_due: u64,
}

impl ScriptSettlementEmploymentSnapshot {
    #[must_use]
    pub fn new(
        id: String,
        worker: String,
        treasury: ScriptSettlementInventoryEndpoint,
        wage: u32,
        period: u64,
        budget: u64,
    ) -> Self {
        Self {
            id,
            worker,
            treasury,
            wage,
            period,
            budget,
            earned: 0,
            paid: 0,
            active: true,
            next_due: 0,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_sequence_id(&self.id)?;
        validate_entity_uuid(&self.worker)?;
        self.treasury.validate()?;
        if self.wage == 0 || self.period == 0 {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if self.paid > self.earned || self.next_due > MAX_SCRIPT_WORLD_TIME {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// One pending quote observation: endpoints plus the read-only item summary
/// (contract summary, not a storage mirror).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementQuoteSnapshot {
    pub id: String,
    pub seller: ScriptSettlementInventoryEndpoint,
    pub buyer: ScriptSettlementInventoryEndpoint,
    pub item_id: String,
    pub quantity: u32,
    pub price: u32,
}

impl ScriptSettlementQuoteSnapshot {
    #[must_use]
    pub fn new(
        id: String,
        seller: ScriptSettlementInventoryEndpoint,
        buyer: ScriptSettlementInventoryEndpoint,
        item_id: String,
        quantity: u32,
        price: u32,
    ) -> Self {
        Self {
            id,
            seller,
            buyer,
            item_id,
            quantity,
            price,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_sequence_id(&self.id)?;
        self.seller.validate()?;
        self.buyer.validate()?;
        crate::check_contract_resource_id(&self.item_id)?;
        if self.seller == self.buyer || self.quantity == 0 || self.price == 0 {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// One escrow mandate observation: the caravan shipment it pays for, the
/// money endpoints, the price and the live state (`escrowed`/`lost`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementMandateSnapshot {
    pub id: String,
    pub shipment: String,
    pub payer: ScriptSettlementInventoryEndpoint,
    pub escrow: ScriptSettlementInventoryEndpoint,
    pub payee: ScriptSettlementInventoryEndpoint,
    pub price: u32,
    pub status: String,
}

impl ScriptSettlementMandateSnapshot {
    #[must_use]
    pub fn new(
        id: String,
        shipment: String,
        payer: ScriptSettlementInventoryEndpoint,
        escrow: ScriptSettlementInventoryEndpoint,
        payee: ScriptSettlementInventoryEndpoint,
        price: u32,
    ) -> Self {
        Self {
            id,
            shipment,
            payer,
            escrow,
            payee,
            price,
            status: "escrowed".into(),
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_sequence_id(&self.id)?;
        validate_job_id(&self.shipment)?;
        self.payer.validate()?;
        self.escrow.validate()?;
        self.payee.validate()?;
        if self.escrow == self.payer || self.escrow == self.payee || self.price == 0 {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if !matches!(self.status.as_str(), "escrowed" | "lost") {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// One caravan shipment observation: the carrier body, endpoints, read-only
/// item summary, the stage, the route cursor, the finite route, the failure
/// reason (required while blocked, optional when lost) and the paid mandate reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementShipmentSnapshot {
    pub id: String,
    pub carrier: String,
    pub source: ScriptSettlementInventoryEndpoint,
    pub destination: ScriptSettlementInventoryEndpoint,
    pub item_id: String,
    pub quantity: u32,
    pub stage: String,
    pub waypoint: u32,
    pub waypoints: Vec<[i32; 3]>,
    pub blocked: Option<String>,
    pub paid_mandate: Option<String>,
}

impl ScriptSettlementShipmentSnapshot {
    #[must_use]
    pub fn new(
        id: String,
        carrier: String,
        source: ScriptSettlementInventoryEndpoint,
        destination: ScriptSettlementInventoryEndpoint,
        item_id: String,
        quantity: u32,
    ) -> Self {
        Self {
            id,
            carrier,
            source,
            destination,
            item_id,
            quantity,
            stage: "loading".into(),
            waypoint: 0,
            waypoints: Vec::new(),
            blocked: None,
            paid_mandate: None,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_job_id(&self.id)?;
        validate_entity_uuid(&self.carrier)?;
        self.source.validate()?;
        self.destination.validate()?;
        crate::check_contract_resource_id(&self.item_id)?;
        if self.quantity == 0 || self.quantity > MAX_SETTLEMENT_CARAVAN_QUANTITY {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if !matches!(
            self.stage.as_str(),
            "loading" | "transit" | "arrived" | "blocked" | "delivered" | "lost" | "cancelled"
        ) {
            return Err(ScriptDtoError::InvalidBounds);
        }
        validate_route(&self.waypoints)?;
        if u64::from(self.waypoint) > self.waypoints.len() as u64 {
            return Err(ScriptDtoError::InvalidBounds);
        }
        match (self.stage.as_str(), self.blocked.as_ref()) {
            ("blocked" | "lost", Some(reason)) => {
                if reason.is_empty() || reason.len() > MAX_SETTLEMENT_CARAVAN_REASON_BYTES {
                    return Err(ScriptDtoError::InvalidBounds);
                }
            }
            ("blocked", None) => return Err(ScriptDtoError::InvalidBounds),
            (_, None) => {}
            _ => return Err(ScriptDtoError::InvalidBounds),
        }
        if let Some(mandate) = &self.paid_mandate {
            validate_sequence_id(mandate)?;
        }
        Ok(())
    }
}

/// One settlement's live economic observation: open jobs, employments,
/// quotes, mandates and shipments plus the monotonic replay watermarks.
/// Watermarks fence only NEW admission at/below the fence; live records with
/// smaller ids stay valid (out-of-order completion survives save/reopen).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSettlementEconomySnapshot {
    pub production: Vec<ScriptSettlementProductionSnapshot>,
    pub employments: Vec<ScriptSettlementEmploymentSnapshot>,
    pub quotes: Vec<ScriptSettlementQuoteSnapshot>,
    pub mandates: Vec<ScriptSettlementMandateSnapshot>,
    pub shipments: Vec<ScriptSettlementShipmentSnapshot>,
    pub ended_employment: u64,
    pub completed_quote: u64,
    pub settled_mandate: u64,
}

impl ScriptSettlementEconomySnapshot {
    #[must_use]
    pub fn new() -> Self {
        Self {
            production: Vec::new(),
            employments: Vec::new(),
            quotes: Vec::new(),
            mandates: Vec::new(),
            shipments: Vec::new(),
            ended_employment: 0,
            completed_quote: 0,
            settled_mandate: 0,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        // Watermarks are full-u64 monotonic sequence fences, not clock ticks:
        // no MAX_SCRIPT_WORLD_TIME bound. They fence only NEW admission at or
        // below the fence; live records with smaller ids stay valid.
        let mut seen = std::collections::BTreeSet::new();
        for job in &self.production {
            job.validate()?;
            if !seen.insert(("job", job.id.as_str())) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for employment in &self.employments {
            employment.validate()?;
            if !seen.insert(employment.id.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for quote in &self.quotes {
            quote.validate()?;
            if !seen.insert(quote.id.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for mandate in &self.mandates {
            mandate.validate()?;
            if !seen.insert(mandate.id.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for shipment in &self.shipments {
            shipment.validate()?;
            if !seen.insert(shipment.id.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}

impl Default for ScriptSettlementEconomySnapshot {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKER: &str = "00000000-0000-0000-0000-000000000001";
    const OTHER: &str = "00000000-0000-0000-0000-000000000002";

    fn building() -> crate::ScriptSettlementBuildingRef {
        crate::ScriptSettlementBuildingRef::new(
            "structure".into(),
            "granary-one".into(),
            "solaris:warehouse".into(),
        )
    }

    fn treasury() -> ScriptSettlementInventoryEndpoint {
        ScriptSettlementInventoryEndpoint::Container {
            position: [8, 64, 8],
        }
    }

    fn carry(uuid: &str) -> ScriptSettlementInventoryEndpoint {
        ScriptSettlementInventoryEndpoint::Carry {
            uuid: uuid.to_owned(),
        }
    }

    fn route() -> Vec<[i32; 3]> {
        vec![[7, 64, 8], [8, 64, 8]]
    }

    #[test]
    fn typed_economic_actions_validate() {
        for action in [
            ScriptSettlementEconomyAction::ProductionStart {
                job: "job-one".into(),
                worker: WORKER.into(),
                recipe: "solaris:boards".into(),
                count: 3,
                building: building(),
                station: [8, 64, 8],
                stand_at: [7, 64, 8],
            },
            ScriptSettlementEconomyAction::ProductionPause {
                job: "job-one".into(),
            },
            ScriptSettlementEconomyAction::ProductionResume {
                job: "job-one".into(),
            },
            ScriptSettlementEconomyAction::ProductionCancel {
                job: "job-one".into(),
            },
            ScriptSettlementEconomyAction::Employ {
                contract: "1".into(),
                worker: WORKER.into(),
                treasury: treasury(),
                wage: 4,
                period: 5,
                budget: 6,
            },
            ScriptSettlementEconomyAction::StopEmployment {
                contract: "1".into(),
            },
            ScriptSettlementEconomyAction::DismissEmployment {
                contract: "1".into(),
            },
            ScriptSettlementEconomyAction::Pay {
                contract: "1".into(),
            },
            ScriptSettlementEconomyAction::Quote {
                id: "2".into(),
                seller: treasury(),
                buyer: carry(WORKER),
                source_slot: 0,
                quantity: 3,
                price: 1,
            },
            ScriptSettlementEconomyAction::AcceptQuote { id: "2".into() },
            ScriptSettlementEconomyAction::CancelQuote { id: "2".into() },
            ScriptSettlementEconomyAction::EscrowReserve {
                id: "3".into(),
                shipment: "run-one".into(),
                payer: treasury(),
                escrow: carry(OTHER),
                payee: carry(WORKER),
                price: 2,
            },
            ScriptSettlementEconomyAction::EscrowPayout { id: "3".into() },
            ScriptSettlementEconomyAction::CaravanOpen {
                id: "run-one".into(),
                carrier: WORKER.into(),
                source_building: building(),
                source_position: [8, 64, 8],
                destination_building: building(),
                destination_position: [20, 64, 8],
                source_slot: 0,
                quantity: 3,
                waypoints: route(),
                max_ticks: 100,
                paid_mandate: Some("3".into()),
            },
            ScriptSettlementEconomyAction::CaravanPickup {
                id: "run-one".into(),
            },
            ScriptSettlementEconomyAction::CaravanUnload {
                id: "run-one".into(),
            },
            ScriptSettlementEconomyAction::CaravanCancel {
                id: "run-one".into(),
            },
            ScriptSettlementEconomyAction::CaravanBlock {
                id: "run-one".into(),
                reason: "rockfall".into(),
            },
        ] {
            action.validate().unwrap();
            let encoded = serde_json::to_vec(&action).unwrap();
            let restored: ScriptSettlementEconomyAction = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(restored, action);
        }
    }

    #[test]
    fn malformed_ids_revisions_and_endpoints_refuse() {
        assert!(
            ScriptSettlementEconomyAction::Employ {
                contract: "01".into(),
                worker: WORKER.into(),
                treasury: treasury(),
                wage: 1,
                period: 1,
                budget: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            ScriptSettlementEconomyAction::Pay {
                contract: "0".into(),
            }
            .validate()
            .is_err()
        );
        assert!(
            ScriptSettlementEconomyAction::ProductionPause { job: "".into() }
                .validate()
                .is_err()
        );
        assert!(
            ScriptSettlementEconomyAction::Employ {
                contract: "1".into(),
                worker: WORKER.into(),
                treasury: treasury(),
                wage: 0,
                period: 1,
                budget: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            ScriptSettlementEconomyAction::Quote {
                id: "2".into(),
                seller: treasury(),
                buyer: treasury(),
                source_slot: 0,
                quantity: 1,
                price: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            ScriptSettlementEconomyAction::CaravanOpen {
                id: "run-one".into(),
                carrier: WORKER.into(),
                source_building: building(),
                source_position: [8, 64, 8],
                destination_building: building(),
                destination_position: [20, 64, 8],
                source_slot: 0,
                quantity: 3,
                waypoints: vec![[7, 64, 8]],
                max_ticks: 100,
                paid_mandate: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            ScriptSettlementEconomyAction::CaravanOpen {
                id: "run-one".into(),
                carrier: WORKER.into(),
                source_building: building(),
                source_position: [8, 64, 8],
                destination_building: building(),
                destination_position: [20, 64, 8],
                source_slot: 0,
                quantity: 3,
                waypoints: route(),
                max_ticks: 0,
                paid_mandate: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            ScriptSettlementEconomyAction::CaravanBlock {
                id: "run-one".into(),
                reason: String::new(),
            }
            .validate()
            .is_err()
        );
        assert!(
            ScriptSettlementInventoryEndpoint::Carry {
                uuid: "not-a-uuid".into(),
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn economic_observations_validate_meaningfully() {
        let mut production = ScriptSettlementProductionSnapshot::new(
            "job-one".into(),
            WORKER.into(),
            "solaris:boards".into(),
            "rev-one".into(),
            3,
        );
        production.completed = 1;
        production.next_due = 9;
        production.validate().unwrap();
        production.completed = 4;
        assert!(production.validate().is_err());
        production.completed = 1;

        let employment =
            ScriptSettlementEmploymentSnapshot::new("1".into(), WORKER.into(), treasury(), 4, 5, 6);
        employment.validate().unwrap();

        let quote = ScriptSettlementQuoteSnapshot::new(
            "2".into(),
            treasury(),
            carry(WORKER),
            "minecraft:bread".into(),
            3,
            1,
        );
        quote.validate().unwrap();

        let mut mandate = ScriptSettlementMandateSnapshot::new(
            "3".into(),
            "run-one".into(),
            treasury(),
            carry(OTHER),
            carry(WORKER),
            2,
        );
        mandate.validate().unwrap();
        mandate.status = "paid".into();
        assert!(
            mandate.validate().is_err(),
            "paid mandates are retired, not live"
        );
        mandate.status = "lost".into();
        mandate.validate().unwrap();

        let mut shipment = ScriptSettlementShipmentSnapshot::new(
            "run-one".into(),
            WORKER.into(),
            treasury(),
            treasury(),
            "minecraft:bread".into(),
            3,
        );
        shipment.waypoints = route();
        shipment.waypoint = 1;
        shipment.stage = "transit".into();
        shipment.validate().unwrap();
        shipment.stage = "blocked".into();
        assert!(shipment.validate().is_err());
        shipment.blocked = Some("rockfall".into());
        shipment.validate().unwrap();

        let mut economy = ScriptSettlementEconomySnapshot::new();
        economy.production.push(production);
        economy.employments.push(employment);
        economy.quotes.push(quote);
        economy.mandates.push(mandate);
        economy.shipments.push(shipment);
        economy.completed_quote = 9;
        economy.validate().unwrap();
        let encoded = serde_json::to_vec(&economy).unwrap();
        let restored: ScriptSettlementEconomySnapshot = serde_json::from_slice(&encoded).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored, economy);
    }

    #[test]
    fn live_snapshots_survive_watermarks_and_scale_past_sixty_four() {
        let mut economy = ScriptSettlementEconomySnapshot::new();
        economy.completed_quote = 2;
        economy.settled_mandate = 5;
        economy.ended_employment = 7;
        for index in 1..=80u32 {
            economy.quotes.push(ScriptSettlementQuoteSnapshot::new(
                index.to_string(),
                treasury(),
                carry(WORKER),
                "minecraft:bread".into(),
                1,
                1,
            ));
            economy
                .employments
                .push(ScriptSettlementEmploymentSnapshot::new(
                    index.to_string(),
                    WORKER.into(),
                    treasury(),
                    1,
                    1,
                    100,
                ));
            economy.mandates.push(ScriptSettlementMandateSnapshot::new(
                index.to_string(),
                format!("run-{index}"),
                treasury(),
                carry(OTHER),
                carry(WORKER),
                1,
            ));
        }
        economy.validate().unwrap();
        let encoded = serde_json::to_vec(&economy).unwrap();
        let restored: ScriptSettlementEconomySnapshot = serde_json::from_slice(&encoded).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.quotes.len(), 80);
        assert_eq!(restored.employments.len(), 80);
        assert_eq!(restored.mandates.len(), 80);
    }
}
