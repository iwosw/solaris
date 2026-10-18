//! Actual native loot publication receipts. They describe canonical custody
//! already transferred at an acknowledged world/native decision, never policy.

use serde::{Deserialize, Serialize};

use crate::{MAX_SCRIPT_WORLD_TIME, ScriptDtoError, ScriptInventoryItem, ScriptPosition};

pub const MAX_NPC_DEATH_LOOT: usize =
    (crate::MAX_NPC_CARRY_SLOTS as usize + crate::MAX_NPC_EQUIPMENT_SLOTS as usize) * 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptNpcDeathInventory {
    Carry,
    Equipment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcDeathLoot {
    pub inventory: ScriptNpcDeathInventory,
    pub slot: u16,
    pub entity_uuid: String,
    pub item: ScriptInventoryItem,
}

impl ScriptNpcDeathLoot {
    #[must_use]
    pub fn new(
        inventory: ScriptNpcDeathInventory,
        slot: u16,
        entity_uuid: String,
        item: ScriptInventoryItem,
    ) -> Self {
        Self {
            inventory,
            slot,
            entity_uuid,
            item,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcDeathCargoReceipt {
    pub receipt_ref: String,
    pub handle: String,
    pub entity_uuid: String,
    pub dimension: String,
    pub position: ScriptPosition,
    pub yaw: f32,
    pub pitch: f32,
    pub death_tick: u64,
    pub source_inventory_revision: u64,
    pub executor_epoch: Option<u64>,
    pub world_decision_id: u64,
    pub loot: Vec<ScriptNpcDeathLoot>,
}

// All public receipt boundaries reject non-finite angles, as for loaded poses.
impl Eq for ScriptNpcDeathCargoReceipt {}

impl ScriptNpcDeathCargoReceipt {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        receipt_ref: String,
        handle: String,
        entity_uuid: String,
        dimension: String,
        position: ScriptPosition,
        yaw: f32,
        pitch: f32,
        death_tick: u64,
        source_inventory_revision: u64,
        executor_epoch: Option<u64>,
        world_decision_id: u64,
        loot: Vec<ScriptNpcDeathLoot>,
    ) -> Self {
        Self {
            receipt_ref,
            handle,
            entity_uuid,
            dimension,
            position,
            yaw,
            pitch,
            death_tick,
            source_inventory_revision,
            executor_epoch,
            world_decision_id,
            loot,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_script_id_value(&self.receipt_ref)?;
        crate::validate_npc_handle(&self.handle)?;
        crate::validate_entity_uuid(&self.entity_uuid)?;
        crate::check_contract_resource_id(&self.dimension)?;
        if !self.yaw.is_finite()
            || !self.pitch.is_finite()
            || self.death_tick > MAX_SCRIPT_WORLD_TIME
            || self.source_inventory_revision > MAX_SCRIPT_WORLD_TIME
            || self
                .executor_epoch
                .is_some_and(|epoch| epoch == 0 || epoch > MAX_SCRIPT_WORLD_TIME)
            || self.world_decision_id == 0
            || self.world_decision_id > MAX_SCRIPT_WORLD_TIME
            || self.loot.len() > MAX_NPC_DEATH_LOOT
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut uuids = std::collections::BTreeSet::new();
        for loot in &self.loot {
            crate::validate_entity_uuid(&loot.entity_uuid)?;
            loot.item.validate()?;
            let slots = match loot.inventory {
                ScriptNpcDeathInventory::Carry => crate::MAX_NPC_CARRY_SLOTS,
                ScriptNpcDeathInventory::Equipment => crate::MAX_NPC_EQUIPMENT_SLOTS,
            };
            if loot.slot >= u16::from(slots)
                || loot.item.count > 64
                || loot.entity_uuid == self.entity_uuid
                || !uuids.insert(&loot.entity_uuid)
            {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}
