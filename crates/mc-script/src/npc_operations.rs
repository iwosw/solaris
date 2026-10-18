//! Durable NPC identity and lifecycle DTOs.
//!
//! The engine owns UUID bindings and physical entity state. Plugins own
//! population, social membership, housing, roles and admission policy.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    MAX_SCRIPT_WORLD_TIME, ScriptDtoError, ScriptInventoryEndpoint, ScriptInventoryMaterial,
    ScriptPosition, validate_bounded_nonempty, validate_script_id,
};

/// Maximum number of opaque handles one `query_npcs` call may address.
pub const MAX_NPC_QUERY_HANDLES: usize = 64;
/// Maximum number of npc records returned by one query page.
pub const MAX_NPC_PAGE: usize = 64;
/// Maximum byte length of an opaque npc handle.
pub const MAX_NPC_HANDLE_BYTES: usize = 64;
/// Maximum byte length of a canonical entity UUID string.
pub const MAX_NPC_ENTITY_UUID_BYTES: usize = 36;
/// Maximum byte length of a deterministic inhabitant generation id.
pub const MAX_NPC_GENERATION_ID_BYTES: usize = 64;
/// Maximum carried item summaries exposed by one npc snapshot.
pub const MAX_NPC_CARRIED_ITEMS: usize = 8;
/// Maximum distance between the claiming actor and the adopted entity.
pub const MAX_NPC_CLAIM_DISTANCE: f64 = 64.0;

const GENERATION_ID_BYTES: usize = 32;

/// Closed lifecycle state of one durable npc handle.
///
/// An unloaded NPC keeps its identity; a dead NPC leaves a tombstone.
/// Neither state permits silently rebinding its handle to another entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptNpcLifecycle {
    AliveLoaded,
    AliveUnloaded,
    Dead,
    Released,
}

impl ScriptNpcLifecycle {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AliveLoaded => "alive_loaded",
            Self::AliveUnloaded => "alive_unloaded",
            Self::Dead => "dead",
            Self::Released => "released",
        }
    }
}

/// One carried item summary read from the live entity snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcItemSummary {
    pub item_id: String,
    pub count: u32,
}

impl ScriptNpcItemSummary {
    #[must_use]
    pub fn new(item_id: String, count: u32) -> Self {
        Self { item_id, count }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::check_contract_resource_id(&self.item_id)?;
        if self.count == 0 || self.count > i32::MAX as u32 {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// Live entity state for a npc whose chunk is currently loaded.
///
/// `Eq` is implemented by hand because the pose and health are floats: every
/// value is validated finite before it becomes durable, so the marker trait
/// cannot silently accept a NaN identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcLoadedState {
    /// Dimension bound to the native owner that supplied this loaded pose.
    pub dimension: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub health: f32,
    pub max_health: f32,
    pub yaw: f32,
    pub carried: Vec<ScriptNpcItemSummary>,
    #[serde(default)]
    pub native_age_ticks: Option<i32>,
    #[serde(default)]
    pub birth_origin: Option<crate::ScriptNativeBirthOrigin>,
}

impl Eq for ScriptNpcLoadedState {}

impl ScriptNpcLoadedState {
    pub fn new(
        dimension: String,
        position: ScriptPosition,
        health: f32,
        max_health: f32,
        yaw: f32,
        carried: Vec<ScriptNpcItemSummary>,
    ) -> Self {
        Self {
            dimension,
            x: position.x(),
            y: position.y(),
            z: position.z(),
            health,
            max_health,
            yaw,
            carried,
            native_age_ticks: None,
            birth_origin: None,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::check_contract_resource_id(&self.dimension)?;
        if ScriptPosition::try_new(self.x, self.y, self.z).is_none()
            || !self.health.is_finite()
            || self.health < 0.0
            || !self.max_health.is_finite()
            || self.max_health <= 0.0
            || self.health > self.max_health
            || !self.yaw.is_finite()
            || self.carried.len() > MAX_NPC_CARRIED_ITEMS
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        for item in &self.carried {
            item.validate()?;
        }
        if let Some(origin) = &self.birth_origin {
            origin.validate()?;
        }
        Ok(())
    }
}

/// Bounded core-owned snapshot of one durable npc handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcSnapshot {
    pub handle: String,
    pub entity_uuid: String,
    pub lifecycle: ScriptNpcLifecycle,
    pub revision: u64,
    pub generation_id: Option<String>,
    pub loaded: Option<ScriptNpcLoadedState>,
}

impl ScriptNpcSnapshot {
    #[must_use]
    pub fn new(
        handle: String,
        entity_uuid: String,
        lifecycle: ScriptNpcLifecycle,
        revision: u64,
        generation_id: Option<String>,
        loaded: Option<ScriptNpcLoadedState>,
    ) -> Self {
        Self {
            handle,
            entity_uuid,
            lifecycle,
            revision,
            generation_id,
            loaded,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_npc_handle(&self.handle)?;
        validate_entity_uuid(&self.entity_uuid)?;
        if self.revision > MAX_SCRIPT_WORLD_TIME
            || (self.lifecycle == ScriptNpcLifecycle::AliveLoaded) != self.loaded.is_some()
        {
            return Err(ScriptDtoError::InconsistentResult {
                field: "npc snapshot",
            });
        }
        if let Some(generation_id) = &self.generation_id {
            validate_bounded_nonempty(
                "npc generation id",
                generation_id,
                MAX_NPC_GENERATION_ID_BYTES,
            )?;
        }
        if let Some(loaded) = &self.loaded {
            loaded.validate()?;
        }
        Ok(())
    }
}

/// Closed typed payload for one committed npc mutation or query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptNpcResult {
    Snapshot {
        npc: ScriptNpcSnapshot,
    },
    Page {
        npcs: Vec<ScriptNpcSnapshot>,
        cursor: Option<String>,
    },
    BirthPage {
        births: Vec<crate::ScriptNativeBirthObservation>,
        cursor: Option<u64>,
        /// The expired-through observation cursor; zero means complete history.
        history_start: u64,
        /// Last inspected sequence, including a final page without a next page.
        resume_cursor: u64,
    },
    HealthChanged {
        handle: String,
    },
}

impl ScriptNpcResult {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::BirthPage {
                births,
                cursor,
                history_start,
                resume_cursor,
            } => {
                if births.len() > MAX_NPC_PAGE
                    || *history_start > MAX_SCRIPT_WORLD_TIME
                    || *resume_cursor < *history_start
                    || *resume_cursor > MAX_SCRIPT_WORLD_TIME
                    || cursor.is_some_and(|cursor| cursor != *resume_cursor)
                    || cursor.is_some_and(|cursor| {
                        cursor <= *history_start || cursor > MAX_SCRIPT_WORLD_TIME
                    })
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut uuids = std::collections::BTreeSet::new();
                for birth in births {
                    birth.validate()?;
                    if !uuids.insert(&birth.entity_uuid) {
                        return Err(ScriptDtoError::InvalidBounds);
                    }
                }
                Ok(())
            }
            Self::Snapshot { npc } => npc.validate(),
            Self::HealthChanged { handle } => validate_npc_handle(handle),
            Self::Page { npcs, cursor } => {
                if npcs.len() > MAX_NPC_PAGE {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "npc page",
                        max: MAX_NPC_PAGE,
                    });
                }
                let mut previous = None;
                for npc in npcs {
                    npc.validate()?;
                    if previous.is_some_and(|handle: &str| handle >= npc.handle.as_str()) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "npc page order",
                        });
                    }
                    previous = Some(npc.handle.as_str());
                }
                if let Some(cursor) = cursor {
                    validate_npc_handle(cursor)?;
                }
                Ok(())
            }
        }
    }
}

/// One bounded npc call inside the durable operation envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptNpcOperation {
    Adopt {
        operation_id: String,
        actor_id: u64,
        entity_uuid: String,
        expected_entity_revision: u64,
    },
    /// Bind the original native child using this plugin's ownership of both
    /// native parents, rather than impersonating an online player.
    AdoptNativeChild {
        operation_id: String,
        entity_uuid: String,
        parent_handles: [String; 2],
        expected_entity_revision: u64,
    },
    Spawn {
        operation_id: String,
        generation_id: String,
        position: ScriptPosition,
        entity_type: String,
    },
    Query {
        handles: Vec<String>,
        cursor: Option<String>,
    },
    QueryBirths {
        parent_handles: Vec<String>,
        cursor: Option<u64>,
        limit: u16,
    },
    Release {
        operation_id: String,
        handle: String,
        expected_revision: u64,
    },
    /// Atomic exact item cost and nonlethal signed health change.
    ApplyHealthDelta {
        operation_id: String,
        handle: String,
        expected_revision: u64,
        source: ScriptInventoryEndpoint,
        material: ScriptInventoryMaterial,
        health_delta_milli: i32,
    },
}

impl ScriptNpcOperation {
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::Adopt { operation_id, .. }
            | Self::AdoptNativeChild { operation_id, .. }
            | Self::Spawn { operation_id, .. }
            | Self::Release { operation_id, .. }
            | Self::ApplyHealthDelta { operation_id, .. } => Some(operation_id),
            Self::Query { .. } | Self::QueryBirths { .. } => None,
        }
    }

    /// Canonical ordering so one operation id always fingerprints identically.
    pub fn canonicalize(&mut self) {
        if let Self::Query { handles, .. } = self {
            handles.sort_unstable();
            handles.dedup();
        }
        if let Self::QueryBirths { parent_handles, .. } = self
            && parent_handles.len() <= MAX_NPC_QUERY_HANDLES
        {
            parent_handles.sort_unstable();
            parent_handles.dedup();
        }
        if let Self::AdoptNativeChild { parent_handles, .. } = self {
            parent_handles.sort_unstable();
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(operation_id) = self.operation_id() {
            validate_script_id(operation_id)?;
        }
        match self {
            Self::AdoptNativeChild {
                entity_uuid,
                parent_handles,
                expected_entity_revision,
                ..
            } => {
                validate_entity_uuid(entity_uuid)?;
                for handle in parent_handles {
                    validate_npc_handle(handle)?;
                }
                if parent_handles[0] >= parent_handles[1]
                    || *expected_entity_revision > MAX_SCRIPT_WORLD_TIME
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::QueryBirths {
                parent_handles,
                cursor,
                limit,
            } => {
                if parent_handles.is_empty()
                    || parent_handles.len() > MAX_NPC_QUERY_HANDLES
                    || *limit == 0
                    || usize::from(*limit) > MAX_NPC_PAGE
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut previous = None;
                for handle in parent_handles {
                    validate_npc_handle(handle)?;
                    if previous.is_some_and(|value: &str| value >= handle.as_str()) {
                        return Err(ScriptDtoError::InvalidBounds);
                    }
                    previous = Some(handle.as_str());
                }
                if cursor.is_some_and(|cursor| cursor > MAX_SCRIPT_WORLD_TIME) {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Adopt {
                actor_id,
                entity_uuid,
                expected_entity_revision,
                ..
            } => {
                if *actor_id == 0 || *actor_id > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                validate_entity_uuid(entity_uuid)?;
                if *expected_entity_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Spawn {
                generation_id,
                position,
                entity_type,
                ..
            } => {
                validate_generation_id(generation_id)?;
                crate::check_contract_resource_id(entity_type)?;
                if ScriptPosition::try_new(position.x(), position.y(), position.z()).is_none() {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Query { handles, cursor } => {
                if handles.len() > MAX_NPC_QUERY_HANDLES {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "npc query handles",
                        max: MAX_NPC_QUERY_HANDLES,
                    });
                }
                let mut previous = None;
                for handle in handles {
                    validate_npc_handle(handle)?;
                    if previous.is_some_and(|value: &str| value >= handle.as_str()) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "npc query order",
                        });
                    }
                    previous = Some(handle.as_str());
                }
                if let Some(cursor) = cursor {
                    validate_npc_handle(cursor)?;
                }
                Ok(())
            }
            Self::Release {
                handle,
                expected_revision,
                ..
            } => {
                validate_npc_handle(handle)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::ApplyHealthDelta {
                handle,
                expected_revision,
                source,
                material,
                health_delta_milli,
                ..
            } => {
                validate_npc_handle(handle)?;
                source.validate()?;
                crate::check_contract_resource_id(&material.resource_id)?;
                if material.quantity == 0
                    || material.quantity > 4096
                    || *expected_revision > MAX_SCRIPT_WORLD_TIME
                    || *health_delta_milli == 0
                    || health_delta_milli.unsigned_abs() > 1_000_000
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
        }
    }
}

pub fn validate_npc_handle(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("npc handle", value, MAX_NPC_HANDLE_BYTES)
}

pub fn validate_entity_uuid(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("npc entity uuid", value, MAX_NPC_ENTITY_UUID_BYTES)?;
    if value.len() != 36
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
    {
        return Err(ScriptDtoError::InvalidId {
            field: "npc entity uuid",
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

/// Derive the owner-scoped opaque handle for one generation identity.
pub fn npc_handle_for_generation(
    plugin_id: &str,
    generation_id: &str,
) -> Result<String, ScriptDtoError> {
    validate_bounded_nonempty("plugin id", plugin_id, crate::MAX_PLUGIN_ID_BYTES)?;
    validate_bounded_nonempty(
        "npc generation id",
        generation_id,
        MAX_NPC_GENERATION_ID_BYTES,
    )?;
    Ok(domain_handle(plugin_id, "generation", generation_id))
}

/// Derive the owner-scoped opaque handle for one adopted entity UUID.
pub fn npc_handle_for_entity(plugin_id: &str, entity_uuid: &str) -> Result<String, ScriptDtoError> {
    validate_bounded_nonempty("plugin id", plugin_id, crate::MAX_PLUGIN_ID_BYTES)?;
    validate_entity_uuid(entity_uuid)?;
    Ok(domain_handle(plugin_id, "entity", entity_uuid))
}

/// Derive the deterministic entity UUID for one generation identity.
///
/// Core materialises the npc exactly once per generation id because this
/// UUID is stable: re-materialising a chunk finds the existing entity instead of
/// creating a second npc.
pub fn npc_entity_uuid(generation_id: &str) -> Result<[u8; 16], ScriptDtoError> {
    validate_bounded_nonempty(
        "npc generation id",
        generation_id,
        MAX_NPC_GENERATION_ID_BYTES,
    )?;
    let mut hasher = Sha256::new();
    hasher.update(b"solaris.npc.entity.v1");
    hasher.update(generation_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 variant / version 8 (custom) so the value stays a valid UUID.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(bytes)
}

fn domain_handle(plugin_id: &str, domain: &str, identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"solaris.npc.handle.v1");
    hasher.update(domain.as_bytes());
    hasher.update(plugin_id.as_bytes());
    hasher.update(identity.as_bytes());
    hex_prefix(hasher.finalize().as_slice(), GENERATION_ID_BYTES)
}

fn hex_prefix(bytes: &[u8], length: usize) -> String {
    let mut value = String::with_capacity(length * 2);
    for byte in bytes.iter().take(length) {
        value.push_str(&format!("{byte:02x}"));
    }
    value
}

/// Validate one plugin-supplied stable generation identity.
pub fn validate_generation_id(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("npc generation id", value, MAX_NPC_GENERATION_ID_BYTES)?;
    Ok(())
}
