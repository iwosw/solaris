//! Durable player/world persistence records, journals, and their NBT codecs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::Compression as GzipCompression;
use flate2::bufread::GzDecoder;
use flate2::write::GzEncoder;
use mc_data::entity_types::{
    EntityArchetype, EntityTypeRegistry, entity_type_contract_26_1_2_by_name,
};
use mc_data::items::ItemRegistry;
use mc_data::{Identifier, ItemStack};
use mc_domain::GameMode;
use mc_entity::{
    EntityId, EntityItemStack, EntityLifecycle, EntityMainHandEquipment, EntitySnapshot,
    EntityStore, RegionPhase, RegionalCommitDecision, RegionalDecisionJournal,
    RegionalDecisionJournalError, Vec3,
};
use mc_nbt::{ListTag, Tag, tag_type};
use serde::Deserialize;
use thiserror::Error;

pub mod dirty_flush;
mod effects;
pub mod inventory_recovery;
pub mod native_entities;
pub mod native_overlay;
mod stack_components;
mod stew_effects;
pub mod world_journal;

use crate::inventory::PlayerInventory;
use crate::survival::SurvivalState;
use inventory_recovery::{INVENTORY_OPERATION_REVISION_FIELD, PlayerOwnedInventory};

pub const CARRIED_ITEM_FIELD: &str = "SolarisCarriedItem";
pub const CRAFTING_TABLE_INPUT_FIELD: &str = "SolarisCraftingTableInput";
pub const ENCHANTING_TABLE_INPUT_FIELD: &str = "SolarisEnchantingTableInput";
pub const MERCHANT_INPUT_FIELD: &str = "SolarisMerchantInput";

pub fn slot_field(fields: &[(String, Tag)]) -> Option<usize> {
    match field(fields, "Slot")? {
        Tag::Byte(value) => Some(*value as u8 as usize),
        Tag::Short(value) => usize::try_from(*value).ok(),
        Tag::Int(value) => usize::try_from(*value).ok(),
        _ => None,
    }
}

pub fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, tag)| tag)
}

pub fn int_field(fields: &[(String, Tag)], name: &str) -> Option<i32> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(i32::from(*value)),
        Tag::Short(value) => Some(i32::from(*value)),
        Tag::Int(value) => Some(*value),
        _ => None,
    }
}

pub fn string_field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a str> {
    match field(fields, name)? {
        Tag::String(value) => Some(value),
        _ => None,
    }
}

const PLAYERDATA_DIR: &str = "playerdata";
const MAX_PERSISTENCE_COMPRESSED_NBT_BYTES: usize = mc_nbt::MAX_NBT_TOTAL_BYTES;
const MAX_PERSISTENCE_DECOMPRESSED_NBT_BYTES: usize = mc_nbt::MAX_NBT_TOTAL_BYTES;
const SOLARIS_DIR: &str = "solaris";
const ENTITIES_FILE: &str = "entities.dat";
const ENTITY_FORMAT_VERSION_FIELD: &str = "SolarisEntityFormatVersion";
const ENTITY_FORMAT_VERSION: i32 = 3;
const ENTITY_LIFECYCLE_TICK_FIELD: &str = "SolarisEntityLifecycleTick";
const ENTITY_REGIONAL_SEQUENCE_FIELD: &str = "SolarisRegionalSequenceWatermark";
const ENTITY_ALLOCATION_HIGH_WATER_FIELD: &str = "SolarisEntityAllocationHighWater";
const ENTITY_GENERATED_ENTITY_CLAIMS_FIELD: &str = "SolarisGeneratedEntityClaims";
const ENTITY_LIFECYCLE_FIELD: &str = "SolarisLifecycle";
const ENTITY_ATTRIBUTES_FIELD: &str = "SolarisAttributes";
const ENTITY_RETAINED_STATE_FIELD: &str = "SolarisRetainedState";
const ENTITY_HEAD_YAW_FIELD: &str = "SolarisHeadYaw";
const ENTITY_GOAL_STATE_FIELD: &str = "SolarisGoalState";
const ENTITY_VEHICLE_STATE_FIELD: &str = "SolarisVehicleState";
const ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2: f64 = 3.0000512E7;
const ENTITY_VERTICAL_POSITION_LIMIT_26_1_2: f64 = 2.0E7;
const ENTITY_VELOCITY_LIMIT_26_1_2: f64 = 10.0;
const ENTITY_TICKS_PER_SECOND: f64 = 20.0;
const WORLD_FILE: &str = "world.dat";
const REGIONAL_DECISION_JOURNAL_FILE: &str = "entity-owner-journal.json";
const REGIONAL_DECISION_JOURNAL_HEADER: &[u8] = b"SOLARIS_ENTITY_OWNER_JOURNAL 3\n";
const REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES: usize = 8;
// One recovery set supports the documented 30k-entity workload. The byte caps leave room for
// rich entity snapshots while bounding crash recovery and writer memory independently.
const MAX_REGIONAL_DECISION_JOURNAL_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_REGIONAL_DECISION_FRAME_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_REGIONAL_DECISIONS_PER_FRAME: usize = 30_000;
const MAX_REGIONAL_ENTITY_MUTATIONS_PER_FRAME: usize = 30_000;
const DAMAGE_COMPONENT: &str = "minecraft:damage";
const CUSTOM_NAME_COMPONENT: &str = "minecraft:custom_name";
const ITEM_MODEL_COMPONENT: &str = "minecraft:item_model";
const ENCHANTMENTS_COMPONENT: &str = "minecraft:enchantments";
static TMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum RegionalDecisionJournalOpenError {
    #[error("regional decision journal IO failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("regional decision journal JSON failed at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported regional decision journal version {0}")]
    UnsupportedVersion(u32),
    #[error("regional decision journal framing failed at {path}: {reason}")]
    Framing { path: PathBuf, reason: &'static str },
    #[error("regional decision journal checksum failed at {path}")]
    Checksum { path: PathBuf },
    #[error("regional decision journal validation failed at {path}: {source}")]
    Validation {
        path: PathBuf,
        #[source]
        source: RegionalDecisionReplayError,
    },
}

#[derive(Debug, Error)]
pub enum RegionalDecisionReplayError {
    #[error("regional decision phases and sequence watermarks must be strictly increasing")]
    InvalidOrdering,
    #[error("regional decision violates upsert/removal invariants")]
    InvalidDecision,
    #[error("regional decision contains an invalid entity snapshot")]
    InvalidSnapshot,
    #[error("regional decision recovery exceeds the supported 30,000-decision boundary")]
    TooManyDecisions,
    #[error("regional decision recovery exceeds the supported 30,000-entity mutation boundary")]
    TooManyEntityMutations,
    #[error("duplicate restored entity UUID {0}")]
    DuplicateEntityUuid(uuid::Uuid),
}

impl From<RegionalDecisionReplayError> for std::io::Error {
    fn from(source: RegionalDecisionReplayError) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, source)
    }
}

#[derive(Deserialize)]
struct EncodedRegionalCommitDecision {
    phase: RegionPhase,
    sequence_watermark: u64,
    lifecycle_epoch: u64,
    upserts: Vec<EntitySnapshot>,
    removed: Vec<EntityId>,
    #[serde(default)]
    inventory_pickup: Option<mc_entity::RegionalItemPickupParticipant>,
}

impl TryFrom<EncodedRegionalCommitDecision> for RegionalCommitDecision {
    type Error = RegionalDecisionReplayError;

    fn try_from(encoded: EncodedRegionalCommitDecision) -> Result<Self, Self::Error> {
        let decision = RegionalCommitDecision::from_parts_at_lifecycle_epoch(
            encoded.phase,
            encoded.sequence_watermark,
            encoded.lifecycle_epoch,
            encoded.upserts,
            encoded.removed,
        )
        .map_err(|_| RegionalDecisionReplayError::InvalidDecision)?;
        Ok(match encoded.inventory_pickup {
            Some(pickup) => decision.with_inventory_pickup(pickup),
            None => decision,
        })
    }
}

pub struct FileRegionalDecisionJournal {
    path: PathBuf,
    pending: Vec<RegionalCommitDecision>,
    needs_compaction: bool,
    writer: std::sync::Arc<world_journal::JournalWriter>,
}

impl FileRegionalDecisionJournal {
    #[cfg(test)]
    pub(crate) fn open_for_test(
        root: &Path,
    ) -> Result<(Self, Vec<RegionalCommitDecision>), RegionalDecisionJournalOpenError> {
        Self::open(root, world_journal::JournalWriter::open(root).unwrap())
    }

    pub fn open(
        world_root: &Path,
        writer: std::sync::Arc<world_journal::JournalWriter>,
    ) -> Result<(Self, Vec<RegionalCommitDecision>), RegionalDecisionJournalOpenError> {
        let path = world_root
            .join(SOLARIS_DIR)
            .join(REGIONAL_DECISION_JOURNAL_FILE);
        let pending = if path.is_file() {
            let bytes = read_regional_decision_journal_file(&path)?;
            let (pending, valid_len) = read_appended_regional_decisions(&path, &bytes)?;
            if valid_len < bytes.len() {
                let file = OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .map_err(|source| RegionalDecisionJournalOpenError::Io {
                        path: path.clone(),
                        source,
                    })?;
                file.set_len(valid_len as u64)
                    .and_then(|()| file.sync_all())
                    .map_err(|source| RegionalDecisionJournalOpenError::Io {
                        path: path.clone(),
                        source,
                    })?;
            }
            pending
        } else {
            Vec::new()
        };
        let journal = Self {
            path,
            pending: pending.clone(),
            needs_compaction: false,
            writer,
        };
        Ok((journal, pending))
    }

    fn persist(&self) -> Result<(), RegionalDecisionJournalOpenError> {
        let (reply, completion) = std::sync::mpsc::channel();
        self.writer
            .requests
            .send(world_journal::WriterRequest::EntityReplace {
                pending: self.pending.clone(),
                reply,
            })
            .map_err(|_| journal_writer_closed(&self.path))?;
        completion
            .recv()
            .map_err(|_| journal_writer_closed(&self.path))?
            .map_err(|_| journal_writer_closed(&self.path))
    }
}

impl Drop for FileRegionalDecisionJournal {
    fn drop(&mut self) {
        if self.needs_compaction {
            let _ = self.persist();
        } else {
            let _ = self.writer.flush();
        }
    }
}

fn append_regional_decisions(
    path: &Path,
    decisions: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionJournalOpenError> {
    if decisions.is_empty() {
        return Ok(());
    }
    validate_regional_commit_decisions(decisions)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    validate_regional_decision_group(decisions)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    let parent = path.parent().expect("journal path has parent");
    create_regional_journal_directory(parent)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if file
        .metadata()
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len()
        == 0
    {
        file.write_all(REGIONAL_DECISION_JOURNAL_HEADER)
            .map_err(|source| RegionalDecisionJournalOpenError::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }
    write_regional_decision_group(&mut file, path, decisions)?;
    Ok(())
}

fn persist_regional_decisions(
    path: &Path,
    pending: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionJournalOpenError> {
    validate_regional_commit_decisions(pending)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    validate_regional_decision_group(pending)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    let parent = path.parent().expect("journal path has parent");
    create_regional_journal_directory(parent)?;
    if pending.is_empty() {
        if path.is_file() {
            std::fs::remove_file(path).map_err(|source| RegionalDecisionJournalOpenError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            sync_regional_journal_directory(parent)?;
        }
        return Ok(());
    }
    let temporary = temporary_write_path(path);
    let mut file =
        File::create(&temporary).map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: temporary.clone(),
            source,
        })?;
    file.write_all(REGIONAL_DECISION_JOURNAL_HEADER)
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: temporary.clone(),
            source,
        })?;
    write_regional_decision_group(&mut file, &temporary, pending)?;
    file.flush()
        .and_then(|()| file.sync_all())
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: temporary.clone(),
            source,
        })?;
    mc_world::atomic_file::replace_file(&temporary, path).map_err(|source| {
        RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    sync_regional_journal_directory(parent)
}

fn journal_writer_closed(path: &Path) -> RegionalDecisionJournalOpenError {
    RegionalDecisionJournalOpenError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "regional journal writer closed",
        ),
    }
}

fn create_regional_journal_directory(path: &Path) -> Result<(), RegionalDecisionJournalOpenError> {
    let existed = path.is_dir();
    std::fs::create_dir_all(path).map_err(|source| RegionalDecisionJournalOpenError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !existed && let Some(parent) = path.parent() {
        sync_regional_journal_directory(parent)?;
    }
    Ok(())
}

fn read_appended_regional_decisions(
    path: &Path,
    bytes: &[u8],
) -> Result<(Vec<RegionalCommitDecision>, usize), RegionalDecisionJournalOpenError> {
    if !bytes.starts_with(REGIONAL_DECISION_JOURNAL_HEADER) {
        return Err(RegionalDecisionJournalOpenError::UnsupportedVersion(0));
    }
    let mut cursor = REGIONAL_DECISION_JOURNAL_HEADER.len();
    let mut pending = Vec::new();
    while cursor < bytes.len() {
        let frame_start = cursor;
        let remaining = bytes.len() - cursor;
        if remaining < REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES {
            return validate_regional_recovery_prefix(path, pending, frame_start);
        }
        let mut payload_len_bytes = [0_u8; 4];
        payload_len_bytes.copy_from_slice(&bytes[cursor..cursor + 4]);
        let payload_len = u32::from_be_bytes(payload_len_bytes) as u64;
        validate_regional_journal_frame_payload_len(payload_len).map_err(|reason| {
            RegionalDecisionJournalOpenError::Framing {
                path: path.to_path_buf(),
                reason,
            }
        })?;
        let payload_len = usize::try_from(payload_len).map_err(|_| {
            RegionalDecisionJournalOpenError::Framing {
                path: path.to_path_buf(),
                reason: "record payload does not fit this platform",
            }
        })?;
        let mut checksum_bytes = [0_u8; 4];
        checksum_bytes.copy_from_slice(
            &bytes[cursor + 4..cursor + REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES],
        );
        let expected_checksum = u32::from_be_bytes(checksum_bytes);
        cursor += REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES;
        let payload_end = cursor.checked_add(payload_len).ok_or_else(|| {
            RegionalDecisionJournalOpenError::Framing {
                path: path.to_path_buf(),
                reason: "record length overflow",
            }
        })?;
        let Some(payload) = bytes.get(cursor..payload_end) else {
            return validate_regional_recovery_prefix(path, pending, frame_start);
        };
        if crc32fast::hash(payload) != expected_checksum {
            return Err(RegionalDecisionJournalOpenError::Checksum {
                path: path.to_path_buf(),
            });
        }
        let encoded_group = serde_json::from_slice::<Vec<EncodedRegionalCommitDecision>>(payload)
            .map_err(|source| RegionalDecisionJournalOpenError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        if encoded_group.is_empty() {
            return Err(RegionalDecisionJournalOpenError::Framing {
                path: path.to_path_buf(),
                reason: "empty commit group",
            });
        }
        let group = encoded_group
            .into_iter()
            .map(RegionalCommitDecision::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| regional_decision_validation_error(path, source))?;
        validate_regional_decision_group(&group)
            .map_err(|source| regional_decision_validation_error(path, source))?;
        pending.extend(group);
        cursor = payload_end;
    }
    validate_regional_recovery_prefix(path, pending, cursor)
}

fn validate_regional_recovery_prefix(
    path: &Path,
    pending: Vec<RegionalCommitDecision>,
    valid_len: usize,
) -> Result<(Vec<RegionalCommitDecision>, usize), RegionalDecisionJournalOpenError> {
    validate_regional_commit_decisions(&pending)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    validate_regional_decision_group(&pending)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    Ok((pending, valid_len))
}

fn write_regional_decision_group(
    file: &mut File,
    path: &Path,
    decisions: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionJournalOpenError> {
    validate_regional_decision_group(decisions)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    let mut payload = BoundedRegionalDecisionPayload::new();
    let serialization = serde_json::to_writer(&mut payload, decisions);
    if payload.limit_exceeded {
        return Err(RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason: "record payload exceeds operational limit",
        });
    }
    serialization.map_err(|source| RegionalDecisionJournalOpenError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    let payload = payload.bytes;
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason: "record payload exceeds u32 length",
        })?;
    let current_len = file
        .metadata()
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let next_len = current_len
        .checked_add(REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES as u64)
        .and_then(|length| length.checked_add(payload.len() as u64))
        .ok_or_else(|| RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason: "journal file length overflow",
        })?;
    validate_regional_journal_file_len(next_len).map_err(|reason| {
        RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason,
        }
    })?;
    file.write_all(&payload_len.to_be_bytes())
        .and_then(|()| file.write_all(&crc32fast::hash(&payload).to_be_bytes()))
        .and_then(|()| file.write_all(&payload))
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })
}

struct BoundedRegionalDecisionPayload {
    bytes: Vec<u8>,
    limit_exceeded: bool,
}

impl BoundedRegionalDecisionPayload {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            limit_exceeded: false,
        }
    }
}

impl Write for BoundedRegionalDecisionPayload {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next_len = self.bytes.len().checked_add(bytes.len()).ok_or_else(|| {
            self.limit_exceeded = true;
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WAL payload length overflow",
            )
        })?;
        if u64::try_from(next_len).unwrap_or(u64::MAX) > MAX_REGIONAL_DECISION_FRAME_PAYLOAD_BYTES {
            self.limit_exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WAL payload exceeds operational limit",
            ));
        }
        self.bytes.try_reserve(bytes.len()).map_err(|source| {
            std::io::Error::other(format!("WAL payload allocation failed: {source}"))
        })?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn read_regional_decision_journal_file(
    path: &Path,
) -> Result<Vec<u8>, RegionalDecisionJournalOpenError> {
    let file = File::open(path).map_err(|source| RegionalDecisionJournalOpenError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file_len = file
        .metadata()
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    validate_regional_journal_file_len(file_len).map_err(|reason| {
        RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason,
        }
    })?;
    let capacity =
        usize::try_from(file_len).map_err(|_| RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason: "journal file length does not fit this platform",
        })?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(format!("journal allocation failed: {source}")),
        })?;
    file.take(MAX_REGIONAL_DECISION_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    validate_regional_journal_file_len(bytes.len() as u64).map_err(|reason| {
        RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason,
        }
    })?;
    Ok(bytes)
}

fn validate_regional_journal_file_len(length: u64) -> Result<(), &'static str> {
    if length > MAX_REGIONAL_DECISION_JOURNAL_BYTES {
        return Err("journal file exceeds operational limit");
    }
    Ok(())
}

fn validate_regional_journal_frame_payload_len(length: u64) -> Result<(), &'static str> {
    if length > MAX_REGIONAL_DECISION_FRAME_PAYLOAD_BYTES {
        return Err("record payload exceeds operational limit");
    }
    Ok(())
}

fn validate_regional_decision_group_shape(
    decision_count: usize,
    entity_mutation_count: usize,
) -> Result<(), RegionalDecisionReplayError> {
    if decision_count > MAX_REGIONAL_DECISIONS_PER_FRAME {
        return Err(RegionalDecisionReplayError::TooManyDecisions);
    }
    if entity_mutation_count > MAX_REGIONAL_ENTITY_MUTATIONS_PER_FRAME {
        return Err(RegionalDecisionReplayError::TooManyEntityMutations);
    }
    Ok(())
}

fn validate_regional_decision_group(
    decisions: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionReplayError> {
    let entity_mutation_count = decisions.iter().try_fold(0_usize, |count, decision| {
        count
            .checked_add(decision.upserts().len())
            .and_then(|count| count.checked_add(decision.removed().len()))
            .ok_or(RegionalDecisionReplayError::TooManyEntityMutations)
    })?;
    validate_regional_decision_group_shape(decisions.len(), entity_mutation_count)
}

fn regional_decision_validation_error(
    path: &Path,
    source: RegionalDecisionReplayError,
) -> RegionalDecisionJournalOpenError {
    RegionalDecisionJournalOpenError::Validation {
        path: path.to_path_buf(),
        source,
    }
}

fn validate_regional_commit_decisions(
    decisions: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionReplayError> {
    for decision in decisions {
        inventory_recovery::validate_native_pickup_decision(decision)
            .map_err(|_| RegionalDecisionReplayError::InvalidDecision)?;
    }
    validate_regional_commit_decision_order(decisions)
}

fn validate_regional_commit_decision_order(
    decisions: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionReplayError> {
    for pair in decisions.windows(2) {
        let [previous, current] = pair else {
            unreachable!("two-item decision window");
        };
        if current.phase() <= previous.phase()
            || current.sequence_watermark() <= previous.sequence_watermark()
            || current.lifecycle_epoch() < previous.lifecycle_epoch()
        {
            return Err(RegionalDecisionReplayError::InvalidOrdering);
        }
    }
    Ok(())
}

fn replayed_entity_snapshot_is_semantically_valid(
    snapshot: &EntitySnapshot,
    runtime_validator: &mut EntityStore,
) -> bool {
    static ENTITY_TYPES: std::sync::OnceLock<mc_data::entity_types::EntityTypeRegistry> =
        std::sync::OnceLock::new();
    let entity_types =
        ENTITY_TYPES.get_or_init(mc_data::entity_types::solaris_required_entity_types);
    let known_type = mc_data::Identifier::parse(snapshot.type_name.clone())
        .ok()
        .is_some_and(|name| entity_types.id_of(&name).is_some());
    if !known_type {
        return false;
    }

    let mut runtime_snapshot = snapshot.clone();
    runtime_snapshot.vehicle = None;
    runtime_validator
        .insert_snapshot(runtime_snapshot)
        .unwrap_or(false)
        && runtime_validator.remove(snapshot.id).is_some()
}

pub fn replay_regional_commit_decisions(
    checkpoint: PersistedEntityCheckpoint,
    decisions: &[RegionalCommitDecision],
) -> Result<PersistedEntityCheckpoint, RegionalDecisionReplayError> {
    validate_regional_commit_decisions(decisions)?;
    validate_regional_decision_group(decisions)?;
    let PersistedEntityCheckpoint {
        lifecycle_clock,
        regional_sequence_watermark,
        mut allocation_high_water,
        records,
        mut generated_entity_claims,
    } = checkpoint;
    let mut entities = records
        .into_iter()
        .map(|record| (record.snapshot.id, record.snapshot))
        .collect::<BTreeMap<_, _>>();
    allocation_high_water =
        allocation_high_water.max(entities.keys().map(|id| id.0).max().unwrap_or(0));
    let checkpoint_boundary = (lifecycle_clock, regional_sequence_watermark);
    let mut replay_boundary = checkpoint_boundary;
    let mut runtime_validator = EntityStore::new();
    for decision in decisions {
        let predates_checkpoint = decision.lifecycle_epoch() < checkpoint_boundary.0
            || (decision.lifecycle_epoch() == checkpoint_boundary.0
                && decision.sequence_watermark() <= checkpoint_boundary.1);
        if predates_checkpoint {
            continue;
        }
        if decision.sequence_watermark() <= replay_boundary.1 {
            return Err(RegionalDecisionReplayError::InvalidOrdering);
        }
        for snapshot in decision.upserts() {
            allocation_high_water = allocation_high_water.max(snapshot.id.0);
            if !replayed_entity_snapshot_is_semantically_valid(snapshot, &mut runtime_validator) {
                return Err(RegionalDecisionReplayError::InvalidSnapshot);
            }
            if let Some(slot) = snapshot.retained.generated_spawn_slot {
                generated_entity_claims.insert(native_entities::consumed_slot_claim(slot));
            }
        }
        for entity in decision.removed() {
            allocation_high_water = allocation_high_water.max(entity.0);
            entities.remove(entity);
        }
        for snapshot in decision.upserts() {
            entities.insert(snapshot.id, snapshot.clone());
        }
        replay_boundary.0 = replay_boundary.0.max(decision.lifecycle_epoch());
        replay_boundary.1 = decision.sequence_watermark();
    }
    let restored = entities
        .into_values()
        .map(|snapshot| {
            PersistedEntityRecord::from_snapshot_at_lifecycle_clock(snapshot, replay_boundary.0)
        })
        .collect::<Vec<_>>();
    let mut uuids = std::collections::BTreeSet::new();
    for record in &restored {
        if !uuids.insert(record.snapshot.uuid) {
            return Err(RegionalDecisionReplayError::DuplicateEntityUuid(
                record.snapshot.uuid,
            ));
        }
    }
    Ok(PersistedEntityCheckpoint {
        lifecycle_clock: replay_boundary.0,
        regional_sequence_watermark: replay_boundary.1,
        allocation_high_water,
        records: restored,
        generated_entity_claims,
    })
}

impl RegionalDecisionJournal for FileRegionalDecisionJournal {
    fn record_commit(
        &mut self,
        decision: &RegionalCommitDecision,
    ) -> Result<(), RegionalDecisionJournalError> {
        self.record_commits(std::slice::from_ref(decision))
    }

    fn record_commits(
        &mut self,
        decisions: &[RegionalCommitDecision],
    ) -> Result<(), RegionalDecisionJournalError> {
        validate_regional_decision_group(decisions)
            .map_err(|_| RegionalDecisionJournalError::SAFE)?;
        validate_regional_commit_decisions(decisions)
            .map_err(|_| RegionalDecisionJournalError::SAFE)?;
        let previous_len = self.pending.len();
        self.pending.extend_from_slice(decisions);
        if validate_regional_commit_decision_order(&self.pending).is_err()
            || validate_regional_decision_group(&self.pending).is_err()
        {
            self.pending.truncate(previous_len);
            return Err(RegionalDecisionJournalError::SAFE);
        }
        // A failure observed before this request is sent cannot have committed
        // this decision. Preserve the original physical claim through rollback.
        if self.writer.failed.load(Ordering::Acquire) {
            self.pending.truncate(previous_len);
            return Err(RegionalDecisionJournalError::SAFE);
        }
        if self
            .writer
            .requests
            .send(world_journal::WriterRequest::EntityAppend {
                decisions: decisions.to_vec(),
            })
            .is_err()
        {
            self.pending.truncate(previous_len);
            return Err(RegionalDecisionJournalError::SAFE);
        }
        Ok(())
    }

    fn clear_commit(&mut self, phase: RegionPhase) -> Result<(), RegionalDecisionJournalError> {
        self.clear_commits(&[phase])
    }

    fn clear_commits(
        &mut self,
        phases: &[RegionPhase],
    ) -> Result<(), RegionalDecisionJournalError> {
        let phases = phases
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let identities = self
            .pending
            .iter()
            .filter(|decision| phases.contains(&decision.phase()))
            .map(RegionalCommitDecision::identity)
            .collect::<Vec<_>>();
        self.clear_commit_identities(&identities)
    }

    fn clear_commit_identities(
        &mut self,
        identities: &[(RegionPhase, u64, u64)],
    ) -> Result<(), RegionalDecisionJournalError> {
        let identities = identities
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        // A player save can fail independently of the entity checkpoint. Never
        // retire its native inventory participant until its canonical image (or
        // a newer checkpoint/projection) is durable in PlayerNBT.
        let root = self
            .path
            .parent()
            .and_then(Path::parent)
            .ok_or(RegionalDecisionJournalError::SAFE)?;
        inventory_recovery::recover_native_pickup_inventory_iter(
            root,
            self.pending
                .iter()
                .filter(|decision| identities.contains(&decision.identity())),
        )
        .map_err(|_| RegionalDecisionJournalError::SAFE)?;
        let previous_len = self.pending.len();
        self.pending
            .retain(|decision| !identities.contains(&decision.identity()));
        if self.pending.len() != previous_len {
            self.needs_compaction = true;
        }
        Ok(())
    }

    fn pending_phases(&self) -> Vec<RegionPhase> {
        self.pending
            .iter()
            .map(RegionalCommitDecision::phase)
            .collect()
    }

    fn pending_commit_identities(&self) -> Vec<(RegionPhase, u64, u64)> {
        self.pending
            .iter()
            .map(RegionalCommitDecision::identity)
            .collect()
    }

    fn recovery_watermark(&self) -> (RegionPhase, u64) {
        self.pending
            .iter()
            .fold((RegionPhase(0), 0), |(phase, sequence), decision| {
                (
                    phase.max(decision.phase()),
                    sequence.max(decision.sequence_watermark()),
                )
            })
    }
}

#[cfg(unix)]
fn sync_regional_journal_directory(path: &Path) -> Result<(), RegionalDecisionJournalOpenError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_regional_journal_directory(_path: &Path) -> Result<(), RegionalDecisionJournalOpenError> {
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldPersistedMetadata {
    pub world_time: u64,
    pub daylight_cycle_enabled: bool,
    pub weather: crate::weather::WeatherState,
    pub players_sleeping_percentage: u32,
    pub keep_inventory: bool,
    pub command_rules: std::collections::BTreeMap<String, String>,
    pub weather_duration_ticks: u64,
    pub world_identity: String,
}

#[derive(Debug, Clone)]
pub struct PersistedEntityRecord {
    pub snapshot: EntitySnapshot,
    pub age: i32,
    pub pickup_delay: i32,
}

impl PersistedEntityRecord {
    pub fn from_snapshot_at_lifecycle_clock(
        snapshot: EntitySnapshot,
        lifecycle_clock: u64,
    ) -> Self {
        let age = lifecycle_clock
            .saturating_sub(snapshot.retained.spawn_tick)
            .min(i32::MAX as u64) as i32;
        let pickup_delay = snapshot
            .retained
            .item_pickup_ready_tick
            .map(|ready_tick| ready_tick.saturating_sub(lifecycle_clock))
            .unwrap_or(0)
            .min(i32::from(i16::MAX) as u64) as i32;
        Self {
            snapshot,
            age,
            pickup_delay,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PersistedEntityCheckpoint {
    pub lifecycle_clock: u64,
    pub regional_sequence_watermark: u64,
    /// Highest id allocated by the existing owner, including removed entities.
    pub allocation_high_water: i32,
    pub records: Vec<PersistedEntityRecord>,
    pub generated_entity_claims: BTreeSet<String>,
}

impl PersistedEntityCheckpoint {
    #[cfg(test)]
    pub(crate) fn new(
        lifecycle_clock: u64,
        records: impl IntoIterator<Item = impl Into<PersistedEntityRecord>>,
    ) -> Self {
        Self {
            lifecycle_clock,
            regional_sequence_watermark: 0,
            allocation_high_water: 0,
            records: records.into_iter().map(Into::into).collect(),
            generated_entity_claims: BTreeSet::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_at_owner_sequence(
        lifecycle_clock: u64,
        regional_sequence_watermark: u64,
        records: impl IntoIterator<Item = impl Into<PersistedEntityRecord>>,
    ) -> Self {
        Self {
            lifecycle_clock,
            regional_sequence_watermark,
            allocation_high_water: 0,
            records: records.into_iter().map(Into::into).collect(),
            generated_entity_claims: BTreeSet::new(),
        }
    }

    pub fn has_valid_temporal_state(&self) -> bool {
        self.lifecycle_clock <= i64::MAX as u64
            && self.allocation_high_water >= 0
            && self.records.iter().all(|record| {
                let Ok(age) = u64::try_from(record.age) else {
                    return false;
                };
                let Ok(pickup_delay) = u64::try_from(record.pickup_delay) else {
                    return false;
                };
                age <= self.lifecycle_clock
                    && record.snapshot.retained.spawn_tick <= self.lifecycle_clock
                    && self
                        .lifecycle_clock
                        .saturating_sub(record.snapshot.retained.spawn_tick)
                        == age
                    && record
                        .snapshot
                        .retained
                        .item_pickup_ready_tick
                        .map(|ready_tick| ready_tick.saturating_sub(self.lifecycle_clock))
                        .unwrap_or(0)
                        == pickup_delay
                    && self
                        .lifecycle_clock
                        .checked_add(pickup_delay)
                        .is_some_and(|deadline| deadline <= i64::MAX as u64)
                    && entity_temporal_state_is_valid(self.lifecycle_clock, &record.snapshot)
            })
    }

    fn has_valid_vehicle_graph(&self) -> bool {
        let entities = self
            .records
            .iter()
            .map(|record| (record.snapshot.id, &record.snapshot))
            .collect::<BTreeMap<_, _>>();
        if entities.len() != self.records.len() {
            return false;
        }
        let mut vehicle_passengers = BTreeMap::new();
        let mut passenger_vehicles = BTreeMap::new();
        for snapshot in entities.values() {
            let Some(passenger) = snapshot.vehicle.and_then(|vehicle| vehicle.passenger) else {
                continue;
            };
            let Some(passenger_snapshot) = entities.get(&passenger) else {
                return false;
            };
            if passenger == snapshot.id
                || mc_entity::RegionKey::from_position(passenger_snapshot.position)
                    != mc_entity::RegionKey::from_position(snapshot.position)
                || vehicle_passengers.insert(snapshot.id, passenger).is_some()
                || passenger_vehicles.insert(passenger, snapshot.id).is_some()
            {
                return false;
            }
        }
        for &start in entities.keys() {
            let mut current = start;
            let mut visited = std::collections::BTreeSet::new();
            while let Some(&passenger) = vehicle_passengers.get(&current) {
                if !visited.insert(current) {
                    return false;
                }
                current = passenger;
            }
        }
        true
    }
}

impl From<EntitySnapshot> for PersistedEntityRecord {
    fn from(snapshot: EntitySnapshot) -> Self {
        Self {
            snapshot,
            age: 0,
            pickup_delay: 0,
        }
    }
}

impl std::ops::Deref for PersistedEntityRecord {
    type Target = EntitySnapshot;

    fn deref(&self) -> &Self::Target {
        &self.snapshot
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct XpState {
    pub level: i32,
    pub progress: f32,
    pub total: i32,
    pub seed: i32,
}

impl Default for XpState {
    fn default() -> Self {
        Self {
            level: 0,
            progress: 0.0,
            total: 0,
            seed: 0,
        }
    }
}

impl XpState {
    pub fn reset(&mut self) -> bool {
        let changed = self.level != 0 || self.progress != 0.0 || self.total != 0;
        self.level = 0;
        self.progress = 0.0;
        self.total = 0;
        changed
    }

    pub fn add_points(&mut self, points: i32) -> bool {
        if points <= 0 {
            return false;
        }
        self.total = self.total.saturating_add(points).max(0);
        self.progress += points as f32 / Self::points_to_next_level(self.level) as f32;
        while self.progress >= 1.0 {
            let points_above_level =
                (self.progress - 1.0) * Self::points_to_next_level(self.level) as f32;
            self.level = self.level.saturating_add(1);
            self.progress = points_above_level / Self::points_to_next_level(self.level) as f32;
        }
        true
    }

    pub fn spend_enchantment_levels(&mut self, levels: i32, next_seed: i32) -> bool {
        if levels <= 0 || self.level < levels {
            return false;
        }
        self.level -= levels;
        self.seed = next_seed;
        true
    }

    fn points_to_next_level(level: i32) -> i32 {
        if level >= 30 {
            112_i32.saturating_add(level.saturating_sub(30).saturating_mul(9))
        } else if level >= 15 {
            37_i32.saturating_add(level.saturating_sub(15).saturating_mul(5))
        } else {
            7_i32.saturating_add(level.max(0).saturating_mul(2))
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpawnState {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub angle: f32,
}

impl SpawnState {
    pub fn from_pose(pose: PersistedPlayerPose) -> Self {
        Self {
            x: pose.x.floor() as i32,
            y: pose.y.floor() as i32,
            z: pose.z.floor() as i32,
            angle: pose.yaw,
        }
    }

    pub fn pose(&self) -> PersistedPlayerPose {
        let mut pose = PersistedPlayerPose::new(
            f64::from(self.x) + 0.5,
            f64::from(self.y),
            f64::from(self.z) + 0.5,
        );
        pose.yaw = self.angle;
        pose
    }
}

/// The player pose fields playerdata persists (`Pos`, `Rotation`, `OnGround`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PersistedPlayerPose {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl PersistedPlayerPose {
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct InventorySlotExtras {
    item_id: u32,
    damage: Option<i32>,
    enchantments: Vec<mc_data::ItemEnchantment>,
    custom_name: Option<String>,
    item_model: Option<Arc<Identifier>>,
    fields: Vec<(String, Tag)>,
}

/// Unmodelled per-slot playerdata item payloads and the native pickup watermark.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInventoryExtras {
    slots: [Option<InventorySlotExtras>; 46],
    native_pickup_revision: u64,
}

impl Default for PlayerInventoryExtras {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            native_pickup_revision: 0,
        }
    }
}

impl PlayerInventoryExtras {
    pub fn native_pickup_revision(&self) -> u64 {
        self.native_pickup_revision
    }

    pub fn set_native_pickup_revision(&mut self, revision: u64) {
        self.native_pickup_revision = revision;
    }
}

/// Pure persisted image of one player's playerdata file.
#[derive(Debug, Clone)]
pub struct PlayerPersistedRecord {
    pub pose: PersistedPlayerPose,
    pub game_mode: GameMode,
    pub survival: SurvivalState,
    pub effects: Option<mc_entity::EntityActiveEffectsState>,
    pub physical_stamina: Option<crate::combat::PhysicalStamina>,
    pub physical_hurt_resistance: Option<crate::combat::PlayerHurtResistance>,
    pub physical_shield_disabled_until: Option<u64>,
    pub inventory: PlayerInventory,
    pub inventory_operation_revision: u64,
    pub carried_item: ItemStack,
    pub crafting_table_input: Option<Box<[ItemStack; 9]>>,
    pub enchanting_table_input: Option<Box<[ItemStack; 2]>>,
    pub merchant_input: Option<Box<[ItemStack; 2]>>,
    pub selected_hotbar_slot: u8,
    pub spawn: SpawnState,
    pub xp: XpState,
    pub inventory_extras: PlayerInventoryExtras,
}

impl PlayerPersistedRecord {
    fn owned_inventory(&self) -> PlayerOwnedInventory<'_> {
        PlayerOwnedInventory {
            inventory: &self.inventory,
            inventory_extras: &self.inventory_extras,
            carried_item: &self.carried_item,
            crafting_table_input: self.crafting_table_input.as_deref(),
            enchanting_table_input: self.enchanting_table_input.as_deref(),
            merchant_input: self.merchant_input.as_deref(),
        }
    }
}

/// A loaded record plus whether playerdata supplied its pose/spawn;
/// otherwise the default record's values were kept.
#[derive(Debug, Clone)]
pub struct LoadedPlayerRecord {
    pub record: PlayerPersistedRecord,
    pub pose_loaded: bool,
    pub spawn_loaded: bool,
}

#[derive(Debug, Error)]
pub enum PlayerPersistenceError {
    #[error("persistence I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("persistence NBT at {path}: {source}")]
    Nbt {
        path: PathBuf,
        source: mc_nbt::NbtError,
    },
    #[error("persistence {kind} at {path} is at least {bytes} bytes, exceeding limit {max}")]
    DataTooLarge {
        path: PathBuf,
        kind: &'static str,
        bytes: usize,
        max: usize,
    },
    #[error("persistence {kind} at {path} has {bytes} trailing byte(s)")]
    TrailingData {
        path: PathBuf,
        kind: &'static str,
        bytes: usize,
    },
    #[error("persistence root at {path} is not a compound")]
    RootNotCompound { path: PathBuf },
    #[error("playerdata item id is invalid: {0}")]
    InvalidItemId(String),
    #[error("persistence item component is invalid: {0}")]
    InvalidItemComponent(&'static str),
    #[error("playerdata item is not in registry: {0}")]
    UnknownItem(String),
    #[error("playerdata enchantment is invalid or not in registry: {0}")]
    InvalidEnchantment(String),
    #[error("persistence numeric field {field} at {path} is not finite")]
    InvalidNumeric { path: PathBuf, field: &'static str },
    #[error("persistence field {field} at {path} has an invalid value")]
    InvalidValue { path: PathBuf, field: &'static str },
    #[error("duplicate entity UUID {uuid} at {path}")]
    DuplicateEntityUuid { path: PathBuf, uuid: uuid::Uuid },
    #[error("unsupported entity persistence format version {version} at {path}")]
    UnsupportedEntityFormatVersion { path: PathBuf, version: i32 },
}

fn validate_player_numeric_state(
    path: &Path,
    state: &PlayerPersistedRecord,
) -> Result<(), PlayerPersistenceError> {
    if !state.pose.x.is_finite() || !state.pose.y.is_finite() || !state.pose.z.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Pos",
        });
    }
    if !state.pose.yaw.is_finite() || !state.pose.pitch.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Rotation",
        });
    }
    if !state.survival.health.is_finite()
        || !state.survival.saturation.is_finite()
        || !state.survival.exhaustion.is_finite()
    {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "survival",
        });
    }
    if !state.spawn.angle.is_finite() || !state.xp.progress.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "spawn/xp",
        });
    }
    Ok(())
}

fn validate_entity_numeric_state(
    path: &Path,
    entity: &EntitySnapshot,
) -> Result<(), PlayerPersistenceError> {
    if !entity.position.x.is_finite()
        || !entity.position.y.is_finite()
        || !entity.position.z.is_finite()
        || !entity.velocity.x.is_finite()
        || !entity.velocity.y.is_finite()
        || !entity.velocity.z.is_finite()
        || !entity.rotation.yaw.is_finite()
        || !entity.rotation.pitch.is_finite()
        || !entity.rotation.head_yaw.is_finite()
        || !entity.health.is_finite()
        || !entity.retained.fall_distance.is_finite()
        || entity.retained.fall_distance < 0.0
    {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "entity Pos/Motion/Rotation/Health/FallDistance",
        });
    }
    Ok(())
}

fn normalize_loaded_entity_kinematics(
    path: &Path,
    position: &mut Vec3,
    rotation: mc_entity::Rotation,
    velocity: &mut Vec3,
) -> Result<(), PlayerPersistenceError> {
    if !position.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Pos",
        });
    }
    if !rotation.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Rotation",
        });
    }
    if !velocity.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Motion",
        });
    }

    position.x = position.x.clamp(
        -ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2,
        ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2,
    );
    position.y = position.y.clamp(
        -ENTITY_VERTICAL_POSITION_LIMIT_26_1_2,
        ENTITY_VERTICAL_POSITION_LIMIT_26_1_2,
    );
    position.z = position.z.clamp(
        -ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2,
        ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2,
    );
    for component in [&mut velocity.x, &mut velocity.y, &mut velocity.z] {
        if component.abs() > ENTITY_VELOCITY_LIMIT_26_1_2 {
            *component = 0.0;
        }
        *component *= ENTITY_TICKS_PER_SECOND;
    }
    Ok(())
}

pub fn load_player_state(
    world_root: &Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    default: PlayerPersistedRecord,
) -> Result<Option<LoadedPlayerRecord>, PlayerPersistenceError> {
    let path = playerdata_path(world_root, uuid);
    if !path.is_file() {
        return Ok(None);
    }

    let (root_name, root) = read_player_root(&path)?;
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    let _ = root_name;

    let mut state = default;
    state.inventory_operation_revision = inventory_recovery::operation_revision(&fields, &path)?;
    state.physical_stamina =
        match field(&fields, "solaris:physical_stamina") {
            None => None,
            Some(Tag::String(value)) => Some(serde_json::from_str(value).map_err(|_| {
                PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: "solaris:physical_stamina",
                }
            })?),
            Some(_) => {
                return Err(PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: "solaris:physical_stamina",
                });
            }
        };
    state.physical_hurt_resistance =
        match field(&fields, "solaris:physical_hurt_resistance") {
            None => None,
            Some(Tag::String(value)) => Some(serde_json::from_str(value).map_err(|_| {
                PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: "solaris:physical_hurt_resistance",
                }
            })?),
            Some(_) => {
                return Err(PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: "solaris:physical_hurt_resistance",
                });
            }
        };
    state.physical_shield_disabled_until =
        match field(&fields, "solaris:physical_shield_disabled_until") {
            None => None,
            Some(Tag::Long(value)) => Some(*value as u64),
            Some(_) => {
                return Err(PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: "solaris:physical_shield_disabled_until",
                });
            }
        };
    state.inventory_extras.native_pickup_revision =
        inventory_recovery::native_pickup_revision(&fields, &path)?;
    let pose = read_pose(&fields);
    let pose_loaded = pose.is_some();
    if let Some(pose) = pose {
        state.pose = pose;
    }
    if let Some(game_mode) = int_field(&fields, "playerGameType") {
        state.game_mode = GameMode::from_id(game_mode);
    }
    if let Some(health) = float_field(&fields, "Health") {
        state.survival.health = health.clamp(0.0, mc_entity::player_survival_26_1_2::MAX_HEALTH);
    }
    state.survival.absorption = float_field(&fields, "AbsorptionAmount")
        .unwrap_or(0.0)
        .max(0.0);
    state.effects = field(&fields, "active_effects")
        .map(|tag| {
            let snapshot = effects::load(tag)?;
            if let Some(snapshot) = &snapshot {
                crate::player_effects::player_effects_from_snapshot(snapshot)?;
            }
            Ok::<_, &'static str>(snapshot)
        })
        .transpose()
        .map_err(|_| PlayerPersistenceError::InvalidValue {
            path: path.clone(),
            field: "active_effects",
        })?
        .flatten();
    if let Some(food) = int_field(&fields, "foodLevel") {
        state.survival.food = food.clamp(0, mc_entity::player_survival_26_1_2::MAX_FOOD);
    }
    if let Some(saturation) = float_field(&fields, "foodSaturationLevel") {
        state.survival.saturation = saturation.max(0.0);
    }
    if let Some(exhaustion) = float_field(&fields, "foodExhaustionLevel") {
        state.survival.exhaustion = exhaustion.max(0.0);
    }
    if let Some(fire) = int_field(&fields, "Fire") {
        state.survival.remaining_fire_ticks = fire;
    }
    if let Some(slot) = int_field(&fields, "SelectedItemSlot") {
        state.selected_hotbar_slot = slot.clamp(0, 8) as u8;
    }
    let spawn_loaded = read_spawn(&fields).is_some();
    if let Some(spawn) = read_spawn(&fields) {
        state.spawn = spawn;
    }
    state.xp.level = int_field(&fields, "XpLevel")
        .unwrap_or(state.xp.level)
        .max(0);
    state.xp.progress = float_field(&fields, "XpP")
        .unwrap_or(state.xp.progress)
        .clamp(0.0, 1.0);
    state.xp.total = int_field(&fields, "XpTotal")
        .unwrap_or(state.xp.total)
        .max(0);
    state.xp.seed = int_field(&fields, "XpSeed").unwrap_or(state.xp.seed);

    if let Some(Tag::List(list)) = field(&fields, "Inventory") {
        for element in &list.elements {
            let Tag::Compound(item_fields) = element else {
                continue;
            };
            let Some(slot) = slot_field(item_fields) else {
                continue;
            };
            if slot >= state.inventory.slots.len() {
                continue;
            }
            let Some(stack) = item_stack_from_fields(items, item_fields)? else {
                continue;
            };
            state.inventory.slots[slot] = stack.clone();
            let extras = item_fields
                .iter()
                .filter(|(name, _)| !is_modelled_item_key(name))
                .cloned()
                .collect::<Vec<_>>();
            if !extras.is_empty() {
                state.inventory_extras.slots[slot] = Some(InventorySlotExtras {
                    item_id: stack.item_id,
                    damage: stack.damage,
                    enchantments: stack.enchantments,
                    custom_name: stack.custom_name.map(|name| name.into_string()),
                    item_model: stack.item_model,
                    fields: extras,
                });
            }
        }
    }
    if let Some(Tag::Compound(item_fields)) = field(&fields, CARRIED_ITEM_FIELD) {
        state.carried_item = item_stack_from_fields(items, item_fields)?.unwrap_or_default();
    }
    state.crafting_table_input =
        item_stack_projection_from_field::<9>(items, &fields, CRAFTING_TABLE_INPUT_FIELD)?;
    state.enchanting_table_input =
        item_stack_projection_from_field::<2>(items, &fields, ENCHANTING_TABLE_INPUT_FIELD)?;
    state.merchant_input =
        item_stack_projection_from_field::<2>(items, &fields, MERCHANT_INPUT_FIELD)?;

    validate_player_numeric_state(&path, &state)?;
    Ok(Some(LoadedPlayerRecord {
        record: state,
        pose_loaded,
        spawn_loaded,
    }))
}

pub fn save_player_state(
    world_root: &Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    state: &PlayerPersistedRecord,
) -> Result<(), PlayerPersistenceError> {
    let path = playerdata_path(world_root, uuid);
    validate_player_numeric_state(&path, state)?;
    let (root_name, root) = if path.is_file() {
        read_player_root(&path)?
    } else {
        (String::new(), Tag::Compound(Vec::new()))
    };
    let Tag::Compound(mut fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };

    set_field(&mut fields, "Pos", pose_position_tag(state.pose));
    set_field(&mut fields, "Rotation", pose_rotation_tag(state.pose));
    set_field(
        &mut fields,
        "OnGround",
        Tag::Byte(i8::from(state.pose.on_ground)),
    );
    set_field(
        &mut fields,
        "playerGameType",
        Tag::Int(state.game_mode.id()),
    );
    set_field(&mut fields, "Health", Tag::Float(state.survival.health));
    if let Some(stamina) = state.physical_stamina {
        let value =
            serde_json::to_string(&stamina).map_err(|_| PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: "solaris:physical_stamina",
            })?;
        set_field(&mut fields, "solaris:physical_stamina", Tag::String(value));
    } else {
        fields.retain(|(name, _)| name != "solaris:physical_stamina");
    }
    if let Some(resistance) = state.physical_hurt_resistance {
        let value = serde_json::to_string(&resistance).map_err(|_| {
            PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: "solaris:physical_hurt_resistance",
            }
        })?;
        set_field(
            &mut fields,
            "solaris:physical_hurt_resistance",
            Tag::String(value),
        );
    } else {
        fields.retain(|(name, _)| name != "solaris:physical_hurt_resistance");
    }
    if let Some(deadline) = state.physical_shield_disabled_until {
        set_field(
            &mut fields,
            "solaris:physical_shield_disabled_until",
            Tag::Long(deadline as i64),
        );
    } else {
        fields.retain(|(name, _)| name != "solaris:physical_shield_disabled_until");
    }
    set_field(
        &mut fields,
        "AbsorptionAmount",
        Tag::Float(state.survival.absorption),
    );
    set_field(
        &mut fields,
        "active_effects",
        effects::save(state.effects.as_ref()).map_err(|_| {
            PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: "active_effects",
            }
        })?,
    );
    set_field(&mut fields, "foodLevel", Tag::Int(state.survival.food));
    set_field(
        &mut fields,
        "foodSaturationLevel",
        Tag::Float(state.survival.saturation),
    );
    set_field(
        &mut fields,
        "foodExhaustionLevel",
        Tag::Float(state.survival.exhaustion),
    );
    set_field(
        &mut fields,
        "Fire",
        Tag::Short(
            state
                .survival
                .remaining_fire_ticks
                .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        ),
    );
    set_field(
        &mut fields,
        "SelectedItemSlot",
        Tag::Int(i32::from(state.selected_hotbar_slot)),
    );
    set_field(&mut fields, "SpawnX", Tag::Int(state.spawn.x));
    set_field(&mut fields, "SpawnY", Tag::Int(state.spawn.y));
    set_field(&mut fields, "SpawnZ", Tag::Int(state.spawn.z));
    set_field(&mut fields, "SpawnAngle", Tag::Float(state.spawn.angle));
    set_field(&mut fields, "XpLevel", Tag::Int(state.xp.level));
    set_field(&mut fields, "XpP", Tag::Float(state.xp.progress));
    set_field(&mut fields, "XpTotal", Tag::Int(state.xp.total));
    set_field(&mut fields, "XpSeed", Tag::Int(state.xp.seed));
    if inventory_recovery::operation_revision(&fields, &path)? <= state.inventory_operation_revision
        && inventory_recovery::native_pickup_revision(&fields, &path)?
            <= state.inventory_extras.native_pickup_revision()
    {
        for (name, value) in
            inventory_recovery::snapshot_owned_fields(items, state.owned_inventory())?
        {
            set_field(&mut fields, name, value);
        }
        set_field(
            &mut fields,
            INVENTORY_OPERATION_REVISION_FIELD,
            Tag::Long(
                i64::try_from(state.inventory_operation_revision).map_err(|_| {
                    PlayerPersistenceError::InvalidValue {
                        path: path.clone(),
                        field: INVENTORY_OPERATION_REVISION_FIELD,
                    }
                })?,
            ),
        );
        inventory_recovery::set_native_pickup_revision(
            &mut fields,
            &path,
            state.inventory_extras.native_pickup_revision(),
        )?;
    }

    write_player_root(&path, &root_name, &Tag::Compound(fields))
}

fn migrate_legacy_villager_gossip(
    retained: &mut mc_entity::EntityRetainedState,
) -> Result<(), mc_entity::villager_gossip_26_1_2::VillagerGossipError> {
    let legacy_gossip = retained
        .villager_merchant
        .as_mut()
        .map(mc_entity::villager_merchant_26_1_2::VillagerMerchantState::take_legacy_gossip)
        .transpose()?
        .flatten();
    let Some(legacy_gossip) = legacy_gossip else {
        return Ok(());
    };
    if let Some(gossip) = retained.villager_gossip.as_mut() {
        gossip.merge_legacy_trading(
            Some(legacy_gossip.last_decay_game_time),
            legacy_gossip
                .player_gossips
                .into_iter()
                .map(|entry| (entry.player, entry.trading)),
        )?;
    } else {
        retained.villager_gossip = Some(legacy_gossip);
    }
    Ok(())
}

pub fn load_persisted_entities(
    world_root: &Path,
    items: &ItemRegistry,
    entity_types: &EntityTypeRegistry,
) -> Result<PersistedEntityCheckpoint, PlayerPersistenceError> {
    let path = entities_path(world_root);
    if !path.is_file() {
        return Ok(PersistedEntityCheckpoint {
            lifecycle_clock: 0,
            regional_sequence_watermark: 0,
            allocation_high_water: 0,
            records: Vec::new(),
            generated_entity_claims: BTreeSet::new(),
        });
    }
    let (_, root) = read_persistence_root(&path, NamedRootPolicy::EntityDocument)?;
    decode_entity_checkpoint(path, root, items, entity_types)
}

fn decode_entity_checkpoint(
    path: PathBuf,
    root: Tag,
    items: &ItemRegistry,
    entity_types: &EntityTypeRegistry,
) -> Result<PersistedEntityCheckpoint, PlayerPersistenceError> {
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    let mut format_versions = fields
        .iter()
        .filter(|(name, _)| name == ENTITY_FORMAT_VERSION_FIELD);
    let format_version = format_versions.next().map(|(_, value)| value);
    if format_versions.next().is_some() {
        return Err(PlayerPersistenceError::InvalidValue {
            path,
            field: ENTITY_FORMAT_VERSION_FIELD,
        });
    }
    match format_version {
        None => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_FORMAT_VERSION_FIELD,
            });
        }
        Some(Tag::Int(ENTITY_FORMAT_VERSION)) => {}
        Some(Tag::Int(version)) => {
            return Err(PlayerPersistenceError::UnsupportedEntityFormatVersion {
                path,
                version: *version,
            });
        }
        Some(_) => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_FORMAT_VERSION_FIELD,
            });
        }
    }
    let mut lifecycle_ticks = fields
        .iter()
        .filter(|(name, _)| name == ENTITY_LIFECYCLE_TICK_FIELD);
    let lifecycle_tick = match lifecycle_ticks.next().map(|(_, value)| value) {
        Some(Tag::Long(value)) if *value >= 0 && lifecycle_ticks.next().is_none() => *value as u64,
        _ => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_LIFECYCLE_TICK_FIELD,
            });
        }
    };
    let mut regional_sequences = fields
        .iter()
        .filter(|(name, _)| name == ENTITY_REGIONAL_SEQUENCE_FIELD);
    let regional_sequence_watermark = match regional_sequences.next().map(|(_, value)| value) {
        Some(Tag::Long(value)) if *value >= 0 && regional_sequences.next().is_none() => {
            *value as u64
        }
        _ => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_REGIONAL_SEQUENCE_FIELD,
            });
        }
    };
    let mut allocation_values = fields
        .iter()
        .filter(|(name, _)| name == ENTITY_ALLOCATION_HIGH_WATER_FIELD);
    let allocation_high_water = match allocation_values.next().map(|(_, value)| value) {
        None => 0, // Existing Anvil checkpoints predate this owner metadata.
        Some(Tag::Int(value)) if *value >= 0 && allocation_values.next().is_none() => *value,
        _ => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_ALLOCATION_HIGH_WATER_FIELD,
            });
        }
    };
    let Some(Tag::List(list)) = field(&fields, "Entities") else {
        return Err(PlayerPersistenceError::InvalidValue {
            path,
            field: "Entities",
        });
    };
    let generated_entity_claims = match field(&fields, ENTITY_GENERATED_ENTITY_CLAIMS_FIELD) {
        None => BTreeSet::new(),
        Some(Tag::List(list)) => list
            .elements
            .iter()
            .map(|value| match value {
                Tag::String(claim) if !claim.is_empty() => Ok(claim.clone()),
                _ => Err(PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_GENERATED_ENTITY_CLAIMS_FIELD,
                }),
            })
            .collect::<Result<BTreeSet<_>, _>>()?,
        Some(_) => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_GENERATED_ENTITY_CLAIMS_FIELD,
            });
        }
    };
    let mut entities = Vec::new();
    let mut entity_uuids = std::collections::BTreeSet::new();
    for element in &list.elements {
        let Tag::Compound(fields) = element else {
            continue;
        };
        let Some(type_name) = string_field(fields, "id") else {
            continue;
        };
        let parsed = mc_data::Identifier::parse(type_name.to_string())
            .map_err(|_| PlayerPersistenceError::InvalidItemId(type_name.to_string()))?;
        let type_id = entity_types
            .id_of(&parsed)
            .ok_or_else(|| PlayerPersistenceError::UnknownItem(type_name.to_string()))?
            as i32;
        let pos = double_list::<3>(
            field(fields, "Pos").unwrap_or(&Tag::List(ListTag::empty())),
            3,
        )
        .unwrap_or([0.0, 0.0, 0.0]);
        let motion = double_list::<3>(
            field(fields, "Motion").unwrap_or(&Tag::List(ListTag::empty())),
            3,
        )
        .unwrap_or([0.0, 0.0, 0.0]);
        let rotation = float_list::<2>(
            field(fields, "Rotation").unwrap_or(&Tag::List(ListTag::empty())),
            2,
        )
        .unwrap_or([0.0, 0.0]);
        let living = persisted_entity_type_is_living(type_name);
        let item_stack = if living {
            None
        } else if let Some(Tag::Compound(item)) = field(fields, "Item") {
            read_entity_item_stack(item, items)?
        } else {
            None
        };
        let experience_value = int_field(fields, "Value").filter(|value| *value > 0);
        let block_state =
            int_field(fields, "BlockState").and_then(|value| u32::try_from(value).ok());
        let health = float_field(fields, "Health").unwrap_or(20.0).max(0.0);
        let attributes = string_field(fields, ENTITY_ATTRIBUTES_FIELD)
            .ok_or_else(|| PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_ATTRIBUTES_FIELD,
            })
            .and_then(|encoded| {
                serde_json::from_str(encoded).map_err(|_| PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_ATTRIBUTES_FIELD,
                })
            })?;
        let id = EntityId(int_field(fields, "SolarisEntityId").unwrap_or(0).max(0));
        let uuid = uuid_field(fields).unwrap_or_else(|| {
            let id = int_field(fields, "SolarisEntityId").unwrap_or(0) as u32 as u128;
            uuid::Uuid::from_u128(0x5f1a_0000_0000_0000_0000_0000_0000_0000 | id)
        });
        let aquatic = persisted_entity_type_is_aquatic(type_name);
        let has_lifetime_age = field(fields, "SolarisLifetimeAge").is_some();
        let animal = persisted_entity_type_is_supported_breeding_animal(type_name).then(|| {
            mc_entity::AnimalBreedingState {
                age_ticks: if has_lifetime_age {
                    int_field(fields, "Age").unwrap_or(0)
                } else {
                    0
                },
                love_ticks: int_field(fields, "InLove")
                    .unwrap_or(0)
                    .clamp(0, i32::from(u16::MAX)) as u16,
                sheep_wool: (type_name == "minecraft:sheep").then(|| {
                    let color = int_field(fields, "Color")
                        .and_then(|value| u8::try_from(value).ok())
                        .and_then(mc_entity::SheepColor::from_id)
                        .unwrap_or_default();
                    mc_entity::SheepWoolState {
                        color,
                        sheared: byte_field(fields, "Sheared").is_some_and(|value| value != 0),
                    }
                }),
            }
        });
        let age = int_field(fields, "SolarisLifetimeAge")
            .or_else(|| animal.is_none().then(|| int_field(fields, "Age")).flatten())
            .unwrap_or(0);
        let pickup_delay = int_field(fields, "PickupDelay").unwrap_or(0);
        if age < 0
            || age as u64 > lifecycle_tick
            || pickup_delay < 0
            || lifecycle_tick
                .checked_add(pickup_delay as u64)
                .is_none_or(|deadline| deadline > i64::MAX as u64)
        {
            return Err(PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_LIFECYCLE_TICK_FIELD,
            });
        }
        let lifecycle = match byte_field(fields, ENTITY_LIFECYCLE_FIELD) {
            Some(0) => EntityLifecycle::Alive,
            Some(1) => EntityLifecycle::Despawning,
            _ => {
                return Err(PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_LIFECYCLE_FIELD,
                });
            }
        };
        let mut retained: mc_entity::EntityRetainedState =
            string_field(fields, ENTITY_RETAINED_STATE_FIELD)
                .ok_or_else(|| PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_RETAINED_STATE_FIELD,
                })
                .and_then(|encoded| {
                    serde_json::from_str(encoded).map_err(|_| {
                        PlayerPersistenceError::InvalidValue {
                            path: path.clone(),
                            field: ENTITY_RETAINED_STATE_FIELD,
                        }
                    })
                })?;
        if living {
            // The compound is the canonical record of initialized gear, empty or not.
            // Its absence is an uninitialized hand: the spawn's native loadout still
            // applies, exactly as for a checkpoint written before gear was tracked.
            retained.equipment = match field(fields, "equipment") {
                Some(Tag::Compound(equipment)) => {
                    let mainhand = match field(equipment, "mainhand") {
                        Some(Tag::Compound(stack)) => read_entity_item_stack(stack, items)?,
                        None => None,
                        Some(_) => {
                            return Err(PlayerPersistenceError::InvalidValue {
                                path: path.clone(),
                                field: "equipment",
                            });
                        }
                    };
                    Some(EntityMainHandEquipment { mainhand })
                }
                None => None,
                Some(_) => {
                    return Err(PlayerPersistenceError::InvalidValue {
                        path: path.clone(),
                        field: "equipment",
                    });
                }
            };
        }
        migrate_legacy_villager_gossip(&mut retained).map_err(|_| {
            PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_RETAINED_STATE_FIELD,
            }
        })?;
        if let Some(value) = byte_field(fields, "NoAI") {
            retained.no_ai = value != 0;
        }
        if let Some(value) = byte_field(fields, "Invulnerable") {
            retained.invulnerable = value != 0;
        }
        read_animal_interaction_tags(fields, items, &mut retained.animal_interaction)?;
        if let Some(fall_distance) = float_field(fields, "FallDistance") {
            retained.fall_distance = f64::from(fall_distance);
        }
        if let Some(fire) = int_field(fields, "Fire") {
            retained.remaining_fire_ticks = fire;
        }
        let head_yaw = float_field(fields, ENTITY_HEAD_YAW_FIELD).ok_or_else(|| {
            PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_HEAD_YAW_FIELD,
            }
        })?;
        let goal = string_field(fields, ENTITY_GOAL_STATE_FIELD)
            .ok_or_else(|| PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_GOAL_STATE_FIELD,
            })
            .and_then(|encoded| {
                serde_json::from_str(encoded).map_err(|_| PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_GOAL_STATE_FIELD,
                })
            })?;
        let vehicle = string_field(fields, ENTITY_VEHICLE_STATE_FIELD)
            .ok_or_else(|| PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_VEHICLE_STATE_FIELD,
            })
            .and_then(|encoded| {
                serde_json::from_str(encoded).map_err(|_| PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_VEHICLE_STATE_FIELD,
                })
            })?;
        let mut snapshot = EntitySnapshot {
            id,
            uuid,
            type_id,
            type_name: type_name.to_string(),
            position: Vec3::new(pos[0], pos[1], pos[2]),
            rotation: mc_entity::Rotation {
                yaw: rotation[0],
                pitch: rotation[1],
                head_yaw,
            },
            velocity: Vec3::new(motion[0], motion[1], motion[2]),
            on_ground: byte_field(fields, "OnGround").unwrap_or(0) != 0 && !aquatic,
            item_stack,
            experience_value,
            block_state,
            lifecycle,
            health,
            attributes,
            goal,
            vehicle,
            animal,
            retained,
        };
        normalize_loaded_entity_kinematics(
            &path,
            &mut snapshot.position,
            snapshot.rotation,
            &mut snapshot.velocity,
        )?;
        validate_entity_numeric_state(&path, &snapshot)?;
        validate_entity_temporal_state(&path, items, lifecycle_tick, &snapshot)?;
        if !entity_uuids.insert(snapshot.uuid) {
            return Err(PlayerPersistenceError::DuplicateEntityUuid {
                path,
                uuid: snapshot.uuid,
            });
        }
        entities.push(PersistedEntityRecord {
            snapshot,
            age,
            pickup_delay,
        });
    }
    let checkpoint = PersistedEntityCheckpoint {
        lifecycle_clock: lifecycle_tick,
        regional_sequence_watermark,
        allocation_high_water: allocation_high_water.max(
            entities
                .iter()
                .map(|record| record.snapshot.id.0)
                .max()
                .unwrap_or(0),
        ),
        records: entities,
        generated_entity_claims,
    };
    if !checkpoint.has_valid_temporal_state() {
        return Err(PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_RETAINED_STATE_FIELD,
        });
    }
    if !checkpoint.has_valid_vehicle_graph() {
        return Err(PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_VEHICLE_STATE_FIELD,
        });
    }
    Ok(checkpoint)
}

fn validate_entity_temporal_state(
    path: &Path,
    items: &ItemRegistry,
    lifecycle_tick: u64,
    snapshot: &EntitySnapshot,
) -> Result<(), PlayerPersistenceError> {
    if !entity_temporal_state_is_valid(lifecycle_tick, snapshot)
        || !villager_population_item_state_is_valid(items, snapshot)
    {
        return Err(PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_RETAINED_STATE_FIELD,
        });
    }
    Ok(())
}

fn entity_temporal_state_is_valid(lifecycle_tick: u64, snapshot: &EntitySnapshot) -> bool {
    if snapshot
        .retained
        .last_damage_tick
        .is_some_and(|tick| tick > lifecycle_tick)
    {
        return false;
    }
    if snapshot.retained.native_birth_origin.is_some_and(|origin| {
        origin.parents[0] == origin.parents[1]
            || origin.parents.contains(&snapshot.uuid)
            || origin.started_tick > origin.birth_tick
            || origin.birth_tick > lifecycle_tick
    }) {
        return false;
    }
    let death_state_valid = match (snapshot.lifecycle, snapshot.retained.death_remove_tick) {
        (EntityLifecycle::Alive, None) => true,
        (EntityLifecycle::Despawning, Some(deadline)) => deadline
            .checked_sub(lifecycle_tick)
            .is_some_and(|remaining| remaining <= crate::entity_ticks::ENTITY_DEATH_TICKS),
        _ => false,
    };
    death_state_valid
        && !snapshot
            .retained
            .sheep_grazing_ticks
            .is_some_and(|remaining| {
                remaining == 0 || remaining > crate::entity_ticks::SHEEP_GRAZING_ANIMATION_TICKS
            })
        && zombie_villager_conversion_state_is_valid(snapshot)
        && villager_population_temporal_state_is_valid(lifecycle_tick, snapshot)
}

fn zombie_villager_conversion_state_is_valid(snapshot: &EntitySnapshot) -> bool {
    let Some(conversion) = snapshot.retained.zombie_villager_conversion else {
        return true;
    };
    if snapshot.lifecycle != EntityLifecycle::Alive
        || snapshot.type_name != "minecraft:zombie_villager"
        || conversion.completes_tick > i64::MAX as u64
        || conversion.completes_tick < snapshot.retained.spawn_tick
    {
        return false;
    }
    let Some(active_effects) = snapshot.retained.active_effects.as_ref() else {
        return false;
    };
    let strength = mc_entity::zombie_villager_26_1_2::STRENGTH_EFFECT_ID;
    let weakness = mc_entity::zombie_villager_26_1_2::WEAKNESS_EFFECT_ID;
    let has_strength = active_effects
        .effects
        .chains
        .iter()
        .any(|chain| chain.current.id == strength && chain.current.has_remaining_duration());
    let has_weakness = active_effects
        .effects
        .chains
        .iter()
        .any(|chain| chain.current.id == weakness)
        || active_effects.action_order.contains(&weakness);
    let action_order_has_strength = active_effects.action_order.contains(&strength);
    has_strength && action_order_has_strength && !has_weakness
}

fn villager_population_temporal_state_is_valid(
    lifecycle_tick: u64,
    snapshot: &EntitySnapshot,
) -> bool {
    let Some(population) = snapshot.retained.villager_population.as_ref() else {
        return true;
    };
    let Some(brain) = snapshot.retained.villager_brain.as_ref() else {
        return false;
    };
    let inventory_shape_valid = population.inventory.slots().iter().all(|slot| {
        slot.as_ref()
            .is_none_or(|stack| (1..=64).contains(&stack.count))
    });
    // Population state is retained during the death animation. The enclosing
    // entity validator checks the lifecycle and its bounded removal deadline.
    if snapshot.type_name != "minecraft:villager"
        || snapshot.retained.villager.is_none()
        || !(-24_000..=6_000).contains(&population.age_ticks)
        || population.food_level > 15
        || !inventory_shape_valid
    {
        return false;
    }
    let schedule_matches_age = if population.age_ticks < 0 {
        brain.schedule == mc_entity::villager_26_1_2::VillagerScheduleKind::Baby
    } else {
        brain.schedule == mc_entity::villager_26_1_2::VillagerScheduleKind::Adult
    };
    if !schedule_matches_age {
        return false;
    }
    let Some(pending) = population.pending_birth.as_ref() else {
        return true;
    };
    let Some(duration) = pending.ready_tick.checked_sub(pending.started_tick) else {
        return false;
    };
    population.age_ticks == 0
        && pending.partner_uuid != snapshot.uuid
        && pending.started_tick <= lifecycle_tick
        && (u64::from(mc_entity::villager_population_26_1_2::VILLAGER_BIRTH_DELAY_MIN_TICKS)
            ..=u64::from(
                mc_entity::villager_population_26_1_2::VILLAGER_BIRTH_DELAY_MIN_TICKS
                    + mc_entity::villager_population_26_1_2::VILLAGER_BIRTH_DELAY_RANDOM_BOUND
                    - 1,
            ))
            .contains(&duration)
}

fn villager_population_item_state_is_valid(
    items: &ItemRegistry,
    snapshot: &EntitySnapshot,
) -> bool {
    let Some(population) = snapshot.retained.villager_population.as_ref() else {
        return true;
    };
    if population.inventory.slots().iter().any(|slot| {
        slot.as_ref()
            .is_some_and(|stack| items.name_of(stack.item_id).is_none())
    }) {
        return false;
    }
    let Some(pending) = population.pending_birth.as_ref() else {
        return true;
    };
    let Some(food_items) = villager_food_item_ids(items) else {
        return false;
    };
    pending.partner_uuid != snapshot.uuid
        && population.total_food_points(food_items)
            >= mc_entity::villager_population_26_1_2::VILLAGER_BREEDING_WILLINGNESS_POINTS
}

#[cfg(test)]
fn villager_population_state_is_valid(
    items: &ItemRegistry,
    lifecycle_tick: u64,
    snapshot: &EntitySnapshot,
) -> bool {
    villager_population_temporal_state_is_valid(lifecycle_tick, snapshot)
        && villager_population_item_state_is_valid(items, snapshot)
}

fn villager_food_item_ids(
    items: &ItemRegistry,
) -> Option<mc_entity::villager_population_26_1_2::VillagerFoodItemIds> {
    let item_id = |name: &str| {
        mc_data::Identifier::parse(name)
            .ok()
            .and_then(|id| items.id_of(&id))
    };
    Some(mc_entity::villager_population_26_1_2::VillagerFoodItemIds {
        bread: item_id("minecraft:bread")?,
        potato: item_id("minecraft:potato")?,
        carrot: item_id("minecraft:carrot")?,
        beetroot: item_id("minecraft:beetroot")?,
    })
}

fn persisted_entity_type_is_aquatic(type_name: &str) -> bool {
    matches!(
        type_name,
        "minecraft:cod"
            | "minecraft:salmon"
            | "minecraft:tropical_fish"
            | "minecraft:pufferfish"
            | "minecraft:squid"
            | "minecraft:glow_squid"
            | "minecraft:dolphin"
            | "minecraft:axolotl"
            | "minecraft:turtle"
    )
}

#[cfg(test)]
pub(crate) fn save_persisted_entities(
    world_root: &Path,
    items: &ItemRegistry,
    entities: &[EntitySnapshot],
) -> Result<(), PlayerPersistenceError> {
    let records = entities
        .iter()
        .cloned()
        .map(PersistedEntityRecord::from)
        .collect::<Vec<_>>();
    save_persisted_entity_records(
        world_root,
        items,
        &PersistedEntityCheckpoint {
            lifecycle_clock: 0,
            regional_sequence_watermark: 0,
            allocation_high_water: 0,
            records,
            generated_entity_claims: BTreeSet::new(),
        },
    )
}

pub fn save_persisted_entity_records(
    world_root: &Path,
    items: &ItemRegistry,
    checkpoint: &PersistedEntityCheckpoint,
) -> Result<(), PlayerPersistenceError> {
    let path = entities_path(world_root);
    let root = encode_entity_checkpoint(&path, items, checkpoint)?;
    write_persistence_root(&path, "", &root, NamedRootPolicy::EntityDocument)
}

fn encode_entity_checkpoint(
    path: &Path,
    items: &ItemRegistry,
    checkpoint: &PersistedEntityCheckpoint,
) -> Result<Tag, PlayerPersistenceError> {
    let path = path.to_path_buf();
    let lifecycle_tick = i64::try_from(checkpoint.lifecycle_clock).map_err(|_| {
        PlayerPersistenceError::InvalidValue {
            path: path.clone(),
            field: ENTITY_LIFECYCLE_TICK_FIELD,
        }
    })?;
    let regional_sequence =
        i64::try_from(checkpoint.regional_sequence_watermark).map_err(|_| {
            PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_REGIONAL_SEQUENCE_FIELD,
            }
        })?;
    if !checkpoint.has_valid_temporal_state() {
        return Err(PlayerPersistenceError::InvalidValue {
            path,
            field: ENTITY_RETAINED_STATE_FIELD,
        });
    }
    if !checkpoint.has_valid_vehicle_graph() {
        return Err(PlayerPersistenceError::InvalidValue {
            path,
            field: ENTITY_VEHICLE_STATE_FIELD,
        });
    }
    let mut elements = Vec::new();
    for record in &checkpoint.records {
        validate_entity_numeric_state(&path, &record.snapshot)?;
        validate_entity_temporal_state(&path, items, checkpoint.lifecycle_clock, &record.snapshot)?;
        elements.push(entity_tag(&path, items, record)?);
    }
    let root = Tag::Compound(vec![
        (
            ENTITY_FORMAT_VERSION_FIELD.into(),
            Tag::Int(ENTITY_FORMAT_VERSION),
        ),
        (
            ENTITY_LIFECYCLE_TICK_FIELD.into(),
            Tag::Long(lifecycle_tick),
        ),
        (
            ENTITY_REGIONAL_SEQUENCE_FIELD.into(),
            Tag::Long(regional_sequence),
        ),
        (
            ENTITY_ALLOCATION_HIGH_WATER_FIELD.into(),
            Tag::Int(
                checkpoint.allocation_high_water.max(
                    checkpoint
                        .records
                        .iter()
                        .map(|record| record.snapshot.id.0)
                        .max()
                        .unwrap_or(0),
                ),
            ),
        ),
        (
            "Entities".into(),
            Tag::List(ListTag {
                element_type: if elements.is_empty() {
                    tag_type::END
                } else {
                    tag_type::COMPOUND
                },
                elements,
            }),
        ),
        (
            ENTITY_GENERATED_ENTITY_CLAIMS_FIELD.into(),
            Tag::List(ListTag {
                element_type: if checkpoint.generated_entity_claims.is_empty() {
                    tag_type::END
                } else {
                    tag_type::STRING
                },
                elements: checkpoint
                    .generated_entity_claims
                    .iter()
                    .cloned()
                    .map(Tag::String)
                    .collect(),
            }),
        ),
    ]);
    Ok(root)
}

pub fn load_world_metadata(
    world_root: &Path,
) -> Result<Option<WorldPersistedMetadata>, PlayerPersistenceError> {
    let path = world_metadata_path(world_root);
    if !path.is_file() {
        return Ok(None);
    }
    let (_, root) = read_player_root(&path)?;
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    let players_sleeping_percentage = match long_field(&fields, "SolarisPlayersSleepingPercentage")
    {
        Some(value) => u32::try_from(value).map_err(|_| PlayerPersistenceError::InvalidValue {
            path: path.clone(),
            field: "SolarisPlayersSleepingPercentage",
        })?,
        None => 100,
    };
    let keep_inventory = match field(&fields, "SolarisKeepInventory") {
        Some(Tag::Byte(0)) | None => false,
        Some(Tag::Byte(1)) => true,
        _ => {
            return Err(PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: "SolarisKeepInventory",
            });
        }
    };
    let daylight_cycle_enabled = match field(&fields, "SolarisDaylightCycleEnabled") {
        Some(Tag::Byte(0)) => false,
        Some(Tag::Byte(1)) | None => true,
        _ => {
            return Err(PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: "SolarisDaylightCycleEnabled",
            });
        }
    };
    let weather_kind =
        match field(&fields, "SolarisWeatherState") {
            Some(Tag::Byte(value)) => crate::weather::WeatherKind::from_u8(*value as u8)
                .ok_or_else(|| PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: "SolarisWeatherState",
                })?,
            None => crate::weather::WeatherKind::Clear,
            _ => {
                return Err(PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: "SolarisWeatherState",
                });
            }
        };
    let rain_level = int_field(&fields, "SolarisRainLevelBits")
        .map(|bits| f32::from_bits(bits as u32))
        .unwrap_or(0.0);
    let thunder_level = int_field(&fields, "SolarisThunderLevelBits")
        .map(|bits| f32::from_bits(bits as u32))
        .unwrap_or(0.0);
    let weather = crate::weather::WeatherState::new(weather_kind, rain_level, thunder_level)
        .ok_or_else(|| PlayerPersistenceError::InvalidValue {
            path: path.clone(),
            field: "SolarisWeatherState",
        })?;
    Ok(Some(WorldPersistedMetadata {
        world_time: long_field(&fields, "SolarisWorldTime").unwrap_or(0) as u64,
        daylight_cycle_enabled,
        weather,
        players_sleeping_percentage,
        keep_inventory,
        command_rules: match field(&fields, "SolarisCommandRules") {
            Some(Tag::Compound(rules)) => rules
                .iter()
                .filter_map(|(name, value)| {
                    if let Tag::String(value) = value {
                        Some((name.clone(), value.clone()))
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Default::default(),
        },
        weather_duration_ticks: long_field(&fields, "SolarisWeatherDurationTicks")
            .unwrap_or(0)
            .max(0) as u64,
        world_identity: string_field(&fields, "SolarisWorldIdentity")
            .unwrap_or_default()
            .to_string(),
    }))
}

pub fn save_world_metadata(
    world_root: &Path,
    metadata: &WorldPersistedMetadata,
) -> Result<(), PlayerPersistenceError> {
    let path = world_metadata_path(world_root);
    let root = Tag::Compound(vec![
        (
            "SolarisWorldTime".into(),
            Tag::Long(metadata.world_time as i64),
        ),
        (
            "SolarisWorldIdentity".into(),
            Tag::String(metadata.world_identity.clone()),
        ),
        (
            "SolarisDaylightCycleEnabled".into(),
            Tag::Byte(i8::from(metadata.daylight_cycle_enabled)),
        ),
        (
            "SolarisWeatherState".into(),
            Tag::Byte(metadata.weather.kind().as_u8() as i8),
        ),
        (
            "SolarisRainLevelBits".into(),
            Tag::Int(metadata.weather.rain_level().to_bits() as i32),
        ),
        (
            "SolarisThunderLevelBits".into(),
            Tag::Int(metadata.weather.thunder_level().to_bits() as i32),
        ),
        (
            "SolarisPlayersSleepingPercentage".into(),
            Tag::Long(i64::from(metadata.players_sleeping_percentage)),
        ),
        (
            "SolarisKeepInventory".into(),
            Tag::Byte(i8::from(metadata.keep_inventory)),
        ),
        (
            "SolarisCommandRules".into(),
            Tag::Compound(
                metadata
                    .command_rules
                    .iter()
                    .map(|(name, value)| (name.clone(), Tag::String(value.clone())))
                    .collect(),
            ),
        ),
        (
            "SolarisWeatherDurationTicks".into(),
            Tag::Long(i64::try_from(metadata.weather_duration_ticks).unwrap_or(i64::MAX)),
        ),
    ]);
    write_player_root(&path, "", &root)
}

pub fn world_identity(world_root: &Path) -> String {
    world_root.to_string_lossy().into_owned()
}

fn world_metadata_path(world_root: &Path) -> PathBuf {
    world_root.join(SOLARIS_DIR).join(WORLD_FILE)
}

fn entities_path(world_root: &Path) -> PathBuf {
    world_root.join(SOLARIS_DIR).join(ENTITIES_FILE)
}

fn entity_tag(
    path: &Path,
    items: &ItemRegistry,
    record: &PersistedEntityRecord,
) -> Result<Tag, PlayerPersistenceError> {
    let entity = &record.snapshot;
    let attributes = serde_json::to_string(&entity.attributes).map_err(|_| {
        PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_ATTRIBUTES_FIELD,
        }
    })?;
    let retained = serde_json::to_string(&entity.retained).map_err(|_| {
        PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_RETAINED_STATE_FIELD,
        }
    })?;
    let unmounted_goal =
        entity
            .retained
            .animal_interaction
            .rider
            .is_some()
            .then(|| mc_entity::GoalState::Wander {
                speed: entity
                    .attributes
                    .base(&mc_entity::AttributeKind::MovementSpeed)
                    .unwrap_or(0.2)
                    * 10.0,
                period_ticks: 80,
            });
    let goal =
        serde_json::to_string(unmounted_goal.as_ref().unwrap_or(&entity.goal)).map_err(|_| {
            PlayerPersistenceError::InvalidValue {
                path: path.to_path_buf(),
                field: ENTITY_GOAL_STATE_FIELD,
            }
        })?;
    let vehicle = serde_json::to_string(&entity.vehicle).map_err(|_| {
        PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_VEHICLE_STATE_FIELD,
        }
    })?;
    let lifecycle = match entity.lifecycle {
        EntityLifecycle::Alive => 0,
        EntityLifecycle::Despawning => 1,
    };
    let mut fields = vec![
        ("id".into(), Tag::String(entity.type_name.clone())),
        ("SolarisEntityId".into(), Tag::Int(entity.id.0)),
        ("UUID".into(), Tag::IntArray(uuid_to_int_array(entity.uuid))),
        ("Pos".into(), vec3_double_list(entity.position)),
        (
            "Motion".into(),
            vec3_double_list(Vec3::new(
                entity.velocity.x / ENTITY_TICKS_PER_SECOND,
                entity.velocity.y / ENTITY_TICKS_PER_SECOND,
                entity.velocity.z / ENTITY_TICKS_PER_SECOND,
            )),
        ),
        (
            "Rotation".into(),
            Tag::List(ListTag {
                element_type: tag_type::FLOAT,
                elements: vec![
                    Tag::Float(entity.rotation.yaw),
                    Tag::Float(entity.rotation.pitch),
                ],
            }),
        ),
        ("OnGround".into(), Tag::Byte(i8::from(entity.on_ground))),
        ("NoAI".into(), Tag::Byte(i8::from(entity.retained.no_ai))),
        (
            "Invulnerable".into(),
            Tag::Byte(i8::from(entity.retained.invulnerable)),
        ),
        (
            "FallDistance".into(),
            Tag::Float(entity.retained.fall_distance as f32),
        ),
        (
            "Fire".into(),
            Tag::Short(
                entity
                    .retained
                    .remaining_fire_ticks
                    .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            ),
        ),
        ("Health".into(), Tag::Float(entity.health)),
        (ENTITY_ATTRIBUTES_FIELD.into(), Tag::String(attributes)),
        (ENTITY_LIFECYCLE_FIELD.into(), Tag::Byte(lifecycle)),
        (ENTITY_RETAINED_STATE_FIELD.into(), Tag::String(retained)),
        (
            ENTITY_HEAD_YAW_FIELD.into(),
            Tag::Float(entity.rotation.head_yaw),
        ),
        (ENTITY_GOAL_STATE_FIELD.into(), Tag::String(goal)),
        (ENTITY_VEHICLE_STATE_FIELD.into(), Tag::String(vehicle)),
        ("SolarisLifetimeAge".into(), Tag::Int(record.age.max(0))),
    ];
    if let Some(animal) = entity.animal {
        fields.push(("Age".into(), Tag::Int(animal.age_ticks)));
        fields.push(("InLove".into(), Tag::Int(i32::from(animal.love_ticks))));
        if let Some(wool) = animal.sheep_wool {
            fields.push(("Color".into(), Tag::Byte(wool.color.id() as i8)));
            fields.push(("Sheared".into(), Tag::Byte(i8::from(wool.sheared))));
        }
    } else {
        fields.push(("Age".into(), Tag::Int(record.age.max(0))));
    }
    append_animal_interaction_tags(&mut fields, entity);
    if persisted_entity_type_is_living(&entity.type_name) {
        let equipment = entity.retained.equipment.as_ref();
        let saddle = entity.retained.animal_interaction.saddle.as_ref();
        if equipment.is_some() || saddle.is_some() {
            // LivingEntity uses one equipment compound for every slot, including saddle.
            let mut gear = Vec::new();
            if let Some(stack) = equipment
                .and_then(|equipment| equipment.mainhand.as_ref())
                .filter(|stack| stack.count > 0)
            {
                gear.push(("mainhand".into(), entity_item_stack_tag(items, stack)?));
            }
            if let Some(saddle) = saddle {
                gear.push(("saddle".into(), entity_item_stack_tag(items, saddle)?));
            }
            fields.push(("equipment".into(), Tag::Compound(gear)));
        }
    } else if let Some(stack) = &entity.item_stack {
        let item = entity_item_stack_tag(items, stack)?;
        fields.push(("Item".into(), item));
        fields.push((
            "PickupDelay".into(),
            Tag::Short(record.pickup_delay.clamp(0, i32::from(i16::MAX)) as i16),
        ));
    }
    if let Some(value) = entity.experience_value {
        fields.push(("Value".into(), Tag::Int(value.max(0))));
    }
    if let Some(block_state) = entity.block_state {
        fields.push(("BlockState".into(), Tag::Int(block_state as i32)));
    }
    Ok(Tag::Compound(fields))
}

fn persisted_entity_type_is_living(type_name: &str) -> bool {
    entity_type_contract_26_1_2_by_name(type_name)
        .is_some_and(|contract| contract.behavior.archetype == EntityArchetype::Living)
}

fn persisted_entity_type_is_supported_breeding_animal(type_name: &str) -> bool {
    mc_entity::natural_spawn_26_1_2::default_breeding_state(type_name).is_some()
}

// 26.1.2 javap: TamableAnimal/AbstractHorse Owner is EntityReference.CODEC
// (UUID int array); LeashData uses lowercase `leash` with a UUID compound.
fn append_animal_interaction_tags(fields: &mut Vec<(String, Tag)>, entity: &EntitySnapshot) {
    let state = &entity.retained.animal_interaction;
    if let Some(owner) = state.owner {
        fields.push(("Owner".into(), Tag::IntArray(uuid_to_int_array(owner))));
    }
    if matches!(
        entity.type_name.as_str(),
        "minecraft:cat" | "minecraft:wolf" | "minecraft:parrot"
    ) {
        fields.push(("Sitting".into(), Tag::Byte(i8::from(state.sitting))));
        if entity.type_name != "minecraft:parrot" {
            fields.push(("CollarColor".into(), Tag::Byte(state.collar_color as i8)));
        }
    }
    if matches!(
        entity.type_name.as_str(),
        "minecraft:horse"
            | "minecraft:donkey"
            | "minecraft:mule"
            | "minecraft:llama"
            | "minecraft:trader_llama"
    ) {
        fields.push(("Tame".into(), Tag::Byte(i8::from(state.is_tamed()))));
        fields.push(("Temper".into(), Tag::Int(i32::from(state.temperament))));
    }
    if let Some(holder) = state.leash_holder {
        fields.push((
            "leash".into(),
            Tag::Compound(vec![(
                "UUID".into(),
                Tag::IntArray(uuid_to_int_array(holder)),
            )]),
        ));
    }
    if let Some(name) = &state.custom_name {
        fields.push(("CustomName".into(), Tag::String(name.clone())));
    }
    if let Some(cause) = state.love_cause {
        fields.push(("LoveCause".into(), Tag::IntArray(uuid_to_int_array(cause))));
    }
    if entity.type_name == "minecraft:mooshroom" {
        fields.push((
            "Type".into(),
            Tag::String(
                if state.brown_mooshroom {
                    "brown"
                } else {
                    "red"
                }
                .into(),
            ),
        ));
        if !state.stew_effects.is_empty() {
            fields.push((
                "stew_effects".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: state
                        .stew_effects
                        .iter()
                        .map(|effect| {
                            Tag::Compound(vec![
                                ("id".into(), Tag::String(effect.id.to_string())),
                                ("duration".into(), Tag::Int(effect.duration)),
                            ])
                        })
                        .collect(),
                }),
            ));
        }
    }
    if entity.type_name == "minecraft:ocelot" {
        fields.push(("Trusting".into(), Tag::Byte(i8::from(state.trusting))));
    }
    if entity.type_name == "minecraft:fox" {
        fields.push((
            "Trusted".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT_ARRAY,
                elements: state
                    .trusted_players
                    .iter()
                    .map(|uuid| Tag::IntArray(uuid_to_int_array(*uuid)))
                    .collect(),
            }),
        ));
    }
}

fn read_animal_interaction_tags(
    fields: &[(String, Tag)],
    items: &ItemRegistry,
    state: &mut mc_entity::animal_interactions::AnimalInteractionState,
) -> Result<(), PlayerPersistenceError> {
    for (key, value) in fields {
        apply_animal_interaction_tag(state, items, key, value)?;
    }
    // Runtime links are resolved against live sessions, never world ids.
    state.rider = None;
    state.rider_entity_id = None;
    state.leash_holder_entity_id = None;
    state.leash_publication = false;
    Ok(())
}

pub fn apply_animal_interaction_tag(
    state: &mut mc_entity::animal_interactions::AnimalInteractionState,
    items: &ItemRegistry,
    key: &str,
    value: &Tag,
) -> Result<bool, PlayerPersistenceError> {
    let invalid = || PlayerPersistenceError::InvalidItemComponent("animal interaction NBT");
    let integer = || match value {
        Tag::Byte(value) => Ok(i32::from(*value)),
        Tag::Short(value) => Ok(i32::from(*value)),
        Tag::Int(value) => Ok(*value),
        _ => Err(invalid()),
    };
    match key {
        "Owner" => state.owner = Some(animal_uuid_tag(value).ok_or_else(invalid)?),
        "Tame" => {
            state.tamed = integer()? != 0;
            if !state.tamed {
                state.owner = None;
            }
        }
        "Sitting" => state.sitting = integer()? != 0,
        "Temper" => state.temperament = integer()?.clamp(0, 100) as u8,
        "CollarColor" => state.collar_color = integer()?.clamp(0, 15) as u8,
        "equipment" => {
            let Tag::Compound(equipment) = value else {
                return Err(invalid());
            };
            state.saddle = match field(equipment, "saddle") {
                Some(Tag::Compound(saddle)) => read_entity_item_stack(saddle, items)?,
                Some(_) => return Err(invalid()),
                None => None,
            };
        }
        "leash" => {
            let Tag::Compound(leash) = value else {
                return Err(invalid());
            };
            state.leash_holder = Some(uuid_field(leash).ok_or_else(invalid)?);
        }
        "CustomName" => {
            let Tag::String(name) = value else {
                return Err(invalid());
            };
            state.custom_name = Some(name.clone());
        }
        "LoveCause" => state.love_cause = Some(animal_uuid_tag(value).ok_or_else(invalid)?),
        "Type" => {
            let Tag::String(name) = value else {
                return Err(invalid());
            };
            state.brown_mooshroom = name == "brown";
        }
        "Trusting" => state.trusting = integer()? != 0,
        "Trusted" => {
            let Tag::List(players) = value else {
                return Err(invalid());
            };
            state.trusted_players = players
                .elements
                .iter()
                .map(|value| animal_uuid_tag(value).ok_or_else(invalid))
                .collect::<Result<_, _>>()?;
            state.trusting = !state.trusted_players.is_empty();
        }
        "stew_effects" => {
            let Tag::List(effects) = value else {
                return Err(invalid());
            };
            state.stew_effects = effects
                .elements
                .iter()
                .map(|tag| {
                    let Tag::Compound(effect) = tag else {
                        return Err(invalid());
                    };
                    let id = string_field(effect, "id")
                        .and_then(|id| mc_data::Identifier::parse(id).ok())
                        .ok_or_else(invalid)?;
                    let duration = int_field(effect, "duration").unwrap_or(160);
                    Ok(mc_data::item_stack::StewEffect { id, duration })
                })
                .collect::<Result<_, _>>()?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn animal_uuid_tag(tag: &Tag) -> Option<uuid::Uuid> {
    let Tag::IntArray(parts) = tag else {
        return None;
    };
    let [a, b, c, d] = parts.as_slice() else {
        return None;
    };
    Some(uuid::Uuid::from_u128(
        (u128::from(*a as u32) << 96)
            | (u128::from(*b as u32) << 64)
            | (u128::from(*c as u32) << 32)
            | u128::from(*d as u32),
    ))
}

pub fn entity_item_stack_tag(
    items: &ItemRegistry,
    stack: &EntityItemStack,
) -> Result<Tag, PlayerPersistenceError> {
    let name = items
        .name_of(stack.item_id)
        .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
    let mut fields = vec![
        ("id".into(), Tag::String(name.as_str().to_string())),
        ("count".into(), Tag::Int(stack.count)),
    ];
    if let Some(damage) = stack.damage {
        set_damage_component(&mut fields, damage);
    }
    if let Some(custom_name) = &stack.custom_name {
        set_custom_name_component(&mut fields, custom_name);
    }
    if let Some(item_model) = &stack.item_model {
        set_item_model_component(&mut fields, item_model);
    }
    if !stack.enchantments.is_empty() {
        set_enchantments_component(&mut fields, &stack.enchantments);
    }
    stew_effects::save(&mut fields, &stack.stew_effects);
    stack_components::save(&mut fields, stack.extended(), items)?;
    Ok(Tag::Compound(fields))
}

pub fn read_entity_item_stack(
    fields: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<Option<EntityItemStack>, PlayerPersistenceError> {
    let Some(item_name) = string_field(fields, "id") else {
        return Ok(None);
    };
    let parsed = mc_data::Identifier::parse(item_name.to_string())
        .map_err(|_| PlayerPersistenceError::InvalidItemId(item_name.to_string()))?;
    let Some(item_id) = items.id_of(&parsed) else {
        return Err(PlayerPersistenceError::UnknownItem(item_name.to_string()));
    };
    let count = int_field(fields, "count").unwrap_or(1).max(0);
    Ok((count > 0).then_some(EntityItemStack {
        item_id,
        count,
        damage: damage_component(fields),
        enchantments: enchantments_component(fields)?,
        custom_name: custom_name_component(fields).map(Box::new),
        item_model: item_model_component(fields)?
            .as_deref()
            .cloned()
            .map(Box::new),
        stew_effects: stew_effects::load(fields)?,
        components: stack_components::load(fields, items)?,
    }))
}

fn vec3_double_list(vec: Vec3) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::DOUBLE,
        elements: vec![Tag::Double(vec.x), Tag::Double(vec.y), Tag::Double(vec.z)],
    })
}

fn uuid_to_int_array(uuid: uuid::Uuid) -> Vec<i32> {
    let bytes = uuid.as_u128().to_be_bytes();
    bytes
        .chunks_exact(4)
        .map(|chunk| i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn uuid_field(fields: &[(String, Tag)]) -> Option<uuid::Uuid> {
    let Tag::IntArray(values) = field(fields, "UUID")? else {
        return None;
    };
    if values.len() != 4 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (idx, value) in values.iter().enumerate() {
        bytes[idx * 4..idx * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    Some(uuid::Uuid::from_u128(u128::from_be_bytes(bytes)))
}

pub fn playerdata_path(world_root: &Path, uuid: uuid::Uuid) -> PathBuf {
    world_root
        .join(PLAYERDATA_DIR)
        .join(format!("{}.dat", uuid.hyphenated()))
}

enum BoundedLength {
    Exact(usize),
    TooLarge { at_least: usize },
}

fn count_bounded_bytes(reader: &mut impl Read, max: usize) -> std::io::Result<BoundedLength> {
    let mut total = 0_usize;
    let mut scratch = [0_u8; 8192];
    loop {
        let remaining = max.saturating_sub(total);
        if remaining == 0 {
            let mut extra = [0_u8; 1];
            return if reader.read(&mut extra)? == 0 {
                Ok(BoundedLength::Exact(total))
            } else {
                Ok(BoundedLength::TooLarge {
                    at_least: max.saturating_add(1),
                })
            };
        }

        let chunk_len = remaining.min(scratch.len());
        let read = reader.read(&mut scratch[..chunk_len])?;
        if read == 0 {
            return Ok(BoundedLength::Exact(total));
        }
        total = total.saturating_add(read);
    }
}

fn allocate_exact_bytes(len: usize, context: &'static str) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|error| {
        std::io::Error::other(format!("reserve exact {context} buffer: {error}"))
    })?;
    bytes.resize(len, 0);
    Ok(bytes)
}

fn read_bounded_persistence_file(
    path: &Path,
    max: usize,
) -> Result<Vec<u8>, PlayerPersistenceError> {
    // The opened descriptor pins the file object; later path replacement cannot
    // switch this read to a different inode, and concurrent growth is caught by
    // the one-byte-over-limit probe.
    let mut file = File::open(path).map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let len =
        match count_bounded_bytes(&mut file, max).map_err(|source| PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        })? {
            BoundedLength::Exact(len) => len,
            BoundedLength::TooLarge { at_least } => {
                return Err(PlayerPersistenceError::DataTooLarge {
                    path: path.to_path_buf(),
                    kind: "compressed NBT",
                    bytes: at_least,
                    max,
                });
            }
        };
    file.seek(SeekFrom::Start(0))
        .map_err(|source| PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut bytes = allocate_exact_bytes(len, "compressed NBT").map_err(|source| {
        PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    file.read_exact(&mut bytes)
        .map_err(|source| PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .map_err(|source| PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        })?
        != 0
    {
        return Err(PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "persistence file changed while being read",
            ),
        });
    }
    Ok(bytes)
}

fn decode_single_gzip_member(
    path: &Path,
    compressed: &[u8],
    max: usize,
) -> Result<Vec<u8>, PlayerPersistenceError> {
    let mut decoder = GzDecoder::new(Cursor::new(compressed));
    let decoded_len = match count_bounded_bytes(&mut decoder, max).map_err(|source| {
        PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        }
    })? {
        BoundedLength::Exact(len) => len,
        BoundedLength::TooLarge { at_least } => {
            return Err(PlayerPersistenceError::DataTooLarge {
                path: path.to_path_buf(),
                kind: "decompressed NBT",
                bytes: at_least,
                max,
            });
        }
    };
    let consumed = decoder.into_inner().position() as usize;
    if consumed != compressed.len() {
        return Err(PlayerPersistenceError::TrailingData {
            path: path.to_path_buf(),
            kind: "compressed gzip member",
            bytes: compressed.len().saturating_sub(consumed),
        });
    }

    let mut decoded = allocate_exact_bytes(decoded_len, "decompressed NBT").map_err(|source| {
        PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let mut decoder = GzDecoder::new(Cursor::new(compressed));
    decoder
        .read_exact(&mut decoded)
        .map_err(|source| PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut extra = [0_u8; 1];
    if decoder
        .read(&mut extra)
        .map_err(|source| PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        })?
        != 0
    {
        return Err(PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "gzip output changed between bounded passes",
            ),
        });
    }
    let consumed = decoder.into_inner().position() as usize;
    if consumed != compressed.len() {
        return Err(PlayerPersistenceError::TrailingData {
            path: path.to_path_buf(),
            kind: "compressed gzip member",
            bytes: compressed.len().saturating_sub(consumed),
        });
    }
    Ok(decoded)
}

#[derive(Clone, Copy)]
enum NamedRootPolicy {
    Generic,
    EntityDocument,
}

fn read_player_root(path: &Path) -> Result<(String, Tag), PlayerPersistenceError> {
    read_persistence_root(path, NamedRootPolicy::Generic)
}

fn read_persistence_root(
    path: &Path,
    policy: NamedRootPolicy,
) -> Result<(String, Tag), PlayerPersistenceError> {
    let compressed = read_bounded_persistence_file(path, MAX_PERSISTENCE_COMPRESSED_NBT_BYTES)?;
    let decoded =
        decode_single_gzip_member(path, &compressed, MAX_PERSISTENCE_DECOMPRESSED_NBT_BYTES)?;
    let mut slice = decoded.as_slice();
    let root = match policy {
        NamedRootPolicy::Generic => mc_nbt::read_named(&mut slice),
        NamedRootPolicy::EntityDocument => mc_nbt::read_named_document(&mut slice),
    }
    .map_err(|source| PlayerPersistenceError::Nbt {
        path: path.to_path_buf(),
        source,
    })?;
    if !slice.is_empty() {
        return Err(PlayerPersistenceError::TrailingData {
            path: path.to_path_buf(),
            kind: "decompressed NBT",
            bytes: slice.len(),
        });
    }
    Ok(root)
}

fn write_player_root(
    path: &Path,
    root_name: &str,
    root: &Tag,
) -> Result<(), PlayerPersistenceError> {
    write_persistence_root(path, root_name, root, NamedRootPolicy::Generic)
}

fn write_persistence_root(
    path: &Path,
    root_name: &str,
    root: &Tag,
    policy: NamedRootPolicy,
) -> Result<(), PlayerPersistenceError> {
    // Preflight before creating a temporary file or touching the old checkpoint.
    let mut bytes = Vec::new();
    match policy {
        NamedRootPolicy::Generic => mc_nbt::write_named(&mut bytes, root_name, root),
        NamedRootPolicy::EntityDocument => {
            mc_nbt::write_named_document(&mut bytes, root_name, root)
        }
    }
    .map_err(|source| PlayerPersistenceError::Nbt {
        path: path.to_path_buf(),
        source,
    })?;
    if let Some(parent) = path.parent() {
        create_persistence_directory(parent)?;
    }
    let tmp_path = temporary_write_path(path);
    let file = File::create(&tmp_path).map_err(|source| PlayerPersistenceError::Io {
        path: tmp_path.clone(),
        source,
    })?;
    let mut encoder = GzEncoder::new(file, GzipCompression::default());
    encoder
        .write_all(&bytes)
        .map_err(|source| PlayerPersistenceError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    let file = encoder
        .finish()
        .map_err(|source| PlayerPersistenceError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    let compressed_bytes = file
        .metadata()
        .map_err(|source| PlayerPersistenceError::Io {
            path: tmp_path.clone(),
            source,
        })?
        .len();
    if compressed_bytes > MAX_PERSISTENCE_COMPRESSED_NBT_BYTES as u64 {
        drop(file);
        let _ = std::fs::remove_file(&tmp_path);
        return Err(PlayerPersistenceError::DataTooLarge {
            path: path.to_path_buf(),
            kind: "compressed NBT",
            bytes: usize::try_from(compressed_bytes).unwrap_or(usize::MAX),
            max: MAX_PERSISTENCE_COMPRESSED_NBT_BYTES,
        });
    }
    file.sync_all()
        .map_err(|source| PlayerPersistenceError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    mc_world::atomic_file::replace_file(&tmp_path, path).map_err(|source| {
        PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn create_persistence_directory(path: &Path) -> Result<(), PlayerPersistenceError> {
    let existed = path.is_dir();
    std::fs::create_dir_all(path).map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !existed && let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), PlayerPersistenceError> {
    let dir = File::open(path).map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    dir.sync_all().map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), PlayerPersistenceError> {
    Ok(())
}

fn temporary_write_path(path: &Path) -> PathBuf {
    let counter = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("persisted.dat");
    path.with_file_name(format!("{file_name}.{pid}.{counter}.tmp"))
}

fn read_pose(fields: &[(String, Tag)]) -> Option<PersistedPlayerPose> {
    let pos = double_list::<3>(field(fields, "Pos")?, 3)?;
    let rotation = float_list::<2>(field(fields, "Rotation")?, 2)?;
    let mut pose = PersistedPlayerPose::new(pos[0], pos[1], pos[2]);
    pose.yaw = rotation[0];
    pose.pitch = rotation[1];
    pose.on_ground = byte_field(fields, "OnGround").unwrap_or(0) != 0;
    Some(pose)
}

fn read_spawn(fields: &[(String, Tag)]) -> Option<SpawnState> {
    Some(SpawnState {
        x: int_field(fields, "SpawnX")?,
        y: int_field(fields, "SpawnY")?,
        z: int_field(fields, "SpawnZ")?,
        angle: float_field(fields, "SpawnAngle").unwrap_or(0.0),
    })
}

fn pose_position_tag(pose: PersistedPlayerPose) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::DOUBLE,
        elements: vec![
            Tag::Double(pose.x),
            Tag::Double(pose.y),
            Tag::Double(pose.z),
        ],
    })
}

fn pose_rotation_tag(pose: PersistedPlayerPose) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::FLOAT,
        elements: vec![Tag::Float(pose.yaw), Tag::Float(pose.pitch)],
    })
}

fn inventory_tag(
    items: &ItemRegistry,
    extras: &PlayerInventoryExtras,
    inventory: &PlayerInventory,
) -> Result<Tag, PlayerPersistenceError> {
    let mut elements = Vec::new();
    for (slot, stack) in inventory.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        let name = items
            .name_of(stack.item_id)
            .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
        let mut fields = extras.slots[slot]
            .as_ref()
            .filter(|extras| {
                extras.item_id == stack.item_id
                    && extras.damage == stack.damage
                    && extras.enchantments == stack.enchantments
                    && extras.custom_name.as_deref() == stack.custom_name.as_deref()
                    && extras.item_model == stack.item_model
            })
            .map(|extras| extras.fields.clone())
            .unwrap_or_default();
        set_field(&mut fields, "Slot", Tag::Byte(slot as i8));
        set_item_stack_fields(&mut fields, name, stack, items)?;
        elements.push(Tag::Compound(fields));
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
}

fn item_stack_from_fields(
    items: &ItemRegistry,
    fields: &[(String, Tag)],
) -> Result<Option<ItemStack>, PlayerPersistenceError> {
    let Some(item_name) = string_field(fields, "id") else {
        return Ok(None);
    };
    let parsed = mc_data::Identifier::parse(item_name.to_string())
        .map_err(|_| PlayerPersistenceError::InvalidItemId(item_name.to_string()))?;
    let item_id = items
        .id_of(&parsed)
        .ok_or_else(|| PlayerPersistenceError::UnknownItem(item_name.to_string()))?;
    Ok(Some(ItemStack {
        count: int_field(fields, "count").unwrap_or(1).max(0),
        item_id,
        damage: damage_component(fields),
        enchantments: enchantments_component(fields)?,
        custom_name: custom_name_component(fields).map(String::into_boxed_str),
        item_model: item_model_component(fields)?,
        stew_effects: stew_effects::load(fields)?,
        components: stack_components::load(fields, items)?,
    }))
}

fn item_stack_tag(items: &ItemRegistry, stack: &ItemStack) -> Result<Tag, PlayerPersistenceError> {
    if stack.is_empty() {
        return Ok(Tag::Compound(Vec::new()));
    }
    let name = items
        .name_of(stack.item_id)
        .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
    let mut fields = Vec::new();
    set_item_stack_fields(&mut fields, name, stack, items)?;
    Ok(Tag::Compound(fields))
}

fn item_stack_projection_from_field<const N: usize>(
    items: &ItemRegistry,
    fields: &[(String, Tag)],
    name: &str,
) -> Result<Option<Box<[ItemStack; N]>>, PlayerPersistenceError> {
    let Some(Tag::List(list)) = field(fields, name) else {
        return Ok(None);
    };
    let mut stacks = std::array::from_fn(|_| ItemStack::EMPTY);
    for element in &list.elements {
        let Tag::Compound(item_fields) = element else {
            continue;
        };
        let Some(slot) = slot_field(item_fields).filter(|slot| *slot < N) else {
            continue;
        };
        if let Some(stack) = item_stack_from_fields(items, item_fields)? {
            stacks[slot] = stack;
        }
    }
    if stacks.iter().all(ItemStack::is_empty) {
        Ok(None)
    } else {
        Ok(Some(Box::new(stacks)))
    }
}

fn item_stack_projection_tag<const N: usize>(
    items: &ItemRegistry,
    projection: Option<&[ItemStack; N]>,
) -> Result<Tag, PlayerPersistenceError> {
    let mut elements = Vec::new();
    if let Some(projection) = projection {
        for (slot, stack) in projection.iter().enumerate() {
            if stack.is_empty() {
                continue;
            }
            let name = items
                .name_of(stack.item_id)
                .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
            let mut fields = vec![("Slot".into(), Tag::Byte(slot as i8))];
            set_item_stack_fields(&mut fields, name, stack, items)?;
            elements.push(Tag::Compound(fields));
        }
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
}

fn set_item_stack_fields(
    fields: &mut Vec<(String, Tag)>,
    name: &Identifier,
    stack: &ItemStack,
    items: &ItemRegistry,
) -> Result<(), PlayerPersistenceError> {
    set_field(fields, "id", Tag::String(name.as_str().to_string()));
    set_field(fields, "count", Tag::Int(stack.count));
    if let Some(damage) = stack.damage {
        set_damage_component(fields, damage);
    }
    if let Some(custom_name) = &stack.custom_name {
        set_custom_name_component(fields, custom_name);
    }
    if let Some(item_model) = &stack.item_model {
        set_item_model_component(fields, item_model);
    }
    if !stack.enchantments.is_empty() {
        set_enchantments_component(fields, &stack.enchantments);
    }
    stew_effects::save(fields, &stack.stew_effects);
    stack_components::save(fields, stack.extended(), items)?;
    Ok(())
}

fn damage_component(fields: &[(String, Tag)]) -> Option<i32> {
    let Tag::Compound(components) = field(fields, "components")? else {
        return None;
    };
    int_field(components, DAMAGE_COMPONENT)
}

fn set_damage_component(fields: &mut Vec<(String, Tag)>, damage: i32) {
    let components = field_mut(fields, "components").and_then(|tag| match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    });
    if let Some(components) = components {
        set_field(components, DAMAGE_COMPONENT, Tag::Int(damage));
    } else {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(DAMAGE_COMPONENT.into(), Tag::Int(damage))]),
        );
    }
}

/// The local 26.1.2 `DataComponents.CUSTOM_NAME` type is a Component. Solaris
/// emits and restores the literal text-component form used by script labels;
/// richer component data remains in `inventory_extras` untouched.
fn custom_name_component(fields: &[(String, Tag)]) -> Option<String> {
    let Tag::Compound(components) = field(fields, "components")? else {
        return None;
    };
    let Tag::Compound(component) = field(components, CUSTOM_NAME_COMPONENT)? else {
        return None;
    };
    let [(name, Tag::String(value))] = component.as_slice() else {
        return None;
    };
    (name == "text").then(|| value.clone())
}

fn set_custom_name_component(fields: &mut Vec<(String, Tag)>, custom_name: &str) {
    let value = Tag::Compound(vec![("text".into(), Tag::String(custom_name.to_owned()))]);
    let components = field_mut(fields, "components").and_then(|tag| match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    });
    if let Some(components) = components {
        set_field(components, CUSTOM_NAME_COMPONENT, value);
    } else {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(CUSTOM_NAME_COMPONENT.into(), value)]),
        );
    }
}

fn item_model_component(
    fields: &[(String, Tag)],
) -> Result<Option<Arc<Identifier>>, PlayerPersistenceError> {
    let Some(Tag::Compound(components)) = field(fields, "components") else {
        return Ok(None);
    };
    let Some(Tag::String(item_model)) = field(components, ITEM_MODEL_COMPONENT) else {
        return Ok(None);
    };
    Identifier::parse(item_model.clone())
        .map(Arc::new)
        .map(Some)
        .map_err(|_| PlayerPersistenceError::InvalidItemId(item_model.clone()))
}

fn set_item_model_component(fields: &mut Vec<(String, Tag)>, item_model: &Identifier) {
    let value = Tag::String(item_model.as_str().to_owned());
    let components = field_mut(fields, "components").and_then(|tag| match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    });
    if let Some(components) = components {
        set_field(components, ITEM_MODEL_COMPONENT, value);
    } else {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(ITEM_MODEL_COMPONENT.into(), value)]),
        );
    }
}

fn enchantments_component(
    fields: &[(String, Tag)],
) -> Result<Vec<mc_data::ItemEnchantment>, PlayerPersistenceError> {
    let Some(Tag::Compound(components)) = field(fields, "components") else {
        return Ok(Vec::new());
    };
    let Some(Tag::Compound(enchantments)) = field(components, ENCHANTMENTS_COMPONENT) else {
        return Ok(Vec::new());
    };
    let mut result = Vec::with_capacity(enchantments.len());
    for (id, level) in enchantments {
        let Tag::Int(level) = level else {
            return Err(PlayerPersistenceError::InvalidEnchantment(id.clone()));
        };
        let parsed = Identifier::parse(id.clone())
            .map_err(|_| PlayerPersistenceError::InvalidEnchantment(id.clone()))?;
        if !(1..=255).contains(level)
            || mc_data::required_registry_entry_id("enchantment", &parsed).is_none()
        {
            return Err(PlayerPersistenceError::InvalidEnchantment(id.clone()));
        }
        result.push(mc_data::ItemEnchantment {
            id: parsed,
            level: *level,
        });
    }
    result.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    Ok(result)
}

fn set_enchantments_component(
    fields: &mut Vec<(String, Tag)>,
    enchantments: &[mc_data::ItemEnchantment],
) {
    let value = Tag::Compound(
        enchantments
            .iter()
            .map(|enchantment| {
                (
                    enchantment.id.as_str().to_string(),
                    Tag::Int(enchantment.level),
                )
            })
            .collect(),
    );
    let components = field_mut(fields, "components").and_then(|tag| match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    });
    if let Some(components) = components {
        set_field(components, ENCHANTMENTS_COMPONENT, value);
    } else {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(ENCHANTMENTS_COMPONENT.into(), value)]),
        );
    }
}

fn is_modelled_item_key(name: &str) -> bool {
    matches!(name, "Slot" | "id" | "count")
}

fn field_mut<'a>(fields: &'a mut [(String, Tag)], name: &str) -> Option<&'a mut Tag> {
    fields
        .iter_mut()
        .find(|(key, _)| key == name)
        .map(|(_, tag)| tag)
}

fn set_field(fields: &mut Vec<(String, Tag)>, name: &str, value: Tag) {
    if let Some((_, existing)) = fields.iter_mut().find(|(key, _)| key == name) {
        *existing = value;
    } else {
        fields.push((name.into(), value));
    }
}

fn long_field(fields: &[(String, Tag)], name: &str) -> Option<i64> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(i64::from(*value)),
        Tag::Short(value) => Some(i64::from(*value)),
        Tag::Int(value) => Some(i64::from(*value)),
        Tag::Long(value) => Some(*value),
        _ => None,
    }
}

fn byte_field(fields: &[(String, Tag)], name: &str) -> Option<i8> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(*value),
        _ => None,
    }
}

fn float_field(fields: &[(String, Tag)], name: &str) -> Option<f32> {
    match field(fields, name)? {
        Tag::Float(value) => Some(*value),
        Tag::Double(value) => Some(*value as f32),
        _ => None,
    }
}

fn double_list<const N: usize>(tag: &Tag, len: usize) -> Option<[f64; N]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != len {
        return None;
    }
    let mut values = [0.0; N];
    for (idx, element) in list.elements.iter().enumerate() {
        values[idx] = match element {
            Tag::Double(value) => *value,
            Tag::Float(value) => f64::from(*value),
            _ => return None,
        };
    }
    Some(values)
}

fn float_list<const N: usize>(tag: &Tag, len: usize) -> Option<[f32; N]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != len {
        return None;
    }
    let mut values = [0.0; N];
    for (idx, element) in list.elements.iter().enumerate() {
        values[idx] = match element {
            Tag::Float(value) => *value,
            Tag::Double(value) => *value as f32,
            _ => return None,
        };
    }
    Some(values)
}

#[cfg(test)]
#[path = "persistence_entity_load_tests.rs"]
mod entity_load_tests;

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
