//! # mc-script
//!
//! Runtime-independent script contracts: the server-owned boundary, immutable
//! event snapshots entering runtimes, bounded command batches leaving them, and
//! the plugin metadata both sides share.
//!
//! No VM lives in this crate. A host crate owns guest execution and reaches this
//! boundary through the trusted host methods below: it submits events, takes
//! control input (a reload envelope whose payload only the host interprets), and
//! commits a candidate deployment through the same route and command admission
//! every other plugin command takes.

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;

mod command_admission;
pub use command_admission::{
    AdmittedScriptCommand, HostCommandAdmission, ScriptCommandAcceptanceError,
    ScriptCommandProvenance, ScriptPluginTarget,
};

mod client_view;
pub use client_view::{
    MAX_CLIENT_VIEW_ACTIONS, MAX_CLIENT_VIEW_CELL_BYTES, MAX_CLIENT_VIEW_DENY_REASON_BYTES,
    MAX_CLIENT_VIEW_EXACT_INTEGER, MAX_CLIENT_VIEW_FIELDS, MAX_CLIENT_VIEW_ID_BYTES,
    MAX_CLIENT_VIEW_MARKER_RADIUS, MAX_CLIENT_VIEW_MARKERS, MAX_CLIENT_VIEW_PACKET_BYTES,
    MAX_CLIENT_VIEW_RANGE_LIMIT, MAX_CLIENT_VIEW_REASON_BYTES, MAX_CLIENT_VIEW_RESOURCES,
    MAX_CLIENT_VIEW_ROWS, MAX_CLIENT_VIEW_SELECTION_TTL_TICKS, MAX_CLIENT_VIEW_TABS,
    MAX_CLIENT_VIEW_TEXT_BYTES, MAX_CLIENT_VIEW_TITLE_BYTES, ScriptClientSelectionBegin,
    ScriptClientSelectionConstraints, ScriptClientViewAction, ScriptClientViewField,
    ScriptClientViewFieldValue, ScriptClientViewFormation, ScriptClientViewKey,
    ScriptClientViewMarker, ScriptClientViewMarkerPosition, ScriptClientViewModel,
    ScriptClientViewOpen, ScriptClientViewPresent, ScriptClientViewResourceEntry,
    ScriptClientViewRow, ScriptClientViewTab,
};
use command_admission::HostAdmissionLedger;

mod command_permissions;
pub use command_permissions::{
    MAX_COMMAND_PERMISSION_ENTRIES, MAX_COMMAND_PERMISSION_ENTRY_BYTES,
    MAX_COMMAND_PERMISSION_NODE_BYTES, ScriptCommandPermissionError, ScriptCommandPermissions,
};

mod client_sound;
pub use client_sound::{ScriptClientSound, ScriptClientSoundPlayback};

mod commit_events;
pub use commit_events::{
    ScriptCommitEnqueueError, ScriptCommitEventMonitor, ScriptCommitEventOutbox,
    ScriptCommitEventOutboxSnapshot, ScriptCommitEventReceiver,
};

mod inventory_operations;
pub use inventory_operations::{
    MAX_INVENTORY_RESOURCE_TYPES, MAX_INVENTORY_WORK_PORTIONS, MAX_NPC_CARRY_SLOTS,
    MAX_NPC_EQUIPMENT_SLOTS, MAX_OWNED_INVENTORY_ITEM_BUDGET, MAX_OWNED_INVENTORY_SLOTS,
    MAX_OWNED_INVENTORY_TRANSFERS, ScriptInventoryEnchantment, ScriptInventoryEndpoint,
    ScriptInventoryExpectedRevision, ScriptInventoryFence, ScriptInventoryItem,
    ScriptInventoryMaterial, ScriptInventoryPlacementSlot, ScriptInventoryReservationQuantity,
    ScriptInventoryReservationSnapshot, ScriptInventoryResourcePlan, ScriptInventorySlot,
    ScriptInventoryStewEffect, ScriptInventoryWorkPortion, ScriptOwnedInventoryOperation,
    ScriptOwnedInventoryResult, ScriptOwnedInventorySnapshot, ScriptOwnedItemTransfer,
};

mod operations;
pub use operations::{
    MAX_STORAGE_SCAN_PAGE, ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome,
    ScriptOperationPayload, ScriptOperationRequest, ScriptOperationState,
    ScriptPluginStorageVersionGuard, ScriptStorageChange, ScriptStorageEntry,
    validate_plugin_storage_version_guards,
};

mod native_birth_operations;
pub use native_birth_operations::{ScriptNativeBirthObservation, ScriptNativeBirthOrigin};

mod settlement_operations;
pub use settlement_operations::{
    MAX_HOUSEHOLD_ID_BYTES, MAX_HOUSEHOLD_POLICY_BYTES, MAX_SETTLEMENT_DISTRICT_BUILDINGS,
    MAX_SETTLEMENT_DISTRICT_REASON_BYTES, MAX_SETTLEMENT_NAME_BYTES, MAX_SETTLEMENT_PAGE,
    MAX_SETTLEMENT_QUERY_NAMES, ScriptSettlementBuildingRef, ScriptSettlementDistrictSnapshot,
    ScriptSettlementHouseholdSnapshot, ScriptSettlementOperation, ScriptSettlementPage,
    ScriptSettlementQuery, ScriptSettlementResult, ScriptSettlementSnapshot,
};

mod settlement_economy;
pub use settlement_economy::{
    MAX_SETTLEMENT_CARAVAN_QUANTITY, MAX_SETTLEMENT_CARAVAN_REASON_BYTES,
    MAX_SETTLEMENT_CARAVAN_TICKS, MAX_SETTLEMENT_CARAVAN_WAYPOINTS, ScriptSettlementEconomyAction,
    ScriptSettlementEconomySnapshot, ScriptSettlementEmploymentSnapshot,
    ScriptSettlementInventoryEndpoint, ScriptSettlementMandateSnapshot,
    ScriptSettlementProductionSnapshot, ScriptSettlementQuoteSnapshot,
    ScriptSettlementShipmentSnapshot,
};

mod npc_operations;
pub use npc_operations::{
    MAX_NPC_CARRIED_ITEMS, MAX_NPC_CLAIM_DISTANCE, MAX_NPC_ENTITY_UUID_BYTES,
    MAX_NPC_GENERATION_ID_BYTES, MAX_NPC_HANDLE_BYTES, MAX_NPC_PAGE, MAX_NPC_QUERY_HANDLES,
    ScriptNpcItemSummary, ScriptNpcLifecycle, ScriptNpcLoadedState, ScriptNpcOperation,
    ScriptNpcResult, ScriptNpcSnapshot, npc_entity_uuid, npc_handle_for_entity,
    npc_handle_for_generation, validate_entity_uuid, validate_generation_id, validate_npc_handle,
};

mod npc_order_operations;
pub use npc_order_operations::{
    MAX_COMBAT_EVENTS, MAX_ENGAGEMENT_RADIUS, MAX_FORMATION_SPACING, MAX_NPC_ORDER_HANDLES,
    MAX_ORDER_AFFILIATIONS, MAX_ORDER_TARGETS, MAX_ORDER_WAYPOINTS, MAX_RECIPE_ID_BYTES,
    MAX_SCRIPT_BLOCK_COORDINATE, MAX_TARGET_REF_BYTES, MAX_WORK_AREA_AXIS, MAX_WORK_RECEIPTS,
    MAX_WORK_UNITS, MIN_ENGAGEMENT_RADIUS, MIN_FORMATION_SPACING, MIN_ORDER_WAYPOINTS,
    ScriptBlockPosition, ScriptCombatEvent, ScriptCraftReceipt, ScriptCraftStack,
    ScriptCraftToolWear, ScriptEngagementPolicy, ScriptFormation, ScriptFormationKind,
    ScriptHostileCategory, ScriptItemChange, ScriptNpcCombatControl, ScriptNpcControlSnapshot,
    ScriptNpcItemUseControl, ScriptNpcOrder, ScriptNpcOrderOperation, ScriptNpcOrderResult,
    ScriptNpcPendingWork, ScriptNpcTargetObservation, ScriptNpcWorkOrder, ScriptOrderMemberOutcome,
    ScriptOrderMemberState, ScriptOrderTarget, ScriptOrderTargetRef, ScriptWorkArea,
    ScriptWorkAssignment, ScriptWorkPauseReason, ScriptWorkState, validate_recipe,
    validate_work_area_axis, validate_work_units,
};

mod npc_route_operations;
pub use npc_route_operations::{
    MAX_NPC_ROUTE_REF_BYTES, MAX_NPC_ROUTE_RESERVATIONS, MAX_NPC_ROUTE_SEGMENT_CELLS,
    MAX_NPC_ROUTE_TICKS, MAX_NPC_ROUTE_WAYPOINTS, ScriptNpcExecutionPermit, ScriptNpcExecutorMode,
    ScriptNpcRouteAdmission, ScriptNpcRouteArrival, ScriptNpcRouteBlockReason,
    ScriptNpcRouteHandoff, ScriptNpcRouteOperation, ScriptNpcRouteResult, ScriptNpcRouteState,
    ScriptNpcRouteStatus, validate_route_ref,
};

mod npc_death_receipts;
pub use npc_death_receipts::{
    MAX_NPC_DEATH_LOOT, ScriptNpcDeathCargoReceipt, ScriptNpcDeathInventory, ScriptNpcDeathLoot,
};

mod world_structure_operations;
pub use world_structure_operations::{
    MAX_BLUEPRINT_FOOTPRINT_AXIS, MAX_BLUEPRINT_ID_BYTES, MAX_STRUCTURE_ACTIVE_PER_PLUGIN,
    MAX_STRUCTURE_ID_BYTES, MAX_STRUCTURE_RESOURCE_TYPES, MAX_STRUCTURE_STAGES,
    MAX_SURVEY_BOUNDS_AXIS, MAX_SURVEY_TAGS, MAX_SURVEY_TOKEN_BYTES, MAX_WORKSITE_POINTS,
    MAX_WORLD_BLOCK_PROPERTIES, MAX_WORLD_BLOCK_PROPERTY_BYTES, MAX_WORLD_BLOCK_QUERY_POSITIONS,
    MAX_WORLD_BOUNDS_AXIS, MAX_WORLD_COMMIT_PORTION, MAX_WORLD_FEATURE_AXIS,
    MAX_WORLD_FEATURE_BLOCKS, MAX_WORLD_FEATURE_ENTITIES, MAX_WORLD_FEATURE_ID_BYTES,
    MAX_WORLD_FEATURE_PAGE, ScriptChunkAvailability, ScriptStructureCheck, ScriptStructureMaterial,
    ScriptStructureReceipt, ScriptStructureSnapshot, ScriptStructureStagePlan,
    ScriptStructureState, ScriptSurveySnapshot, ScriptWorksiteCheck, ScriptWorksiteFailure,
    ScriptWorksiteInteractionPoint, ScriptWorksitePointFailure, ScriptWorksiteRequest,
    ScriptWorksiteSnapshot, ScriptWorldBlockFacts, ScriptWorldBlockProperty, ScriptWorldBlockQuery,
    ScriptWorldBounds, ScriptWorldFeature, ScriptWorldFeatureBlock, ScriptWorldFeaturePage,
    ScriptWorldStructureOperation, ScriptWorldStructureResult,
};

mod plugin_metadata;
mod route_authority;
pub use plugin_metadata::{
    ClientBundle, ClientBundleDiscovery, ClientContentKind, ClientLoader, ClientPermission,
    PluginDeployment, PluginDiscovery, PluginWorldgenOreProfile,
};

pub mod precommit;

mod gameplay_rules;
mod manifest;
pub use gameplay_rules::{
    BiomeSpawns, ClayRule, GameplayRules, GameplayRulesError, SpawnEntry, SpawnPlacement, TreeRule,
};
pub use manifest::{
    CommandCapabilities, ScriptCommandCapability, ScriptCommandCapabilityKind,
    ScriptEventSubscription, ScriptPluginDependency, ScriptPluginDependencyRelation,
    ScriptPluginLoadPhase, ScriptPluginManifest, ScriptPluginManifestError,
    ValidatedScriptPluginManifest, validate_script_resource_id,
};

#[cfg(test)]
use manifest::is_supported_event_name;
use manifest::{RequiredCommandCapability, is_valid_plugin_id};

#[cfg(test)]
mod custom_payload_tests;
#[cfg(test)]
mod entity_interaction_tests;
#[cfg(test)]
mod entity_kill_tests;
#[cfg(test)]
mod host_control_tests;
#[cfg(test)]
mod inventory_operations_tests;
#[cfg(test)]
mod item_pickup_tests;
#[cfg(test)]
mod npc_operations_tests;
#[cfg(test)]
mod npc_order_operations_tests;
#[cfg(test)]
mod player_death_tests;
#[cfg(test)]
mod player_inventory_tests;
#[cfg(test)]
mod player_query_tests;
#[cfg(test)]
mod player_teleport_tests;
#[cfg(test)]
mod precommit_tests;
#[cfg(test)]
mod tick_delivery_tests;
#[cfg(test)]
mod world_structure_operations_tests;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The API version every deployed WebAssembly component package must request.
pub const COMPONENT_PLUGIN_API_VERSION: ScriptApiVersion = ScriptApiVersion::new(0, 8, 0);

/// Maximum entity types one plugin may allow-list for spawning.
pub const MAX_SPAWN_ENTITY_TYPES: usize = 32;

/// Maximum byte length of a script-visible namespaced resource identifier.
pub const MAX_SCRIPT_RESOURCE_ID_BYTES: usize = 128;
/// Largest integer preserved exactly by the component command boundary.
pub const MAX_SCRIPT_WORLD_TIME: u64 = (1_u64 << 53) - 1;
/// Maximum raw damage accepted by one bounded plugin combat request.
pub const MAX_SCRIPT_ENTITY_DAMAGE: f32 = 1_000_000.0;

/// Maximum byte length of a plugin-scoped identifier or request correlation id.
pub const MAX_SCRIPT_ID_BYTES: usize = 64;

pub const MAX_SCRIPT_PLAYER_UUID_BYTES: usize = 64;
pub const MAX_SCRIPT_PLAYER_NAME_BYTES: usize = 16;
pub const MAX_SCRIPT_CHAT_MESSAGE_BYTES: usize = 4_096;
pub const MAX_SCRIPT_DISCONNECT_REASON_BYTES: usize = 1_024;
/// Maximum raw custom-payload body accepted through the script boundary.
pub const MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES: usize = 32_768;
pub const MAX_SCRIPT_COMMAND_BATCH: usize = 32;
pub const MAX_SCRIPT_EVENT_QUEUE_CAPACITY: usize = 1_024;
pub const MAX_SCRIPT_COMMAND_QUEUE_CAPACITY: usize = 256;
/// Maximum connected-player snapshots returned by one plugin query.
pub const MAX_ONLINE_PLAYER_QUERY_LIMIT: usize = 256;
pub const MAX_PLUGIN_ID_BYTES: usize = 64;
pub const MAX_PLUGIN_DISPLAY_NAME_BYTES: usize = 128;
pub const MAX_PLUGIN_VERSION_BYTES: usize = 64;
pub const MAX_MANIFEST_EVENT_SUBSCRIPTIONS: usize = 64;
pub const MAX_MANIFEST_DEPENDENCIES: usize = 64;
pub const MAX_MANIFEST_CAPABILITIES: usize = 128;
/// Largest accepted `plugin.toml` for a component deployment.
pub const MAX_PLUGIN_MANIFEST_BYTES: usize = 64 * 1024;
/// Largest accepted `api = "MAJOR.MINOR.PATCH"` string.
pub const MAX_API_VERSION_BYTES: usize = 16;
pub const MAX_MANIFEST_PERMISSIONS: usize = 64;
pub const MAX_MANIFEST_COMMAND_PERMISSIONS: usize = 256;
pub const MAX_MANIFEST_FIELD_BYTES: usize = 128;

/// Maximum byte length of a plugin storage key.
pub const MAX_PLUGIN_STORAGE_KEY_BYTES: usize = 128;

/// Maximum byte length of a plugin storage value.
pub const MAX_PLUGIN_STORAGE_VALUE_BYTES: usize = 4_096;

/// Maximum number of server-owned menu slots a plugin may describe.
pub const MAX_INVENTORY_MENU_SLOTS: usize = 54;

/// Maximum atomic inventory and storage mutations in one plugin request.
pub const MAX_INVENTORY_STORAGE_MUTATIONS: usize = 16;

/// Maximum absolute resource count changed by one inventory delta.
pub const MAX_INVENTORY_RESOURCE_DELTA: i16 = 64;

/// Maximum byte length of a server-rendered inventory menu title.
pub const MAX_INVENTORY_MENU_TITLE_BYTES: usize = 128;

/// Maximum absolute horizontal coordinate accepted from a component command.
pub const SCRIPT_HORIZONTAL_COORDINATE_LIMIT: f64 = 30_000_000.0;

/// Maximum absolute vertical coordinate accepted from a component command.
pub const SCRIPT_VERTICAL_COORDINATE_LIMIT: f64 = 20_000_000.0;

/// Maximum byte length of one ASCII plugin player-command root.
pub const MAX_PLAYER_COMMAND_ROOT_BYTES: usize = 64;

/// Maximum number of active plugin player-command roots across the server.
pub const MAX_PLAYER_COMMAND_ROOTS: usize = 128;
/// Maximum number of exclusive custom-payload channels owned across the server.
pub const MAX_PLUGIN_PAYLOAD_CHANNELS: usize = 128;
/// Maximum command-tree nodes added by active plugin roots.
pub const MAX_PLAYER_COMMAND_TREE_NODES: usize = MAX_PLAYER_COMMAND_ROOTS * 2;

/// Player command roots reserved by Solaris' built-in command parser.
pub const BUILT_IN_PLAYER_COMMAND_ROOTS: &[&str] = &[
    "debug",
    "defaultgamemode",
    "gamemode",
    "gamerule",
    "give",
    "kill",
    "save-all",
    "settlement",
    "status",
    "stop",
    "summon",
    "teleport",
    "time",
    "tp",
];

const fn decimal_u8_len(value: u8) -> usize {
    if value >= 100 {
        3
    } else if value >= 10 {
        2
    } else {
        1
    }
}

/// Result of admitting a player command to a plugin-owned root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlayerCommandAdmission {
    NotOwned,
    Enqueued,
    Dropped,
    PermissionDenied,
    OwnedRejected { error: ScriptDtoError },
}

/// An authenticated command event prepared for the native owner's commit FIFO.
/// Preparation does not publish an event or claim queue admission.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PreparedPlayerCommand {
    NotOwned,
    PermissionDenied,
    OwnedRejected { error: ScriptDtoError },
    Event(ScriptEvent),
}

/// Version requested by a script runtime or supported by the server host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScriptApiVersion {
    major: u16,
    minor: u16,
    patch: u16,
}

impl ScriptApiVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub const fn major(&self) -> u16 {
        self.major
    }

    pub const fn minor(&self) -> u16 {
        self.minor
    }

    pub const fn patch(&self) -> u16 {
        self.patch
    }
}

/// Whether `requested` is exactly the expected plugin API version.
#[must_use]
pub const fn supports_plugin_api_version(
    requested: ScriptApiVersion,
    expected: ScriptApiVersion,
) -> bool {
    requested.major == expected.major
        && requested.minor == expected.minor
        && requested.patch == expected.patch
}

/// Stable player identifier snapshot for script-visible DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScriptPlayerId(u64);

impl ScriptPlayerId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Validated immutable position carried by script commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
#[serde(
    try_from = "ScriptPositionCoordinates",
    into = "ScriptPositionCoordinates"
)]
pub struct ScriptPosition {
    x_bits: u64,
    y_bits: u64,
    z_bits: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptPositionCoordinates {
    x: f64,
    y: f64,
    z: f64,
}

impl TryFrom<ScriptPositionCoordinates> for ScriptPosition {
    type Error = ScriptDtoError;

    fn try_from(value: ScriptPositionCoordinates) -> Result<Self, Self::Error> {
        Self::try_new(value.x, value.y, value.z).ok_or(ScriptDtoError::InvalidBounds)
    }
}

impl From<ScriptPosition> for ScriptPositionCoordinates {
    fn from(value: ScriptPosition) -> Self {
        Self {
            x: value.x(),
            y: value.y(),
            z: value.z(),
        }
    }
}

impl ScriptPosition {
    #[must_use]
    pub fn try_new(x: f64, y: f64, z: f64) -> Option<Self> {
        (x.is_finite()
            && y.is_finite()
            && z.is_finite()
            && x.abs() <= SCRIPT_HORIZONTAL_COORDINATE_LIMIT
            && y.abs() <= SCRIPT_VERTICAL_COORDINATE_LIMIT
            && z.abs() <= SCRIPT_HORIZONTAL_COORDINATE_LIMIT)
            .then_some(Self {
                x_bits: x.to_bits(),
                y_bits: y.to_bits(),
                z_bits: z.to_bits(),
            })
    }

    #[must_use]
    pub fn x(self) -> f64 {
        f64::from_bits(self.x_bits)
    }

    #[must_use]
    pub fn y(self) -> f64 {
        f64::from_bits(self.y_bits)
    }

    #[must_use]
    pub fn z(self) -> f64 {
        f64::from_bits(self.z_bits)
    }
}

/// Bounded same-dimension player teleport requested by one plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlayerTeleportRequest {
    request_id: String,
    player_id: ScriptPlayerId,
    position: ScriptPosition,
    dimension: String,
}

impl ScriptPlayerTeleportRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        dimension: impl AsRef<str>,
        position: ScriptPosition,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            player_id,
            dimension: validate_contract_resource_id(dimension.as_ref())?,
            position,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    pub const fn position(&self) -> ScriptPosition {
        self.position
    }
}

/// Exact reason why an admitted player teleport did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptPlayerTeleportFailure {
    PlayerUnavailable,
    DimensionMismatch,
    RuntimeUnavailable,
}

impl ScriptPlayerTeleportFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlayerUnavailable => "player_unavailable",
            Self::DimensionMismatch => "dimension_mismatch",
            Self::RuntimeUnavailable => "runtime_unavailable",
        }
    }
}

/// Typed request to set the authoritative overworld simulation time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptWorldTimeSetRequest {
    request_id: String,
    world_time: u64,
}

impl ScriptWorldTimeSetRequest {
    pub fn try_new(request_id: impl AsRef<str>, world_time: u64) -> Result<Self, ScriptDtoError> {
        if world_time > MAX_SCRIPT_WORLD_TIME {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            world_time,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn world_time(&self) -> u64 {
        self.world_time
    }
}

/// Exact public category for a world-time request that did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptWorldTimeSetFailure {
    Busy,
    RuntimeUnavailable,
    Rejected,
}

impl ScriptWorldTimeSetFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::Rejected => "rejected",
        }
    }
}

/// Typed request to set one default block state in the authoritative world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptWorldBlockSetRequest {
    request_id: String,
    dimension: String,
    block_id: String,
    x: i32,
    y: i32,
    z: i32,
}

impl ScriptWorldBlockSetRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        dimension: impl AsRef<str>,
        block_id: impl AsRef<str>,
        x: i32,
        y: i32,
        z: i32,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            dimension: validate_contract_resource_id(dimension.as_ref())?,
            block_id: validate_contract_resource_id(block_id.as_ref())?,
            x,
            y,
            z,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    pub fn block_id(&self) -> &str {
        &self.block_id
    }

    pub const fn x(&self) -> i32 {
        self.x
    }

    pub const fn y(&self) -> i32 {
        self.y
    }

    pub const fn z(&self) -> i32 {
        self.z
    }
}

/// Exact public category for a world-block request that did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptWorldBlockSetFailure {
    UnknownBlock,
    UnsupportedDimension,
    OutOfWorld,
    Busy,
    RuntimeUnavailable,
    Rejected,
}

impl ScriptWorldBlockSetFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownBlock => "unknown_block",
            Self::UnsupportedDimension => "unsupported_dimension",
            Self::OutOfWorld => "out_of_world",
            Self::Busy => "busy",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::Rejected => "rejected",
        }
    }
}

/// Typed request to grant one Loader-owned custom block item through player inventory authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptLoaderItemGrantRequest {
    request_id: String,
    player_id: ScriptPlayerId,
    block_id: String,
    count: u8,
}

impl ScriptLoaderItemGrantRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        block_id: impl AsRef<str>,
        count: u8,
    ) -> Result<Self, ScriptDtoError> {
        if !(1..=64).contains(&count) {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            player_id,
            block_id: validate_contract_resource_id(block_id.as_ref())?,
            count,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    pub fn block_id(&self) -> &str {
        &self.block_id
    }

    pub const fn count(&self) -> u8 {
        self.count
    }
}

/// Exact public category for a Loader custom-block item grant that did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptLoaderItemGrantFailure {
    LoaderUnavailable,
    NotOwned,
    PlayerUnavailable,
    InventoryFull,
    RuntimeUnavailable,
    Rejected,
}

impl ScriptLoaderItemGrantFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LoaderUnavailable => "loader_unavailable",
            Self::NotOwned => "not_owned",
            Self::PlayerUnavailable => "player_unavailable",
            Self::InventoryFull => "inventory_full",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::Rejected => "rejected",
        }
    }
}

/// Bounded request for a point-in-time connected-player snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptOnlinePlayersRequest {
    request_id: String,
    limit: usize,
}

impl ScriptOnlinePlayersRequest {
    pub fn try_new(request_id: impl AsRef<str>, limit: usize) -> Result<Self, ScriptDtoError> {
        if limit == 0 || limit > MAX_ONLINE_PLAYER_QUERY_LIMIT {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            limit,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn limit(&self) -> usize {
        self.limit
    }
}

/// Immutable server-authoritative player context attached to gameplay events.
///
/// This is a point-in-time value. It deliberately contains no connection or
/// network-address data and cannot be used to query live server state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlayerContext {
    snapshot: Box<ScriptPlayerContextSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptPlayerContextSnapshot {
    uuid: String,
    username: String,
    operator: bool,
    dimension: String,
    x_bits: u64,
    y_bits: u64,
    z_bits: u64,
}

impl ScriptPlayerContext {
    pub fn try_new(
        uuid: impl AsRef<str>,
        username: impl AsRef<str>,
        operator: bool,
        dimension: impl AsRef<str>,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<Self, ScriptDtoError> {
        let uuid = uuid.as_ref();
        let username = username.as_ref();
        validate_bounded_nonempty("player uuid", uuid, MAX_SCRIPT_PLAYER_UUID_BYTES)?;
        validate_bounded_nonempty("player username", username, MAX_SCRIPT_PLAYER_NAME_BYTES)?;
        validate_player_identity(uuid, username)?;
        let position = ScriptPosition::try_new(x, y, z).ok_or(ScriptDtoError::InvalidBounds)?;
        Ok(Self {
            snapshot: Box::new(ScriptPlayerContextSnapshot {
                uuid: uuid.to_owned(),
                username: username.to_owned(),
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                operator,
                x_bits: position.x_bits,
                y_bits: position.y_bits,
                z_bits: position.z_bits,
            }),
        })
    }

    #[must_use]
    pub fn new(
        uuid: impl AsRef<str>,
        username: impl AsRef<str>,
        operator: bool,
        dimension: impl AsRef<str>,
        x: f64,
        y: f64,
        z: f64,
    ) -> Self {
        Self::try_new(uuid, username, operator, dimension, x, y, z)
            .expect("server-authored script player context must be bounded")
    }

    #[must_use]
    pub fn uuid(&self) -> &str {
        &self.snapshot.uuid
    }

    #[must_use]
    pub fn username(&self) -> &str {
        &self.snapshot.username
    }

    #[must_use]
    pub fn operator(&self) -> bool {
        self.snapshot.operator
    }

    #[must_use]
    pub fn dimension(&self) -> &str {
        &self.snapshot.dimension
    }

    #[must_use]
    pub fn x(&self) -> f64 {
        f64::from_bits(self.snapshot.x_bits)
    }

    #[must_use]
    pub fn y(&self) -> f64 {
        f64::from_bits(self.snapshot.y_bits)
    }

    #[must_use]
    pub fn z(&self) -> f64 {
        f64::from_bits(self.snapshot.z_bits)
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("player uuid", self.uuid(), MAX_SCRIPT_PLAYER_UUID_BYTES)?;
        validate_bounded_nonempty(
            "player username",
            self.username(),
            MAX_SCRIPT_PLAYER_NAME_BYTES,
        )?;
        validate_player_identity(self.uuid(), self.username())?;
        check_contract_resource_id(self.dimension())?;
        ScriptPosition::try_new(self.x(), self.y(), self.z())
            .ok_or(ScriptDtoError::InvalidBounds)
            .map(drop)
    }
}

/// Immutable identity, pose, and dimension for one connected player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptOnlinePlayerSnapshot {
    player_id: ScriptPlayerId,
    context: ScriptPlayerContext,
    dimension: String,
}

impl ScriptOnlinePlayerSnapshot {
    pub fn try_new(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        let dimension = dimension.as_ref();
        validate_contract_resource_id(dimension)?;
        Ok(Self {
            player_id,
            context,
            dimension: dimension.to_owned(),
        })
    }

    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    pub fn context(&self) -> &ScriptPlayerContext {
        &self.context
    }

    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    fn validate(&self) -> Result<(), ScriptDtoError> {
        self.context.validate()?;
        validate_contract_resource_id(&self.dimension).map(drop)
    }
}

/// Stable entity identifier snapshot for script-visible DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScriptEntityId(u64);

impl ScriptEntityId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Exact public category for an entity-spawn request that did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptEntitySpawnFailure {
    UnknownEntityType,
    ActorUnavailable,
    Busy,
    RuntimeUnavailable,
    Rejected,
}

impl ScriptEntitySpawnFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownEntityType => "unknown_entity_type",
            Self::ActorUnavailable => "actor_unavailable",
            Self::Busy => "busy",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::Rejected => "rejected",
        }
    }
}

/// Typed request to apply one bounded server-owned damage mutation to a non-player entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEntityDamageRequest {
    request_id: String,
    entity_id: ScriptEntityId,
    amount_bits: u32,
}

impl ScriptEntityDamageRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        entity_id: ScriptEntityId,
        amount: f32,
    ) -> Result<Self, ScriptDtoError> {
        if entity_id.value() > i32::MAX as u64
            || !amount.is_finite()
            || !(0.0..=MAX_SCRIPT_ENTITY_DAMAGE).contains(&amount)
            || amount == 0.0
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            entity_id,
            amount_bits: amount.to_bits(),
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn entity_id(&self) -> ScriptEntityId {
        self.entity_id
    }

    pub const fn amount(&self) -> f32 {
        f32::from_bits(self.amount_bits)
    }
}

/// Exact public category for an entity-damage request that did not apply damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptEntityDamageFailure {
    Busy,
    RuntimeUnavailable,
    Rejected,
}

impl ScriptEntityDamageFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::Rejected => "rejected",
        }
    }
}

/// Validation failure for a bounded script DTO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptDtoError {
    InvalidId {
        field: &'static str,
        actual_bytes: usize,
    },
    InvalidResourceId {
        field: &'static str,
        actual_bytes: usize,
    },
    ValueTooLong {
        field: &'static str,
        max_bytes: usize,
        actual_bytes: usize,
    },
    EmptyValue {
        field: &'static str,
    },
    InconsistentResult {
        field: &'static str,
    },
    InvalidBounds,
    InvalidAmount,
    EmptyTransaction,
    TooManyEntries {
        field: &'static str,
        max: usize,
    },
    DuplicateId {
        field: &'static str,
        actual_bytes: usize,
    },
}

impl fmt::Display for ScriptDtoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId {
                field,
                actual_bytes,
            } => write!(formatter, "{field}:invalid_id:{actual_bytes}"),
            Self::InvalidResourceId {
                field,
                actual_bytes,
            } => write!(formatter, "{field}:invalid_resource_id:{actual_bytes}"),
            Self::ValueTooLong {
                field,
                max_bytes,
                actual_bytes,
            } => write!(formatter, "{field}:too_long:{actual_bytes}:{max_bytes}"),
            Self::EmptyValue { field } => write!(formatter, "{field}:empty"),
            Self::InconsistentResult { field } => write!(formatter, "{field}:inconsistent"),
            Self::InvalidBounds => formatter.write_str("bounds:invalid"),
            Self::InvalidAmount => formatter.write_str("amount:invalid"),
            Self::EmptyTransaction => formatter.write_str("transaction:empty"),
            Self::TooManyEntries { field, max } => {
                write!(formatter, "{field}:too_many:{max}")
            }
            Self::DuplicateId {
                field,
                actual_bytes,
            } => write!(formatter, "{field}:duplicate:{actual_bytes}"),
        }
    }
}

impl std::error::Error for ScriptDtoError {}

/// Maximum player uuids one protected zone's allow-list may name.
pub const MAX_SCRIPT_ZONE_ACTORS: usize = 32;

/// Generic actor policy attached to a protected zone by its owning plugin: the
/// players whose actions the server admits inside the zone (owner and trusted
/// members alike), plus whether everyone may use doors, trapdoors, fence gates,
/// buttons and levers. Every other build, container, ignition, explosion,
/// entity-interaction and damage action inside the zone is native-checked
/// against this list; operators are always admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptZoneProtection {
    allowed_actor_uuids: Vec<String>,
    public_use: bool,
}

impl ScriptZoneProtection {
    /// A non-empty allow-list of at most [`MAX_SCRIPT_ZONE_ACTORS`] distinct
    /// player uuids, normalized and sorted.
    pub fn try_new<I, S>(allowed_actor_uuids: I, public_use: bool) -> Result<Self, ScriptDtoError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut actors = std::collections::BTreeSet::new();
        for uuid in allowed_actor_uuids {
            if actors.len() == MAX_SCRIPT_ZONE_ACTORS {
                return Err(ScriptDtoError::TooManyEntries {
                    field: "protection actor uuids",
                    max: MAX_SCRIPT_ZONE_ACTORS,
                });
            }
            actors.insert(normalize_player_uuid(uuid.as_ref())?);
        }
        if actors.is_empty() {
            return Err(ScriptDtoError::EmptyValue {
                field: "protection actor uuids",
            });
        }
        Ok(Self {
            allowed_actor_uuids: actors.into_iter().collect(),
            public_use,
        })
    }

    pub fn allowed_actor_uuids(&self) -> &[String] {
        &self.allowed_actor_uuids
    }

    pub fn public_use(&self) -> bool {
        self.public_use
    }

    pub fn allows_actor(&self, actor_uuid: &str, operator: bool) -> bool {
        operator
            || normalize_player_uuid(actor_uuid)
                .is_ok_and(|actor_uuid| self.allowed_actor_uuids.binary_search(&actor_uuid).is_ok())
    }
}

/// A bounded axis-aligned zone definition. The server owns its lifecycle and membership checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptAxisAlignedZone {
    id: String,
    dimension: String,
    minimum: ScriptPosition,
    maximum: ScriptPosition,
    protection: Option<ScriptZoneProtection>,
}

impl ScriptAxisAlignedZone {
    pub fn try_new(
        id: impl AsRef<str>,
        dimension: impl AsRef<str>,
        minimum: ScriptPosition,
        maximum: ScriptPosition,
    ) -> Result<Self, ScriptDtoError> {
        Self::try_new_with_protection(id, dimension, minimum, maximum, None)
    }

    pub fn try_new_with_protection(
        id: impl AsRef<str>,
        dimension: impl AsRef<str>,
        minimum: ScriptPosition,
        maximum: ScriptPosition,
        protection: Option<ScriptZoneProtection>,
    ) -> Result<Self, ScriptDtoError> {
        if minimum.x() > maximum.x() || minimum.y() > maximum.y() || minimum.z() > maximum.z() {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            id: validate_script_id(id.as_ref())?,
            dimension: validate_contract_resource_id(dimension.as_ref())?,
            minimum,
            maximum,
            protection,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    pub fn minimum(&self) -> ScriptPosition {
        self.minimum
    }

    pub fn maximum(&self) -> ScriptPosition {
        self.maximum
    }

    pub fn protection(&self) -> Option<&ScriptZoneProtection> {
        self.protection.as_ref()
    }
}

/// One immutable item descriptor used in a server-owned inventory menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryMenuItem {
    resource_id: String,
    count: u8,
    label: Option<String>,
}

impl ScriptInventoryMenuItem {
    pub fn try_new(
        resource_id: impl AsRef<str>,
        count: u8,
        label: Option<String>,
    ) -> Result<Self, ScriptDtoError> {
        if count == 0 {
            return Err(ScriptDtoError::InvalidAmount);
        }
        if let Some(label) = &label
            && label.len() > MAX_INVENTORY_MENU_TITLE_BYTES
        {
            return Err(ScriptDtoError::ValueTooLong {
                field: "inventory menu item label",
                max_bytes: MAX_INVENTORY_MENU_TITLE_BYTES,
                actual_bytes: label.len(),
            });
        }
        Ok(Self {
            resource_id: validate_contract_resource_id(resource_id.as_ref())?,
            count,
            label,
        })
    }

    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    pub const fn count(&self) -> u8 {
        self.count
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

/// One fixed slot in a server-owned inventory menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryMenuSlot {
    index: u8,
    item: ScriptInventoryMenuItem,
}

impl ScriptInventoryMenuSlot {
    pub const fn new(index: u8, item: ScriptInventoryMenuItem) -> Self {
        Self { index, item }
    }

    pub const fn index(&self) -> u8 {
        self.index
    }

    pub fn item(&self) -> &ScriptInventoryMenuItem {
        &self.item
    }
}

/// Immutable menu description requested by a plugin and owned by the server after admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryMenu {
    id: String,
    title: String,
    slots: Vec<ScriptInventoryMenuSlot>,
}

impl ScriptInventoryMenu {
    pub fn try_new(
        id: impl AsRef<str>,
        title: impl AsRef<str>,
        slots: Vec<ScriptInventoryMenuSlot>,
    ) -> Result<Self, ScriptDtoError> {
        let title = title.as_ref();
        validate_bounded_nonempty(
            "inventory menu title",
            title,
            MAX_INVENTORY_MENU_TITLE_BYTES,
        )?;
        if slots.len() > MAX_INVENTORY_MENU_SLOTS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "inventory menu slots",
                max: MAX_INVENTORY_MENU_SLOTS,
            });
        }
        let mut indexes = BTreeMap::new();
        for slot in &slots {
            if usize::from(slot.index) >= MAX_INVENTORY_MENU_SLOTS {
                return Err(ScriptDtoError::InvalidBounds);
            }
            if indexes.insert(slot.index, ()).is_some() {
                return Err(ScriptDtoError::DuplicateId {
                    field: "inventory menu slot index",
                    actual_bytes: decimal_u8_len(slot.index),
                });
            }
        }
        Ok(Self {
            id: validate_script_id(id.as_ref())?,
            title: title.to_owned(),
            slots,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn slots(&self) -> &[ScriptInventoryMenuSlot] {
        &self.slots
    }
}

/// One logical player-inventory resource delta in an atomic transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryResourceDelta {
    resource_id: String,
    delta: i16,
}

impl ScriptInventoryResourceDelta {
    pub fn try_new(resource_id: impl AsRef<str>, delta: i16) -> Result<Self, ScriptDtoError> {
        if delta == 0 || delta.unsigned_abs() > MAX_INVENTORY_RESOURCE_DELTA as u16 {
            return Err(ScriptDtoError::InvalidAmount);
        }
        Ok(Self {
            resource_id: validate_contract_resource_id(resource_id.as_ref())?,
            delta,
        })
    }

    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    pub const fn delta(&self) -> i16 {
        self.delta
    }
}

/// One plugin-storage mutation in an atomic inventory and storage transaction.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptStorageMutation {
    #[serde(rename = "cas")]
    CompareAndSwap {
        key: String,
        expected_version: Option<u64>,
        value: String,
    },
    #[serde(rename = "delete")]
    Delete {
        key: String,
        expected_version: Option<u64>,
    },
}

impl ScriptStorageMutation {
    pub fn compare_and_swap(
        key: impl AsRef<str>,
        expected_version: Option<u64>,
        value: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        let value = value.as_ref();
        validate_plugin_storage_value(value)?;
        Ok(Self::CompareAndSwap {
            key: validate_plugin_storage_key(key.as_ref())?,
            expected_version,
            value: value.to_owned(),
        })
    }

    pub fn delete(
        key: impl AsRef<str>,
        expected_version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self::Delete {
            key: validate_plugin_storage_key(key.as_ref())?,
            expected_version,
        })
    }

    pub fn key(&self) -> &str {
        match self {
            Self::CompareAndSwap { key, .. } | Self::Delete { key, .. } => key,
        }
    }

    fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::CompareAndSwap { key, value, .. } => {
                validate_plugin_storage_key(key)?;
                validate_plugin_storage_value(value)
            }
            Self::Delete { key, .. } => validate_plugin_storage_key(key).map(drop),
        }
    }
}

/// Validated plugin-storage read request used to correlate its targeted result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPluginStorageGetRequest {
    request_id: String,
    key: String,
}

impl ScriptPluginStorageGetRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        key: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            key: validate_plugin_storage_key(key.as_ref())?,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

/// Validated plugin-storage compare-and-swap request and result correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPluginStorageCompareAndSwapRequest {
    request_id: String,
    key: String,
    expected_version: Option<u64>,
    value: String,
}

impl ScriptPluginStorageCompareAndSwapRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        key: impl AsRef<str>,
        expected_version: Option<u64>,
        value: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        let value = value.as_ref();
        validate_plugin_storage_value(value)?;
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            key: validate_plugin_storage_key(key.as_ref())?,
            expected_version,
            value: value.to_owned(),
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub const fn expected_version(&self) -> Option<u64> {
        self.expected_version
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Validated plugin-storage delete request used to correlate its targeted result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPluginStorageDeleteRequest {
    request_id: String,
    key: String,
    expected_version: Option<u64>,
}

/// Explicit terminal reason for a plugin-storage request that could not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptPluginStorageFailure {
    /// This server has no persistent world-backed plugin storage.
    Unavailable,
    /// The storage actor stopped after a durable write or synchronization failure.
    DurabilityFailed,
}

impl ScriptPluginStorageFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::DurabilityFailed => "durability_failed",
        }
    }
}

impl ScriptPluginStorageDeleteRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        key: impl AsRef<str>,
        expected_version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            key: validate_plugin_storage_key(key.as_ref())?,
            expected_version,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub const fn expected_version(&self) -> Option<u64> {
        self.expected_version
    }
}

/// A bounded request the server must commit or reject as one inventory and storage mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryStorageTransaction {
    id: String,
    player_id: ScriptPlayerId,
    inventory: Vec<ScriptInventoryResourceDelta>,
    storage: Vec<ScriptStorageMutation>,
}

impl ScriptInventoryStorageTransaction {
    pub fn try_new(
        id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        inventory: Vec<ScriptInventoryResourceDelta>,
        storage: Vec<ScriptStorageMutation>,
    ) -> Result<Self, ScriptDtoError> {
        if inventory.is_empty() || storage.is_empty() {
            return Err(ScriptDtoError::EmptyTransaction);
        }
        if inventory.len() > MAX_INVENTORY_STORAGE_MUTATIONS
            || storage.len() > MAX_INVENTORY_STORAGE_MUTATIONS
        {
            return Err(ScriptDtoError::TooManyEntries {
                field: "inventory storage transaction",
                max: MAX_INVENTORY_STORAGE_MUTATIONS,
            });
        }
        let mut inventory_ids = BTreeMap::new();
        for delta in &inventory {
            if inventory_ids.insert(delta.resource_id(), ()).is_some() {
                return Err(ScriptDtoError::DuplicateId {
                    field: "inventory resource id",
                    actual_bytes: delta.resource_id().len(),
                });
            }
        }
        let mut storage_keys = BTreeMap::new();
        for mutation in &storage {
            mutation.validate()?;
            if storage_keys.insert(mutation.key(), ()).is_some() {
                return Err(ScriptDtoError::DuplicateId {
                    field: "plugin storage key",
                    actual_bytes: mutation.key().len(),
                });
            }
        }
        Ok(Self {
            id: validate_script_id(id.as_ref())?,
            player_id,
            inventory,
            storage,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    pub fn inventory(&self) -> &[ScriptInventoryResourceDelta] {
        &self.inventory
    }

    pub fn storage(&self) -> &[ScriptStorageMutation] {
        &self.storage
    }
}

/// Bounded atomic mutation of one connected player's main inventory and hotbar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlayerInventoryTransaction {
    request_id: String,
    player_id: ScriptPlayerId,
    deltas: Vec<ScriptInventoryResourceDelta>,
}

impl ScriptPlayerInventoryTransaction {
    pub fn try_new(
        request_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        deltas: Vec<ScriptInventoryResourceDelta>,
    ) -> Result<Self, ScriptDtoError> {
        if deltas.is_empty() {
            return Err(ScriptDtoError::EmptyTransaction);
        }
        if deltas.len() > MAX_INVENTORY_STORAGE_MUTATIONS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "player inventory transaction",
                max: MAX_INVENTORY_STORAGE_MUTATIONS,
            });
        }
        let mut resource_ids = BTreeMap::new();
        for delta in &deltas {
            if resource_ids.insert(delta.resource_id(), ()).is_some() {
                return Err(ScriptDtoError::DuplicateId {
                    field: "inventory resource id",
                    actual_bytes: delta.resource_id().len(),
                });
            }
        }
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            player_id,
            deltas,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    pub fn deltas(&self) -> &[ScriptInventoryResourceDelta] {
        &self.deltas
    }
}

/// Exact reason why an admitted player inventory transaction did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptPlayerInventoryFailure {
    PlayerUnavailable,
    RuntimeUnavailable,
    UnknownResource,
    InsufficientResource,
    InventoryFull,
}

impl ScriptPlayerInventoryFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlayerUnavailable => "player_unavailable",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::UnknownResource => "unknown_resource",
            Self::InsufficientResource => "insufficient_resource",
            Self::InventoryFull => "inventory_full",
        }
    }
}

/// Server-normalized inventory click kind. Plugins never receive slot stacks or packet state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptInventoryClick {
    Primary,
    Secondary,
    ShiftPrimary,
    ShiftSecondary,
}

/// Closed crafting source snapshot exposed by item-crafted events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptCraftingSource {
    Inventory,
    CraftingTable,
}

impl ScriptCraftingSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inventory => "inventory",
            Self::CraftingTable => "crafting_table",
        }
    }
}

/// Closed pickup source snapshot exposed by item-picked-up events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptItemPickupSource {
    ItemEntity,
    Arrow,
}

impl ScriptItemPickupSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ItemEntity => "item_entity",
            Self::Arrow => "arrow",
        }
    }
}

/// Closed source snapshot exposed by player entity-kill events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptEntityKillSource {
    Melee,
}

impl ScriptEntityKillSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Melee => "melee",
        }
    }
}

/// Closed hand snapshot exposed by player entity-interaction events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptInteractionHand {
    MainHand,
    OffHand,
}

impl ScriptInteractionHand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MainHand => "main_hand",
            Self::OffHand => "off_hand",
        }
    }
}

/// Closed game-mode snapshot exposed by gameplay events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptGameMode {
    Survival,
    Creative,
    Adventure,
}

impl ScriptGameMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
        }
    }
}

/// Protocol phase that produced a client-originated custom payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptProtocolPhase {
    Configuration,
    Play,
}

impl ScriptProtocolPhase {
    /// Return the stable component-visible phase name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Play => "play",
        }
    }
}

/// Immutable inbound event snapshots visible to script runtimes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEvent {
    target_plugin_id: Option<String>,
    /// Registration that issued an asynchronous request, if this is its result.
    /// The response must not be reattributed to a replacement with the same id.
    origin_generation: Option<u64>,
    kind: ScriptEventKind,
}

impl ScriptEvent {
    /// Keep the registration that issued a request even when its owner replies
    /// after a deployment reload. Unsolicited events use admission-time routing.
    fn with_origin_generation(mut self, generation: Option<u64>) -> Self {
        self.origin_generation = generation;
        self
    }

    /// Build a server-started event snapshot.
    pub fn server_started() -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::ServerStarted,
            origin_generation: None,
        }
    }

    /// Build a server-stopping event snapshot.
    pub fn server_stopping(reason: impl AsRef<str>) -> Self {
        let reason = reason.as_ref();
        validate_bounded_nonempty(
            "server stopping reason",
            reason,
            MAX_SCRIPT_DISCONNECT_REASON_BYTES,
        )
        .expect("server-authored stopping reason must be bounded");
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::ServerStopping {
                reason: reason.to_owned(),
            },
            origin_generation: None,
        }
    }

    /// Build a player-joined event snapshot with server-authoritative context.
    pub fn player_joined_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
    ) -> Self {
        context
            .validate()
            .expect("server-authored player context must be bounded");
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerJoined {
                player_id,
                username: context.username().to_owned(),
                context,
            },
            origin_generation: None,
        }
    }

    /// Build a player-left event snapshot.
    pub fn player_left(player_id: ScriptPlayerId, reason: impl AsRef<str>) -> Self {
        let reason = reason.as_ref();
        validate_bounded_nonempty(
            "player-left reason",
            reason,
            MAX_SCRIPT_DISCONNECT_REASON_BYTES,
        )
        .expect("server-authored player-left reason must be bounded");
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerLeft {
                player_id,
                reason: reason.to_owned(),
            },
            origin_generation: None,
        }
    }

    /// Build a player-chat event snapshot with server-authoritative context.
    pub fn player_chat_with_context(
        player_id: ScriptPlayerId,
        message: impl AsRef<str>,
        context: ScriptPlayerContext,
    ) -> Self {
        let message = message.as_ref();
        validate_bounded_nonempty(
            "player chat message",
            message,
            MAX_SCRIPT_CHAT_MESSAGE_BYTES,
        )
        .expect("server-authored player chat must be bounded");
        context
            .validate()
            .expect("server-authored player context must be bounded");
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerChat {
                player_id,
                message: message.to_owned(),
                context,
            },
            origin_generation: None,
        }
    }

    /// Build a reliable block-break event after the authoritative world commit.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_block_broken_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        block_id: impl AsRef<str>,
        x: i32,
        y: i32,
        z: i32,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerBlockBroken {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                block_id: validate_contract_resource_id(block_id.as_ref())?,
                x,
                y,
                z,
                game_mode,
            },
            origin_generation: None,
        })
    }

    /// Build a reliable block-place event after the authoritative world commit.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_block_placed_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        block_id: impl AsRef<str>,
        x: i32,
        y: i32,
        z: i32,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerBlockPlaced {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                block_id: validate_contract_resource_id(block_id.as_ref())?,
                x,
                y,
                z,
                game_mode,
            },
            origin_generation: None,
        })
    }

    /// Build a reliable item-crafted event after the authoritative inventory commit.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_item_crafted_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        item_id: impl AsRef<str>,
        count: u64,
        craft_count: u32,
        source: ScriptCraftingSource,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        if count == 0 || craft_count == 0 {
            return Err(ScriptDtoError::InvalidAmount);
        }
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerItemCrafted {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                item_id: validate_contract_resource_id(item_id.as_ref())?,
                count,
                craft_count,
                source,
                game_mode,
            },
            origin_generation: None,
        })
    }

    /// Build a reliable item-pickup event after the authoritative inventory commit.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_item_picked_up_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        item_id: impl AsRef<str>,
        count: u64,
        source: ScriptItemPickupSource,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        if count == 0 {
            return Err(ScriptDtoError::InvalidAmount);
        }
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerItemPickedUp {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                item_id: validate_contract_resource_id(item_id.as_ref())?,
                count,
                source,
                game_mode,
            },
            origin_generation: None,
        })
    }

    /// Build a reliable direct player-melee kill event after the entity commit.
    pub fn try_player_entity_killed_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        entity_id: ScriptEntityId,
        entity_type: impl AsRef<str>,
        source: ScriptEntityKillSource,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerEntityKilled {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                entity_id,
                entity_type: validate_contract_resource_id(entity_type.as_ref())?,
                source,
                game_mode,
            },
            origin_generation: None,
        })
    }

    /// Build a broadcast event after an authoritative entity interaction is accepted.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_entity_interacted_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        entity_id: ScriptEntityId,
        entity_type: impl AsRef<str>,
        hand: ScriptInteractionHand,
        secondary_action: bool,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerEntityInteracted {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                entity_id,
                entity_type: validate_contract_resource_id(entity_type.as_ref())?,
                hand,
                secondary_action,
                game_mode,
            },
            origin_generation: None,
        })
    }

    /// Build a reliable player-death event after the authoritative survival commit.
    pub fn try_player_died_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerDied {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                game_mode,
            },
            origin_generation: None,
        })
    }

    /// Build the targeted player-grave observation after the death commit.
    ///
    /// The event is addressed to the one package that declared the
    /// `death_graves` capability, so a deployment without one publishes nothing
    /// of this kind.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_grave_created_with_context(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        x: i32,
        y: i32,
        z: i32,
        items: u32,
        spilled: u32,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        let event = Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PlayerGraveCreated {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                x,
                y,
                z,
                items,
                spilled,
            },
            origin_generation: None,
        };
        event.validate()?;
        Ok(event)
    }

    /// Build a bounded player command event with server-authoritative context.
    pub fn try_player_command_with_context(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        root: impl AsRef<str>,
        arguments: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        let target_plugin_id = validate_target_plugin_id(target_plugin_id.as_ref())?;
        let root = root.as_ref();
        let arguments = arguments.as_ref();
        validate_bounded_nonempty("player command root", root, MAX_PLAYER_COMMAND_ROOT_BYTES)?;
        validate_bounded_value(
            "player command arguments",
            arguments,
            MAX_SCRIPT_CHAT_MESSAGE_BYTES,
        )?;
        context.validate()?;
        Ok(Self {
            target_plugin_id: Some(target_plugin_id),
            kind: ScriptEventKind::PlayerCommand {
                player_id,
                username: context.username().to_owned(),
                root: root.to_owned(),
                arguments: arguments.to_owned(),
                context,
            },
            origin_generation: None,
        })
    }

    /// Build a server-tick event snapshot.
    pub fn server_tick(tick: u64) -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::ServerTick { tick },
            origin_generation: None,
        }
    }

    /// Build a targeted plugin-storage read result.
    pub fn plugin_storage_get_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptPluginStorageGetRequest,
        value: Option<String>,
        version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        if value.is_some() != version.is_some() {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage get value/version",
            });
        }
        if let Some(value) = &value {
            validate_plugin_storage_value(value)?;
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PluginStorageGetResult {
                request_id: request.request_id().to_owned(),
                key: request.key().to_owned(),
                value,
                version,
                failure: None,
            },
            origin_generation: None,
        })
    }

    /// Build a targeted plugin-storage compare-and-swap result.
    pub fn plugin_storage_cas_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptPluginStorageCompareAndSwapRequest,
        applied: bool,
        version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        if applied && version.is_none() {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage compare-and-swap success/version",
            });
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PluginStorageCasResult {
                request_id: request.request_id().to_owned(),
                key: request.key().to_owned(),
                applied,
                version,
                failure: None,
            },
            origin_generation: None,
        })
    }

    /// Build a targeted plugin-storage delete result.
    pub fn plugin_storage_delete_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptPluginStorageDeleteRequest,
        deleted: bool,
        version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        if deleted && version.is_none() {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage delete success/version",
            });
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PluginStorageDeleteResult {
                request_id: request.request_id().to_owned(),
                key: request.key().to_owned(),
                deleted,
                version,
                failure: None,
            },
            origin_generation: None,
        })
    }

    /// Build a targeted click event for one server-owned inventory menu.
    pub(crate) fn inventory_menu_clicked(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        menu: &ScriptInventoryMenu,
        slot: u8,
        click: ScriptInventoryClick,
    ) -> Result<Self, ScriptDtoError> {
        if usize::from(slot) >= MAX_INVENTORY_MENU_SLOTS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::InventoryMenuClicked {
                player_id,
                context,
                menu_id: menu.id().to_owned(),
                slot,
                click,
            },
            origin_generation: None,
        })
    }

    /// Build a targeted completion event for an atomic inventory and storage request.
    pub(crate) fn inventory_storage_transaction_result(
        target_plugin_id: impl AsRef<str>,
        transaction: &ScriptInventoryStorageTransaction,
        committed: bool,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::InventoryStorageTransactionResult {
                request_id: transaction.id().to_owned(),
                committed,
            },
            origin_generation: None,
        })
    }

    pub fn operation_result(
        target_plugin_id: impl AsRef<str>,
        request_id: &str,
        operation_id: Option<&str>,
        outcome: ScriptOperationOutcome,
    ) -> Result<Self, ScriptDtoError> {
        let event = Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::OperationResult {
                request_id: validate_script_id(request_id)?,
                operation_id: operation_id.map(validate_script_id).transpose()?,
                outcome,
            },
            origin_generation: None,
        };
        event.validate()?;
        Ok(event)
    }

    /// Build the targeted result of one admitted player inventory transaction.
    pub(crate) fn player_inventory_transaction_result(
        target_plugin_id: impl AsRef<str>,
        transaction: &ScriptPlayerInventoryTransaction,
        failure: Option<ScriptPlayerInventoryFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PlayerInventoryTransactionResult {
                request_id: transaction.request_id().to_owned(),
                player_id: transaction.player_id(),
                failure,
            },
            origin_generation: None,
        })
    }

    /// Build a targeted zone-entry snapshot owned by the plugin that registered the zone.
    pub(crate) fn player_zone_entered(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone: &ScriptAxisAlignedZone,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PlayerZoneEntered {
                player_id,
                context,
                zone_id: zone.id().to_owned(),
            },
            origin_generation: None,
        })
    }

    /// Build a targeted zone-exit snapshot owned by the plugin that registered the zone.
    pub(crate) fn player_zone_exited(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone: &ScriptAxisAlignedZone,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PlayerZoneExited {
                player_id,
                context,
                zone_id: zone.id().to_owned(),
            },
            origin_generation: None,
        })
    }

    fn zone_command_result(
        target_plugin_id: impl AsRef<str>,
        zone_id: impl AsRef<str>,
        accepted: bool,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::ZoneCommandResult {
                zone_id: validate_script_id(zone_id.as_ref())?,
                accepted,
            },
            origin_generation: None,
        })
    }

    /// Build the targeted result of one admitted same-dimension player teleport.
    pub(crate) fn player_teleport_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptPlayerTeleportRequest,
        failure: Option<ScriptPlayerTeleportFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PlayerTeleportResult {
                request_id: request.request_id().to_owned(),
                player_id: request.player_id(),
                position: request.position(),
                failure,
            },
            origin_generation: None,
        })
    }

    /// Build the targeted result of one admitted authoritative world-time mutation.
    pub(crate) fn world_time_set_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptWorldTimeSetRequest,
        failure: Option<ScriptWorldTimeSetFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::WorldTimeSetResult {
                request_id: request.request_id().to_owned(),
                world_time: request.world_time(),
                failure,
            },
            origin_generation: None,
        })
    }

    /// Build the targeted result of one admitted authoritative default-block mutation.
    pub(crate) fn world_block_set_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptWorldBlockSetRequest,
        applied: bool,
        failure: Option<ScriptWorldBlockSetFailure>,
    ) -> Result<Self, ScriptDtoError> {
        if applied != failure.is_none() {
            return Err(ScriptDtoError::InconsistentResult {
                field: "world block set result",
            });
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::WorldBlockSetResult {
                request_id: request.request_id().to_owned(),
                dimension: request.dimension().to_owned(),
                block_id: request.block_id().to_owned(),
                x: request.x(),
                y: request.y(),
                z: request.z(),
                applied,
                failure,
            },
            origin_generation: None,
        })
    }

    /// Build the targeted result of one admitted Loader custom-block item grant.
    pub(crate) fn loader_item_grant_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptLoaderItemGrantRequest,
        failure: Option<ScriptLoaderItemGrantFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::LoaderItemGrantResult {
                request_id: request.request_id().to_owned(),
                player_id: request.player_id(),
                block_id: request.block_id().to_owned(),
                count: request.count(),
                failure,
            },
            origin_generation: None,
        })
    }

    /// Build the targeted result of one admitted entity-spawn mutation.
    pub(crate) fn entity_spawn_result(
        target_plugin_id: impl AsRef<str>,
        request_id: impl AsRef<str>,
        actor: ScriptPlayerId,
        entity_type: impl AsRef<str>,
        position: ScriptPosition,
        failure: Option<ScriptEntitySpawnFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::EntitySpawnResult {
                request_id: validate_script_id(request_id.as_ref())?,
                player_id: actor,
                entity_type: validate_contract_resource_id(entity_type.as_ref())?,
                position,
                failure,
            },
            origin_generation: None,
        })
    }

    /// Build the targeted result of one admitted entity-damage mutation.
    pub(crate) fn entity_damage_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptEntityDamageRequest,
        health: Option<f32>,
        killed: bool,
        failure: Option<ScriptEntityDamageFailure>,
    ) -> Result<Self, ScriptDtoError> {
        if health.is_some() != failure.is_none()
            || killed && health.is_none()
            || health.is_some_and(|health| !health.is_finite() || health < 0.0)
            || killed && health.is_some_and(|health| health > 0.0)
        {
            return Err(ScriptDtoError::InconsistentResult {
                field: "entity damage result",
            });
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::EntityDamageResult {
                request_id: request.request_id().to_owned(),
                entity_id: request.entity_id(),
                amount_bits: request.amount().to_bits(),
                health_bits: health.map(f32::to_bits),
                killed,
                failure,
            },
            origin_generation: None,
        })
    }

    /// Build the targeted result of one admitted connected-player query.
    pub(crate) fn online_players_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptOnlinePlayersRequest,
        players: Vec<ScriptOnlinePlayerSnapshot>,
        truncated: bool,
    ) -> Result<Self, ScriptDtoError> {
        if players.len() > request.limit() {
            return Err(ScriptDtoError::TooManyEntries {
                field: "online player snapshots",
                max: request.limit(),
            });
        }
        for player in &players {
            player.validate()?;
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::OnlinePlayersResult {
                request_id: request.request_id().to_owned(),
                players,
                truncated,
            },
            origin_generation: None,
        })
    }

    /// Build one explicit key-driven view request event targeted to the owner.
    pub fn loader_view_request(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        request_kind: ScriptClientViewKey,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::LoaderViewRequest {
                player_id,
                request_kind,
            },
            origin_generation: None,
        })
    }

    /// Build one admitted client view action snapshot targeted to the owner.
    #[allow(clippy::too_many_arguments)]
    pub fn loader_view_action(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        view_instance_id: impl AsRef<str>,
        view_revision: u64,
        action_id: impl AsRef<str>,
        action_sequence: u64,
        fields: Vec<ScriptClientViewField>,
        selection_token: Option<String>,
    ) -> Result<Self, ScriptDtoError> {
        let view_instance_id = validate_contract_resource_id(view_instance_id.as_ref())?;
        let action_id = validate_contract_resource_id(action_id.as_ref())?;
        if view_revision > MAX_CLIENT_VIEW_EXACT_INTEGER
            || action_sequence > MAX_CLIENT_VIEW_EXACT_INTEGER
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if fields.len() > MAX_CLIENT_VIEW_FIELDS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "view action fields",
                max: MAX_CLIENT_VIEW_FIELDS,
            });
        }
        if let Some(token) = selection_token.as_deref() {
            validate_bounded_nonempty("view selection token", token, MAX_CLIENT_VIEW_ID_BYTES)?;
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::LoaderViewAction {
                player_id,
                view_instance_id,
                view_revision,
                action_id,
                action_sequence,
                fields,
                selection_token,
            },
            origin_generation: None,
        })
    }

    /// Build the outcome of one `open_client_view` request for the caller.
    pub fn client_view_opened(
        target_plugin_id: impl AsRef<str>,
        request_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        view_instance_id: Option<String>,
        revision: Option<u64>,
        failure: Option<ScriptClientViewFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::ClientViewOpened {
                request_id: validate_script_id(request_id.as_ref())?,
                player_id,
                view_instance_id,
                revision,
                failure,
            },
            origin_generation: None,
        })
    }

    /// Build the outcome of one `begin_client_selection` request for the caller.
    pub fn client_selection_started(
        target_plugin_id: impl AsRef<str>,
        request_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        selection_context_id: Option<String>,
        expires_at_tick: Option<u64>,
        failure: Option<ScriptClientViewFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::ClientSelectionStarted {
                request_id: validate_script_id(request_id.as_ref())?,
                player_id,
                selection_context_id,
                expires_at_tick,
                failure,
            },
            origin_generation: None,
        })
    }

    /// Build a client-brand event snapshot. Empty brands are preserved.
    pub fn client_brand(
        player_id: ScriptPlayerId,
        brand: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        let brand = brand.as_ref();
        validate_bounded_value("client brand", brand, MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES)?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::ClientBrand {
                player_id,
                brand: brand.to_owned(),
            },
            origin_generation: None,
        })
    }

    /// Build a custom-payload event snapshot targeted to the channel owner.
    /// The owned payload moves into the event without copying.
    pub fn custom_payload(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        phase: ScriptProtocolPhase,
        channel: impl AsRef<str>,
        payload: Vec<u8>,
    ) -> Result<Self, ScriptDtoError> {
        let target_plugin_id = validate_target_plugin_id(target_plugin_id.as_ref())?;
        let channel = validate_contract_resource_id(channel.as_ref())?;
        if payload.len() > MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES {
            return Err(ScriptDtoError::ValueTooLong {
                field: "custom payload",
                max_bytes: MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES,
                actual_bytes: payload.len(),
            });
        }
        Ok(Self {
            target_plugin_id: Some(target_plugin_id),
            kind: ScriptEventKind::CustomPayload {
                player_id,
                phase,
                channel,
                payload,
            },
            origin_generation: None,
        })
    }

    /// Return the plugin id for an event that must not be broadcast to other runtimes.
    pub fn target_plugin_id(&self) -> Option<&str> {
        self.target_plugin_id.as_deref()
    }

    /// Return the immutable event kind.
    pub fn kind(&self) -> &ScriptEventKind {
        &self.kind
    }

    /// Return the stable manifest subscription name for this event.
    pub fn event_name(&self) -> &'static str {
        match self.kind {
            ScriptEventKind::ServerStarted => "server.started",
            ScriptEventKind::ServerStopping { .. } => "server.stopping",
            ScriptEventKind::PlayerJoined { .. } => "player.joined",
            ScriptEventKind::PlayerLeft { .. } => "player.left",
            ScriptEventKind::PlayerChat { .. } => "player.chat",
            ScriptEventKind::PlayerBlockBroken { .. } => "player.block_broken",
            ScriptEventKind::PlayerBlockPlaced { .. } => "player.block_placed",
            ScriptEventKind::PlayerItemCrafted { .. } => "player.item_crafted",
            ScriptEventKind::PlayerItemPickedUp { .. } => "player.item_picked_up",
            ScriptEventKind::PlayerEntityKilled { .. } => "player.entity_killed",
            ScriptEventKind::PlayerEntityInteracted { .. } => "player.entity_interacted",
            ScriptEventKind::PlayerDied { .. } => "player.died",
            ScriptEventKind::PlayerGraveCreated { .. } => "player.grave_created",
            ScriptEventKind::PlayerCommand { .. } => "player.command",
            ScriptEventKind::ServerTick { .. } => "server.tick",
            ScriptEventKind::PluginStorageGetResult { .. } => "plugin.storage.get_result",
            ScriptEventKind::PluginStorageCasResult { .. } => "plugin.storage.cas_result",
            ScriptEventKind::PluginStorageDeleteResult { .. } => "plugin.storage.delete_result",
            ScriptEventKind::InventoryMenuClicked { .. } => "inventory.menu.clicked",
            ScriptEventKind::InventoryStorageTransactionResult { .. } => {
                "inventory.storage_transaction.result"
            }
            ScriptEventKind::OperationResult { .. } => "operation.result",
            ScriptEventKind::PlayerInventoryTransactionResult { .. } => {
                "player.inventory_transaction_result"
            }
            ScriptEventKind::PlayerZoneEntered { .. } => "player.zone_entered",
            ScriptEventKind::PlayerZoneExited { .. } => "player.zone_exited",
            ScriptEventKind::ZoneCommandResult { .. } => "zone.command_result",
            ScriptEventKind::PlayerTeleportResult { .. } => "player.teleport_result",
            ScriptEventKind::WorldTimeSetResult { .. } => "world.time_set_result",
            ScriptEventKind::WorldBlockSetResult { .. } => "world.block_set_result",
            ScriptEventKind::LoaderItemGrantResult { .. } => "loader.item_grant_result",
            ScriptEventKind::EntitySpawnResult { .. } => "entity.spawn_result",
            ScriptEventKind::EntityDamageResult { .. } => "entity.damage_result",
            ScriptEventKind::OnlinePlayersResult { .. } => "player.online_result",
            ScriptEventKind::LoaderViewRequest { .. } => "loader.view_request",
            ScriptEventKind::LoaderViewAction { .. } => "loader.view_action",
            ScriptEventKind::ClientViewOpened { .. } => "loader.view_opened",
            ScriptEventKind::ClientSelectionStarted { .. } => "loader.selection_started",
            ScriptEventKind::ClientBrand { .. } => "player.client_brand",
            ScriptEventKind::CustomPayload { .. } => "player.custom_payload",
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(plugin_id) = &self.target_plugin_id {
            validate_target_plugin_id(plugin_id)?;
        }
        match &self.kind {
            ScriptEventKind::ServerStarted | ScriptEventKind::ServerTick { .. } => Ok(()),
            ScriptEventKind::ServerStopping { reason }
            | ScriptEventKind::PlayerLeft { reason, .. } => validate_bounded_nonempty(
                "event reason",
                reason,
                MAX_SCRIPT_DISCONNECT_REASON_BYTES,
            ),
            ScriptEventKind::PlayerJoined {
                username, context, ..
            } => {
                validate_bounded_nonempty(
                    "event username",
                    username,
                    MAX_SCRIPT_PLAYER_NAME_BYTES,
                )?;
                context.validate()
            }
            ScriptEventKind::PlayerChat {
                message, context, ..
            } => {
                validate_bounded_nonempty(
                    "event chat message",
                    message,
                    MAX_SCRIPT_CHAT_MESSAGE_BYTES,
                )?;
                context.validate()
            }
            ScriptEventKind::PlayerBlockBroken {
                context,
                dimension,
                block_id,
                ..
            }
            | ScriptEventKind::PlayerBlockPlaced {
                context,
                dimension,
                block_id,
                ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                validate_contract_resource_id(block_id).map(drop)
            }
            ScriptEventKind::PlayerItemCrafted {
                context,
                dimension,
                item_id,
                count,
                craft_count,
                ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                validate_contract_resource_id(item_id)?;
                if *count == 0 || *craft_count == 0 {
                    return Err(ScriptDtoError::InvalidAmount);
                }
                Ok(())
            }
            ScriptEventKind::PlayerItemPickedUp {
                context,
                dimension,
                item_id,
                count,
                ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                validate_contract_resource_id(item_id)?;
                if *count == 0 {
                    return Err(ScriptDtoError::InvalidAmount);
                }
                Ok(())
            }
            ScriptEventKind::PlayerEntityKilled {
                context,
                dimension,
                entity_type,
                ..
            }
            | ScriptEventKind::PlayerEntityInteracted {
                context,
                dimension,
                entity_type,
                ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                validate_contract_resource_id(entity_type).map(drop)
            }
            ScriptEventKind::PlayerDied {
                context, dimension, ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                Ok(())
            }
            ScriptEventKind::PlayerGraveCreated {
                context,
                dimension,
                x,
                y,
                z,
                items,
                spilled,
                ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                if f64::from(*x).abs() > SCRIPT_HORIZONTAL_COORDINATE_LIMIT
                    || f64::from(*y).abs() > SCRIPT_VERTICAL_COORDINATE_LIMIT
                    || f64::from(*z).abs() > SCRIPT_HORIZONTAL_COORDINATE_LIMIT
                    || *items > MAX_OWNED_INVENTORY_SLOTS as u32
                    || *spilled > MAX_OWNED_INVENTORY_SLOTS as u32
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            ScriptEventKind::PlayerCommand {
                username,
                root,
                arguments,
                context,
                ..
            } => {
                validate_bounded_nonempty(
                    "event username",
                    username,
                    MAX_SCRIPT_PLAYER_NAME_BYTES,
                )?;
                validate_bounded_nonempty(
                    "event command root",
                    root,
                    MAX_PLAYER_COMMAND_ROOT_BYTES,
                )?;
                validate_bounded_value(
                    "event command arguments",
                    arguments,
                    MAX_SCRIPT_CHAT_MESSAGE_BYTES,
                )?;
                context.validate()
            }
            ScriptEventKind::PluginStorageGetResult {
                request_id,
                key,
                value,
                version,
                failure,
            } => {
                validate_script_id(request_id)?;
                validate_plugin_storage_key(key)?;
                if value.is_some() != version.is_some() {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "plugin storage get value/version",
                    });
                }
                if let Some(value) = value {
                    validate_plugin_storage_value(value)?;
                }
                if failure.is_some() && (value.is_some() || version.is_some()) {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "plugin storage get failure/result",
                    });
                }
                Ok(())
            }
            ScriptEventKind::PluginStorageCasResult {
                request_id,
                key,
                applied,
                version,
                failure,
            }
            | ScriptEventKind::PluginStorageDeleteResult {
                request_id,
                key,
                deleted: applied,
                version,
                failure,
            } => {
                validate_script_id(request_id)?;
                validate_plugin_storage_key(key)?;
                if *applied && version.is_none() {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "plugin storage mutation success/version",
                    });
                }
                if failure.is_some() && (*applied || version.is_some()) {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "plugin storage mutation failure/result",
                    });
                }
                Ok(())
            }
            ScriptEventKind::InventoryMenuClicked {
                context,
                menu_id,
                slot,
                ..
            } => {
                context.validate()?;
                validate_script_id(menu_id)?;
                if usize::from(*slot) >= MAX_INVENTORY_MENU_SLOTS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            ScriptEventKind::InventoryStorageTransactionResult { request_id, .. } => {
                validate_script_id(request_id).map(drop)
            }
            ScriptEventKind::OperationResult {
                request_id,
                operation_id,
                outcome,
            } => {
                validate_script_id_value(request_id)?;
                if let Some(operation_id) = operation_id {
                    validate_script_id_value(operation_id)?;
                }
                outcome.validate()
            }
            ScriptEventKind::PlayerZoneEntered {
                context, zone_id, ..
            }
            | ScriptEventKind::PlayerZoneExited {
                context, zone_id, ..
            } => {
                context.validate()?;
                validate_script_id(zone_id).map(drop)
            }
            ScriptEventKind::ZoneCommandResult { zone_id, .. } => {
                validate_script_id(zone_id).map(drop)
            }
            ScriptEventKind::PlayerInventoryTransactionResult { request_id, .. }
            | ScriptEventKind::PlayerTeleportResult { request_id, .. }
            | ScriptEventKind::WorldTimeSetResult { request_id, .. } => {
                validate_script_id(request_id).map(drop)
            }
            ScriptEventKind::WorldBlockSetResult {
                request_id,
                dimension,
                block_id,
                applied,
                failure,
                ..
            } => {
                validate_script_id(request_id)?;
                validate_contract_resource_id(dimension)?;
                validate_contract_resource_id(block_id)?;
                if *applied != failure.is_none() {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "world block set result",
                    });
                }
                Ok(())
            }
            ScriptEventKind::LoaderItemGrantResult {
                request_id,
                player_id,
                block_id,
                count,
                ..
            } => ScriptLoaderItemGrantRequest::try_new(request_id, *player_id, block_id, *count)
                .map(drop),
            ScriptEventKind::EntitySpawnResult {
                request_id,
                entity_type,
                position,
                ..
            } => {
                validate_script_id(request_id)?;
                validate_contract_resource_id(entity_type)?;
                ScriptPosition::try_new(position.x(), position.y(), position.z())
                    .ok_or(ScriptDtoError::InvalidBounds)
                    .map(drop)
            }
            ScriptEventKind::EntityDamageResult {
                request_id,
                entity_id,
                amount_bits,
                health_bits,
                killed,
                failure,
            } => {
                validate_script_id(request_id)?;
                ScriptEntityDamageRequest::try_new(
                    request_id,
                    *entity_id,
                    f32::from_bits(*amount_bits),
                )?;
                let health = health_bits.map(f32::from_bits);
                if health.is_some() != failure.is_none()
                    || *killed && health.is_none()
                    || health.is_some_and(|health| !health.is_finite() || health < 0.0)
                    || *killed && health.is_some_and(|health| health > 0.0)
                {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "entity damage result",
                    });
                }
                Ok(())
            }
            ScriptEventKind::OnlinePlayersResult {
                request_id,
                players,
                ..
            } => {
                validate_script_id(request_id)?;
                if players.len() > MAX_ONLINE_PLAYER_QUERY_LIMIT {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "online player snapshots",
                        max: MAX_ONLINE_PLAYER_QUERY_LIMIT,
                    });
                }
                for player in players {
                    player.validate()?;
                }
                Ok(())
            }
            ScriptEventKind::LoaderViewRequest { .. } => Ok(()),
            ScriptEventKind::LoaderViewAction {
                view_instance_id,
                view_revision,
                action_id,
                action_sequence,
                fields,
                selection_token,
                ..
            } => {
                validate_contract_resource_id(view_instance_id)?;
                validate_contract_resource_id(action_id)?;
                if *view_revision > MAX_CLIENT_VIEW_EXACT_INTEGER
                    || *action_sequence > MAX_CLIENT_VIEW_EXACT_INTEGER
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                if fields.len() > MAX_CLIENT_VIEW_FIELDS {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "view action fields",
                        max: MAX_CLIENT_VIEW_FIELDS,
                    });
                }
                let mut ids = std::collections::BTreeSet::new();
                for field in fields {
                    if !ids.insert(field.id()) {
                        return Err(ScriptDtoError::DuplicateId {
                            field: "view action field",
                            actual_bytes: field.id().len(),
                        });
                    }
                }
                if let Some(token) = selection_token.as_deref() {
                    validate_bounded_nonempty(
                        "view selection token",
                        token,
                        MAX_CLIENT_VIEW_ID_BYTES,
                    )?;
                }
                Ok(())
            }
            ScriptEventKind::ClientViewOpened {
                request_id,
                view_instance_id,
                revision,
                failure,
                ..
            } => {
                validate_script_id(request_id)?;
                if view_instance_id.is_some() != failure.is_none()
                    || revision.is_some() != view_instance_id.is_some()
                    || revision.is_some_and(|revision| {
                        revision == 0 || revision > MAX_CLIENT_VIEW_EXACT_INTEGER
                    })
                {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "client view opened result",
                    });
                }
                if let Some(id) = view_instance_id.as_deref() {
                    validate_contract_resource_id(id)?;
                }
                Ok(())
            }
            ScriptEventKind::ClientSelectionStarted {
                request_id,
                selection_context_id,
                expires_at_tick,
                failure,
                ..
            } => {
                validate_script_id(request_id)?;
                if selection_context_id.is_some() != expires_at_tick.is_some()
                    || selection_context_id.is_some() != failure.is_none()
                {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "client selection started result",
                    });
                }
                if let Some(id) = selection_context_id.as_deref() {
                    validate_contract_resource_id(id)?;
                }
                Ok(())
            }
            ScriptEventKind::ClientBrand { brand, .. } => {
                validate_bounded_value("client brand", brand, MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES)
            }
            ScriptEventKind::CustomPayload {
                channel, payload, ..
            } => {
                validate_contract_resource_id(channel)?;
                if payload.len() > MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES {
                    return Err(ScriptDtoError::ValueTooLong {
                        field: "custom payload",
                        max_bytes: MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES,
                        actual_bytes: payload.len(),
                    });
                }
                Ok(())
            }
        }
    }
}

/// Why a view lifecycle request was refused before it reached the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptClientViewFailure {
    /// The owner does not own the requested view or lacks the permission pair.
    Refused,
    /// The target player has no live Loader session.
    PlayerUnavailable,
    /// The referenced instance is not open (or already retired).
    UnknownInstance,
    /// The caller's expected revision no longer matches the live instance.
    StaleRevision,
    /// The model exceeded a hard bound.
    TooLarge,
}

impl ScriptClientViewFailure {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Refused => "refused",
            Self::PlayerUnavailable => "player_unavailable",
            Self::UnknownInstance => "unknown_instance",
            Self::StaleRevision => "stale_revision",
            Self::TooLarge => "too_large",
        }
    }
}

/// Script-visible event variants.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptEventKind {
    ServerStarted,
    ServerStopping {
        reason: String,
    },
    PlayerJoined {
        player_id: ScriptPlayerId,
        username: String,
        context: ScriptPlayerContext,
    },
    PlayerLeft {
        player_id: ScriptPlayerId,
        reason: String,
    },
    PlayerChat {
        player_id: ScriptPlayerId,
        message: String,
        context: ScriptPlayerContext,
    },
    PlayerBlockBroken {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        block_id: String,
        x: i32,
        y: i32,
        z: i32,
        game_mode: ScriptGameMode,
    },
    PlayerBlockPlaced {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        block_id: String,
        x: i32,
        y: i32,
        z: i32,
        game_mode: ScriptGameMode,
    },
    PlayerItemCrafted {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        item_id: String,
        count: u64,
        craft_count: u32,
        source: ScriptCraftingSource,
        game_mode: ScriptGameMode,
    },
    PlayerItemPickedUp {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        item_id: String,
        count: u64,
        source: ScriptItemPickupSource,
        game_mode: ScriptGameMode,
    },
    PlayerEntityKilled {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        entity_id: ScriptEntityId,
        entity_type: String,
        source: ScriptEntityKillSource,
        game_mode: ScriptGameMode,
    },
    PlayerEntityInteracted {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        entity_id: ScriptEntityId,
        entity_type: String,
        hand: ScriptInteractionHand,
        secondary_action: bool,
        game_mode: ScriptGameMode,
    },
    PlayerDied {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        game_mode: ScriptGameMode,
    },
    PlayerGraveCreated {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        x: i32,
        y: i32,
        z: i32,
        items: u32,
        spilled: u32,
    },
    PlayerCommand {
        player_id: ScriptPlayerId,
        username: String,
        root: String,
        arguments: String,
        context: ScriptPlayerContext,
    },
    ServerTick {
        tick: u64,
    },
    PluginStorageGetResult {
        request_id: String,
        key: String,
        value: Option<String>,
        version: Option<u64>,
        failure: Option<ScriptPluginStorageFailure>,
    },
    PluginStorageCasResult {
        request_id: String,
        key: String,
        applied: bool,
        version: Option<u64>,
        failure: Option<ScriptPluginStorageFailure>,
    },
    PluginStorageDeleteResult {
        request_id: String,
        key: String,
        deleted: bool,
        version: Option<u64>,
        failure: Option<ScriptPluginStorageFailure>,
    },
    InventoryMenuClicked {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        menu_id: String,
        slot: u8,
        click: ScriptInventoryClick,
    },
    InventoryStorageTransactionResult {
        request_id: String,
        committed: bool,
    },
    OperationResult {
        request_id: String,
        operation_id: Option<String>,
        outcome: ScriptOperationOutcome,
    },
    PlayerInventoryTransactionResult {
        request_id: String,
        player_id: ScriptPlayerId,
        failure: Option<ScriptPlayerInventoryFailure>,
    },
    PlayerZoneEntered {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone_id: String,
    },
    PlayerZoneExited {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone_id: String,
    },
    ZoneCommandResult {
        zone_id: String,
        accepted: bool,
    },
    PlayerTeleportResult {
        request_id: String,
        player_id: ScriptPlayerId,
        position: ScriptPosition,
        failure: Option<ScriptPlayerTeleportFailure>,
    },
    WorldTimeSetResult {
        request_id: String,
        world_time: u64,
        failure: Option<ScriptWorldTimeSetFailure>,
    },
    WorldBlockSetResult {
        request_id: String,
        dimension: String,
        block_id: String,
        x: i32,
        y: i32,
        z: i32,
        applied: bool,
        failure: Option<ScriptWorldBlockSetFailure>,
    },
    LoaderItemGrantResult {
        request_id: String,
        player_id: ScriptPlayerId,
        block_id: String,
        count: u8,
        failure: Option<ScriptLoaderItemGrantFailure>,
    },
    EntitySpawnResult {
        request_id: String,
        player_id: ScriptPlayerId,
        entity_type: String,
        position: ScriptPosition,
        failure: Option<ScriptEntitySpawnFailure>,
    },
    EntityDamageResult {
        request_id: String,
        entity_id: ScriptEntityId,
        amount_bits: u32,
        health_bits: Option<u32>,
        killed: bool,
        failure: Option<ScriptEntityDamageFailure>,
    },
    OnlinePlayersResult {
        request_id: String,
        players: Vec<ScriptOnlinePlayerSnapshot>,
        truncated: bool,
    },
    /// One explicit key-driven view request admitted for the owning plugin.
    LoaderViewRequest {
        player_id: ScriptPlayerId,
        request_kind: ScriptClientViewKey,
    },
    /// One admitted client view action after the server re-read the instance.
    LoaderViewAction {
        player_id: ScriptPlayerId,
        view_instance_id: String,
        view_revision: u64,
        action_id: String,
        action_sequence: u64,
        fields: Vec<ScriptClientViewField>,
        selection_token: Option<String>,
    },
    /// Outcome of one `open_client_view` request.
    ClientViewOpened {
        request_id: String,
        player_id: ScriptPlayerId,
        view_instance_id: Option<String>,
        revision: Option<u64>,
        failure: Option<ScriptClientViewFailure>,
    },
    /// Outcome of one `begin_client_selection` request.
    ClientSelectionStarted {
        request_id: String,
        player_id: ScriptPlayerId,
        selection_context_id: Option<String>,
        expires_at_tick: Option<u64>,
        failure: Option<ScriptClientViewFailure>,
    },
    ClientBrand {
        player_id: ScriptPlayerId,
        brand: String,
    },
    CustomPayload {
        player_id: ScriptPlayerId,
        phase: ScriptProtocolPhase,
        channel: String,
        payload: Vec<u8>,
    },
}

/// Outbound command requests emitted by component code.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptCommand {
    /// A request emitted by a component and attested by its host. Guest code has
    /// no API for constructing `ScriptCommandProvenance` or selecting another
    /// plugin id.
    HostAttached {
        provenance: ScriptCommandProvenance,
        request: Arc<ScriptCommand>,
    },
    SendChatMessage {
        player_id: ScriptPlayerId,
        message: String,
    },
    BroadcastChatMessage {
        message: String,
    },
    DisconnectPlayer {
        player_id: ScriptPlayerId,
        reason: String,
    },
    UnlockSession {
        player_id: ScriptPlayerId,
    },
    SpawnEntity {
        request_id: String,
        actor: ScriptPlayerId,
        entity_type: String,
        position: ScriptPosition,
    },
    DamageEntity {
        request: ScriptEntityDamageRequest,
    },
    PluginStorageGet {
        request: ScriptPluginStorageGetRequest,
    },
    PluginStorageCompareAndSwap {
        request: ScriptPluginStorageCompareAndSwapRequest,
    },
    PluginStorageDelete {
        request: ScriptPluginStorageDeleteRequest,
    },
    Operation {
        request: ScriptOperationRequest,
    },
    OpenInventoryMenu {
        player_id: ScriptPlayerId,
        menu: ScriptInventoryMenu,
    },
    OpenClientView {
        request: ScriptClientViewOpen,
    },
    PresentClientView {
        request: ScriptClientViewPresent,
    },
    CloseClientView {
        player_id: ScriptPlayerId,
        view_instance_id: String,
    },
    BeginClientSelection {
        request_id: String,
        player_id: ScriptPlayerId,
        view_instance_id: String,
        view_revision: u64,
        action_id: String,
        constraints: ScriptClientSelectionConstraints,
    },
    CancelClientSelection {
        player_id: ScriptPlayerId,
        selection_context_id: String,
    },
    ClientSound {
        player_id: ScriptPlayerId,
        sound: ScriptClientSound,
    },
    GrantLoaderBlockItem {
        request: ScriptLoaderItemGrantRequest,
    },
    CloseInventoryMenu {
        player_id: ScriptPlayerId,
        menu_id: String,
    },
    InventoryStorageTransaction {
        transaction: ScriptInventoryStorageTransaction,
    },
    PlayerInventoryTransaction {
        transaction: ScriptPlayerInventoryTransaction,
    },
    UpsertZone {
        zone: ScriptAxisAlignedZone,
    },
    RemoveZone {
        zone_id: String,
    },
    TeleportPlayer {
        request: ScriptPlayerTeleportRequest,
    },
    SetWorldTime {
        request: ScriptWorldTimeSetRequest,
    },
    SetWorldBlock {
        request: ScriptWorldBlockSetRequest,
    },
    ListOnlinePlayers {
        request: ScriptOnlinePlayersRequest,
    },
    SendCustomPayload {
        player_id: ScriptPlayerId,
        channel: String,
        payload: Vec<u8>,
    },
}

/// Bounded state returned when the script event queue cannot accept an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptQueueError {
    Full,
    Closed,
}

/// Rejection returned by the public raw-command submission path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptCommandSubmissionError {
    ProvenanceRejected,
    PermissionDenied {
        capability: ScriptCommandCapabilityKind,
    },
    InvalidCommand {
        error: ScriptDtoError,
    },
    QueueFull,
    QueueClosed,
}

/// Why a plugin's already-validated batch did not reach the command queue.
///
/// The batch comes back inside the error, so a caller can report exactly which
/// commands were refused instead of losing them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptBatchSubmissionError {
    Full(CommandBatch),
    Closed(CommandBatch),
    Rejected {
        batch: CommandBatch,
        error: CommandBatchError,
    },
}

/// Why a candidate deployment did not reach the live boundary.
///
/// Only the trusted host sees this: the commit path keeps readiness, route
/// ownership, the caller's swap and command delivery in one order for every
/// caller, so a refused reload leaves the running generation untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptReloadCommitError {
    QueueFull,
    QueueClosed,
    Rejected { error: CommandBatchError },
    Ownership { error: ScriptRouteRegistrationError },
}

/// Trusted control input of the host half of the boundary.
///
/// This is not a server observation and never a guest DTO: the boundary moves a
/// payload between the two trusted halves without interpreting it, and only the
/// runtime that queued it knows what it carries. A consumer that serves events
/// only drops such input unread, which closes whatever response its payload holds.
#[non_exhaustive]
pub enum ScriptHostInput {
    /// One immutable server event, in one FIFO with every other event.
    Event(ScriptHostEvent),
    /// Host-private control: a reload envelope whose payload only its sender reads.
    Reload(Box<dyn std::any::Any + Send>),
    /// One native pre-commit question, in the same FIFO and the same order.
    ///
    /// It is a request rather than an event because only the host can answer it:
    /// the boundary moves it to the runtime that runs the registered handlers, and
    /// the runtime answers it through the request's own one-shot reply. A consumer
    /// that only serves events drops it unread, which answers the owner with
    /// [`precommit::HookFailure::Unavailable`] instead of leaving it waiting.
    Precommit(precommit::Request),
}

/// A server event admitted to the trusted host mailbox.
///
/// The wrapped event remains the guest-visible DTO. Target registration state is
/// host-private so an event admitted for a retired plugin generation cannot be
/// forged for its replacement.
#[derive(Debug, Clone)]
pub struct ScriptHostEvent {
    event: ScriptEvent,
    target_generation: EventTargetGeneration,
}

#[derive(Debug, Clone, Copy)]
enum EventTargetGeneration {
    Untargeted,
    Targeted(Option<u64>),
}

impl ScriptHostEvent {
    fn untargeted(event: ScriptEvent) -> Self {
        Self {
            event,
            target_generation: EventTargetGeneration::Untargeted,
        }
    }

    /// Recover the immutable guest-visible event after the host accepts it.
    #[must_use]
    pub fn into_event(self) -> ScriptEvent {
        self.event
    }
}

impl Deref for ScriptHostEvent {
    type Target = ScriptEvent;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}

/// Trusted handle the host queues its own control input through.
///
/// It is deliberately narrow — a host can queue a reload, not fabricate events
/// or reach the queue — and deliberately weak: the boundary drops the ability to
/// queue control when it closes its host side, so a reload after shutdown reports
/// a closed queue instead of resurrecting the host. Control input shares the one
/// bounded mailbox and its FIFO with events; there is no second queue.
#[derive(Debug, Clone)]
pub struct ScriptHostInputSender {
    sender: mpsc::WeakSender<ScriptHostInput>,
}

impl ScriptHostInputSender {
    /// Queue one reload envelope, waiting for bounded mailbox capacity.
    ///
    /// A boundary that closed its host side stops the weak upgrade, and a closed
    /// send fails the same way: both report [`ScriptQueueError::Closed`], which is
    /// what a reload caller turns into its own closed-host result. Capacity is
    /// awaited rather than reported, so [`ScriptQueueError::Full`] is never
    /// returned from here.
    pub async fn send_reload(
        &self,
        payload: Box<dyn std::any::Any + Send>,
    ) -> Result<(), ScriptQueueError> {
        let Some(sender) = self.sender.upgrade() else {
            return Err(ScriptQueueError::Closed);
        };
        sender
            .send(ScriptHostInput::Reload(payload))
            .await
            .map_err(|_| ScriptQueueError::Closed)
    }
}

/// One deployed package the script host discovered.
///
/// The host already parses each package manifest; this exposes the facts core
/// needs to trigger authored-data subsystems (an authored catalog directory and
/// the manifest's declared required features) without a second manifest reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPackage {
    plugin_id: String,
    package_dir: PathBuf,
    required_features: Vec<String>,
}

impl PluginPackage {
    #[must_use]
    pub fn new(
        plugin_id: impl Into<String>,
        package_dir: impl Into<PathBuf>,
        required_features: Vec<String>,
    ) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            package_dir: package_dir.into(),
            required_features,
        }
    }

    #[must_use]
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Root directory of the deployed package (`plugin.toml`'s parent).
    #[must_use]
    pub fn package_dir(&self) -> &Path {
        &self.package_dir
    }

    #[must_use]
    pub fn required_features(&self) -> &[String] {
        &self.required_features
    }

    /// Whether the deployed manifest declared `feature` in `required_features`.
    #[must_use]
    pub fn declares_feature(&self, feature: &str) -> bool {
        self.required_features.iter().any(|entry| entry == feature)
    }
}

/// Error returned when a command batch cannot accept another command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandBatchError {
    Full {
        limit: NonZeroUsize,
    },
    PermissionDenied {
        capability: ScriptCommandCapabilityKind,
    },
    ProvenanceRejected,
    InvalidCommand {
        error: ScriptDtoError,
    },
    AdmissionUnavailable,
}

/// Bounded list of commands produced by one script event invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBatch {
    limit: NonZeroUsize,
    commands: Vec<ScriptCommand>,
}

impl CommandBatch {
    /// Create an empty command batch with a fixed command count limit.
    pub fn new(limit: NonZeroUsize) -> Self {
        let limit = NonZeroUsize::new(limit.get().min(MAX_SCRIPT_COMMAND_BATCH))
            .expect("script command batch limit is non-zero");
        Self {
            limit,
            commands: Vec::new(),
        }
    }

    /// Return the maximum number of commands this batch may contain.
    pub fn limit(&self) -> NonZeroUsize {
        self.limit
    }

    /// Return queued commands as an immutable slice.
    pub fn commands(&self) -> &[ScriptCommand] {
        &self.commands
    }

    /// Consume this batch and return the queued commands.
    pub fn into_commands(self) -> Vec<ScriptCommand> {
        self.commands
    }

    /// Try to append one command without exceeding the batch limit.
    pub fn try_push(&mut self, command: ScriptCommand) -> Result<(), CommandBatchError> {
        if matches!(command, ScriptCommand::HostAttached { .. }) {
            return Err(CommandBatchError::ProvenanceRejected);
        }
        command
            .validate_contract()
            .map_err(|error| CommandBatchError::InvalidCommand { error })?;
        if let Some(capability) = command.required_capability_kind() {
            return Err(CommandBatchError::PermissionDenied { capability });
        }
        self.try_push_validated(command)
    }

    fn try_push_validated(&mut self, command: ScriptCommand) -> Result<(), CommandBatchError> {
        if self.commands.len() >= self.limit.get() {
            return Err(CommandBatchError::Full { limit: self.limit });
        }
        self.commands.push(command);
        Ok(())
    }

    /// Try to append one command if granted by the host capability allow-list.
    pub fn try_push_authorized(
        &mut self,
        command: ScriptCommand,
        capabilities: &CommandCapabilities,
    ) -> Result<(), CommandBatchError> {
        if matches!(command, ScriptCommand::HostAttached { .. }) {
            return Err(CommandBatchError::ProvenanceRejected);
        }
        command
            .validate_contract()
            .map_err(|error| CommandBatchError::InvalidCommand { error })?;
        if let Some(capability) = command.required_capability()
            && !capabilities.allows(capability)
        {
            return Err(CommandBatchError::PermissionDenied {
                capability: capability.kind(),
            });
        }
        self.try_push_validated(command)
    }
}

fn validate_script_id(value: &str) -> Result<String, ScriptDtoError> {
    validate_script_id_value(value)?;
    Ok(value.to_owned())
}

/// Validate the contract's own script-id shape without allocating.
///
/// The no-allocation form of the contract's script-id check, for a host that only
/// needs the refusal.
pub fn validate_script_id_value(value: &str) -> Result<(), ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue { field: "script id" });
    }
    if value.len() > MAX_SCRIPT_ID_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "script id",
            max_bytes: MAX_SCRIPT_ID_BYTES,
            actual_bytes: value.len(),
        });
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
    }) {
        return Err(ScriptDtoError::InvalidId {
            field: "script id",
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

fn validate_bounded_value(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), ScriptDtoError> {
    if value.len() > max_bytes {
        return Err(ScriptDtoError::ValueTooLong {
            field,
            max_bytes,
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

fn validate_bounded_nonempty(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue { field });
    }
    validate_bounded_value(field, value, max_bytes)
}

fn validate_player_identity(uuid: &str, username: &str) -> Result<(), ScriptDtoError> {
    if !uuid
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ScriptDtoError::InvalidId {
            field: "player uuid",
            actual_bytes: uuid.len(),
        });
    }
    if !username
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ScriptDtoError::InvalidId {
            field: "player username",
            actual_bytes: username.len(),
        });
    }
    Ok(())
}

fn normalize_player_uuid(uuid: &str) -> Result<String, ScriptDtoError> {
    let normalized = uuid
        .bytes()
        .filter(|byte| *byte != b'-')
        .map(char::from)
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized.len() != 32 || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ScriptDtoError::InvalidId {
            field: "protection actor uuid",
            actual_bytes: uuid.len(),
        });
    }
    Ok(normalized)
}

/// Validate one namespaced contract resource identifier, returning it on success.
pub fn validate_contract_resource_id(value: &str) -> Result<String, ScriptDtoError> {
    check_contract_resource_id(value)?;
    Ok(value.to_owned())
}

fn check_contract_resource_id(value: &str) -> Result<(), ScriptDtoError> {
    if value.len() > MAX_SCRIPT_RESOURCE_ID_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "resource id",
            max_bytes: MAX_SCRIPT_RESOURCE_ID_BYTES,
            actual_bytes: value.len(),
        });
    }
    let Some((namespace, path)) = value.split_once(':') else {
        return Err(ScriptDtoError::InvalidResourceId {
            field: "resource id",
            actual_bytes: value.len(),
        });
    };
    if namespace.is_empty()
        || path.is_empty()
        || path.contains(':')
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
        || !path.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b'/')
        })
    {
        return Err(ScriptDtoError::InvalidResourceId {
            field: "resource id",
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

fn validate_plugin_storage_key(value: &str) -> Result<String, ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue {
            field: "plugin storage key",
        });
    }
    if value.len() > MAX_PLUGIN_STORAGE_KEY_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "plugin storage key",
            max_bytes: MAX_PLUGIN_STORAGE_KEY_BYTES,
            actual_bytes: value.len(),
        });
    }
    Ok(value.to_owned())
}

fn validate_plugin_storage_value(value: &str) -> Result<(), ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue {
            field: "plugin storage value",
        });
    }
    if value.len() > MAX_PLUGIN_STORAGE_VALUE_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "plugin storage value",
            max_bytes: MAX_PLUGIN_STORAGE_VALUE_BYTES,
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

fn validate_target_plugin_id(value: &str) -> Result<String, ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue {
            field: "target plugin id",
        });
    }
    if value.len() > MAX_SCRIPT_ID_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "target plugin id",
            max_bytes: MAX_SCRIPT_ID_BYTES,
            actual_bytes: value.len(),
        });
    }
    if !is_valid_plugin_id(value) {
        return Err(ScriptDtoError::InvalidId {
            field: "target plugin id",
            actual_bytes: value.len(),
        });
    }
    Ok(value.to_owned())
}

mod runtime;
pub use route_authority::{
    ScriptBoundary, ScriptHostEndpoint, ScriptRouteRegistrationError, script_boundary_pair,
};
pub use runtime::{RuntimeContext, RuntimeControls, RuntimeError, RuntimeResult, ScriptRuntime};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
