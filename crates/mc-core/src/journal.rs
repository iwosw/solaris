//! Tick journal: framed batches of absolute post-images + tombstones,
//! native-transaction checkpoints, recovery by replay (ADR 0013 "Durability").
//!
//! Layout under `<world_dir>/`:
//! - `journal/batch-<first_tick>-<last_tick>.bin`: framed batches. One batch
//!   holds whole ticks: `[magic u32][version u32][tick_lo u64][tick_hi u64][len u32][crc32 u32][payload]`.
//!   Payload is JSON lines of [`JournalEntry`]. Unpublished `.tmp` files are ignored.
//! - `native/state.redb`: one atomic checkpoint of chunks, entities, players,
//!   globals and the committed tick across all dimensions.
//!
//! Write-ahead rule: batches through checkpoint tick `c` are durable before
//! the native transaction starts. Only its Immediate commit makes pruning
//! batches at or below `c` safe; recovery folds only batches above that tick.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::boundary::{BlockPos, ChunkPos, EntityId};
use crate::config::CoreError;

const BATCH_MAGIC: u32 = 0x534F_4C4A;
const BATCH_VERSION: u32 = 1;
const HEADER_LEN: usize = 4 + 4 + 8 + 8 + 4 + 4;

/// One absolute post-image inside a batch. Replay is idempotent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JournalEntry {
    /// Native plugin custody, KV tombstones and exactly-once operation receipts.
    Plugins {
        value: Box<crate::plugin::PluginDelta>,
    },
    /// Dimension-qualified local post-images in the same global tick batch.
    Dimension {
        dimension: crate::dimensions::Dimension,
        entries: Vec<JournalEntry>,
    },
    EndFight {
        value: crate::end_fight::Fight,
    },
    Raids {
        #[serde(deserialize_with = "crate::raids::deserialize")]
        value: crate::raids::Raids,
    },
    WanderingTrader {
        value: crate::mob_lanes::TraderSpawner,
    },
    /// Absolute block state after the tick.
    Block {
        pos: [i32; 3],
        state: u32,
    },
    /// Absolute native scheduled work for one chunk, including empty clears.
    BlockTicks {
        image: crate::block_ticks::BlockTickChunkImage,
        next_sequence: u64,
    },
    FluidTicks {
        image: crate::fluids::FluidTicksImage,
    },
    /// A chunk entered/left the player-ticking set; unchanged intervals need
    /// no further records because the batch clock supplies elapsed ticks.
    InhabitedTime {
        image: InhabitedTimeImage,
    },
    /// 001 slice0 active-only debt clock (001-d/e, roadmap C/G5).
    /// Absolute per-chunk catch-up debt. Scope0 consumes with zero effects;
    /// the zero image is still journaled for exact-once durability.
    SimDebt {
        image: SimDebtImage,
    },
    /// Absolute world clock, weather duration and retained fade levels.
    Climate {
        value: crate::weather::ClimateImage,
    },
    Border {
        value: crate::command_border::Border,
    },
    Difficulty {
        value: crate::survival::Difficulty,
    },
    /// Canonical gamerule overrides, including rules without an active subsystem.
    GameRules {
        values: BTreeMap<String, String>,
    },
    /// Absolute scoreboard post-image after a scoreboard mutation.
    /// Once-only structure marker claim; retained even when its entity dies.
    GeneratedEntityClaim {
        claim: String,
    },
    Scoreboard {
        value: crate::scoreboard::Scoreboard,
    },
    /// Player post-image; omitted heavy fields retain the previous image.
    /// Inventory 41 + cursor + open crafting grid ride as separate stacks
    /// (real custody; never folded). Containers ride as block-entity
    /// post-images, not here.
    Player {
        uuid: String,
        #[serde(default)]
        opaque_player_fields: Vec<u8>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_advancement_progress"
        )]
        advancement_progress: Option<Arc<serde_json::Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unlocked_recipes: Option<Arc<BTreeSet<String>>>,
        #[serde(default)]
        statistics: Box<mc_data::statistics::PlayerStatistics>,
        #[serde(default)]
        dimension: crate::dimensions::Dimension,
        pos: [f64; 3],
        yaw: f32,
        pitch: f32,
        health: f32,
        food: i32,
        saturation: f32,
        exhaustion: f32,
        air: i32,
        experience_level: i32,
        experience_progress: f32,
        experience_total: i32,
        game_mode: u8,
        selected: u8,
        inventory: Vec<mc_data::ItemStack>,
        carried: mc_data::ItemStack,
        grid: Vec<mc_data::ItemStack>,
        /// Bed head or forced command-selected respawn cell.
        #[serde(default)]
        respawn: Option<[i32; 3]>,
        #[serde(default)]
        respawn_dimension: crate::dimensions::Dimension,
        #[serde(default)]
        respawn_forced: bool,
        #[serde(default)]
        respawn_yaw: f32,
        #[serde(default)]
        respawn_pitch: f32,
        #[serde(default)]
        active_effects: crate::effects::ActiveEffects,
        #[serde(default)]
        enchantment_seed: i32,
        #[serde(default)]
        warden_warning: crate::shrieker::Warning,
        #[serde(default, deserialize_with = "crate::mace::deserialize_impact")]
        current_impulse_impact_pos: Option<[f64; 3]>,
        #[serde(default)]
        fall_flying: bool,
        #[serde(default)]
        burn_ticks: u32,
        #[serde(default)]
        portal: crate::portals::PortalState,
        tick: u64,
    },
    /// Statistics-only absolute changes; idle timer progression needs no entry.
    Statistics {
        uuid: String,
        #[serde(deserialize_with = "deserialize_statistics_changes")]
        changes: Vec<(u32, u32, i32)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timers: Option<mc_data::statistics::StatisticsTimers>,
        tick: u64,
    },
    /// Block-entity post-image (chest/furnace slots + furnace progress).
    BlockEntity {
        pos: [i32; 3],
        slots: Vec<mc_data::ItemStack>,
        furnace: Option<FurnaceImage>,
        tick: u64,
    },
    /// Block-entity removal tombstone (container block broken): deletes the
    /// checkpoint base record on replay so the next checkpoint's injection
    /// (which clears first) cannot resurrect it.
    RemoveBlockEntity {
        pos: [i32; 3],
        tick: u64,
    },
    /// Opaque vanilla NBT (signs), or a removal tombstone.
    OpaqueBlockEntity {
        pos: [i32; 3],
        #[serde(deserialize_with = "crate::conduit::deserialize_nbt")]
        nbt: Option<Vec<u8>>,
    },
    /// Full mob/item/projectile post-image. Mob motion remains checkpoint-only
    /// (ADR 0013), except birth coordinates. Projectile motion is journaled
    /// every flight tick together with its custody/payload, so replay cannot
    /// rewind a missile across a hit or resurrect ammunition.
    /// Retained state shares its immutable allocation with the journal baseline;
    /// other large payloads are boxed. Serde keeps the existing representation.
    Entity {
        id: i32,
        uuid: String,
        type_id: u32,
        health: f32,
        alive: bool,
        age: u32,
        breed_cooldown: u32,
        pickup_delay: u32,
        item: Option<mc_data::ItemStack>,
        appearance: EntityAppearanceImage,
        #[serde(default)]
        active_effects: crate::effects::ActiveEffects,
        #[serde(default, deserialize_with = "deserialize_optional_villager_image")]
        villager: Option<Box<crate::villager::VillagerState>>,
        #[serde(default)]
        state: Arc<EntityStateImage>,
        #[serde(default, deserialize_with = "deserialize_optional_projectile_image")]
        projectile: Option<Box<crate::projectiles::ProjectileState>>,
        tick: u64,
        /// Birth coordinates for mobs; current coordinates for projectiles.
        pos: [f64; 3],
        /// Core blocks/second. Only projectile replay advances this value.
        #[serde(default)]
        vel: [f64; 3],
        /// Spawn yaw (same rule as `pos`).
        yaw: f32,
        /// Spawn pitch (same rule as `pos`).
        pitch: f32,
    },
    /// Compact absolute non-wire state; keeps timers out of appearance post-images.
    /// Shared with the comparison baseline, with unchanged serde representation.
    EntityState {
        id: i32,
        state: Arc<EntityStateImage>,
        tick: u64,
    },
    /// Health-only update (hurt, no spawn/appearance change): the full
    /// record (appearance, item, uuid) rides the spawn entry + checkpoint
    /// base. Keeps per-tick journal volume proportional to damage, not to
    /// appearance image size.
    EntityVitals {
        id: i32,
        health: f32,
        alive: bool,
        tick: u64,
    },
    /// Removal tombstone.
    RemoveEntity {
        id: i32,
        tick: u64,
    },
    /// Remote ticket state after the tick.
    Ticket {
        owner: String,
        chunk: [i32; 2],
        tick: u64,
        /// Chunk radius around the centre (default 0 = centre only; keeps
        /// pre-radius journals readable).
        #[serde(default)]
        radius: i32,
    },
    /// Absolute dimension-local settlement post-image plus monotonic high water.
    Settlement {
        image: crate::settlements::RecordImage,
        high_water: u64,
        tick: u64,
    },
}

fn deserialize_statistics_changes<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Vec<(u32, u32, i32)>, D::Error> {
    use mc_data::statistics::{CustomStat, StatType};
    let changes = Vec::<(u32, u32, i32)>::deserialize(decoder)?;
    if changes.iter().any(|&(kind, id, _)| {
        StatType::from_id(kind).is_none()
            || kind == StatType::Custom.id() && CustomStat::from_id(id).is_none()
            || id > i32::MAX as u32
    }) {
        return Err(serde::de::Error::custom("invalid statistics registry id"));
    }
    Ok(changes)
}

fn deserialize_optional_villager_image<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<Box<crate::villager::VillagerState>>, D::Error> {
    crate::villager::persistence::deserialize_optional_state(decoder)
        .map(|state| state.map(Box::new))
}

fn deserialize_optional_projectile_image<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<Box<crate::projectiles::ProjectileState>>, D::Error> {
    crate::projectiles::persistence::deserialize_optional_state(decoder)
        .map(|state| state.map(Box::new))
}

/// Fsync one file path (file must exist).
pub fn fsync_file(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

/// Fsync a directory entry. On Linux opening a directory as a `File` and
/// syncing it fsyncs the directory entry; elsewhere this is a no-op.
pub fn fsync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Append batches to the journal on the journal thread. Every batch file is
/// fsynced when written; the directory every `fsync_every` appends (call
/// [`JournalWriter::flush`] to force).
pub struct JournalWriter {
    dir: PathBuf,
    pending_fsync: u32,
    fsync_every: u32,
    /// Ticks `lo..=hi` appended contiguously since open: the WAL gate
    /// checks coverage here instead of re-reading batch files.
    appended: Option<(u64, u64)>,
    /// Immutable pre-open coverage `(checkpoint_tick, recovered_tick]`, decoded
    /// either by startup recovery or the first checkpoint gate.
    pre_open_verified: Option<(u64, u64)>,
}

impl JournalWriter {
    pub fn open(world_dir: &Path, fsync_every: u32) -> Result<Self, CoreError> {
        let dir = world_dir.join("journal");
        std::fs::create_dir_all(&dir).map_err(|err| CoreError::Storage(err.to_string()))?;
        Ok(Self {
            dir,
            pending_fsync: 0,
            fsync_every: fsync_every.max(1),
            appended: None,
            pre_open_verified: None,
        })
    }

    /// Reuse the contiguous tail already validated while holding the world lease.
    /// Published batches are immutable; the checkpoint gate still fsyncs the
    /// directory and verifies every tick appended by this writer.
    pub(crate) fn open_recovered(
        world_dir: &Path,
        fsync_every: u32,
        checkpoint_tick: u64,
        recovered_tick: u64,
    ) -> Result<Self, CoreError> {
        let mut writer = Self::open(world_dir, fsync_every)?;
        writer.pre_open_verified = Some((checkpoint_tick, recovered_tick));
        Ok(writer)
    }

    /// Write-ahead gate for checkpoint `through`: every tick in
    /// `(prev_tick, through]` is in a durable batch. Appends since open
    /// are contiguous (an append failure ends the journal), so coverage is
    /// tracked in process; batches from before open are verified on disk
    /// once. Batch files are fsynced at append, so only the directory
    /// needs a final fsync.
    pub fn gate(&mut self, prev_tick: u64, through: u64) -> Result<(), CoreError> {
        // Recovery may already cover the whole save without a new tick/append.
        if through == prev_tick
            || self
                .pre_open_verified
                .is_some_and(|(from, hi)| from <= prev_tick && hi >= through)
        {
            return self.flush();
        }
        let hole = |from: u64| {
            Err(CoreError::Storage(format!(
                "journal hole: ticks {from}..={through} have no batch"
            )))
        };
        let first_appended = match self.appended {
            Some((lo, hi)) if hi >= through => lo,
            Some((_, hi)) => return hole(hi + 1),
            None => return hole(prev_tick + 1),
        };
        if first_appended > prev_tick + 1
            && !self
                .pre_open_verified
                .is_some_and(|(from, through)| from <= prev_tick && through >= first_appended - 1)
        {
            verify_batches_through(&self.dir, prev_tick, first_appended - 1)?;
            self.pre_open_verified = Some((prev_tick, first_appended - 1));
        }
        self.flush()
    }

    /// Append one batch covering ticks `tick_lo..=tick_hi`.
    pub fn append(
        &mut self,
        tick_lo: u64,
        tick_hi: u64,
        entries: &[JournalEntry],
    ) -> Result<(), CoreError> {
        if let Some((_, hi)) = self.appended
            && tick_lo != hi + 1
        {
            return Err(CoreError::Storage(format!(
                "journal append {tick_lo} does not follow {hi}"
            )));
        }
        // Test hook (unit tests only): fail EVERY append for the armed
        // world dir while armed — per-dir so parallel tests never poison
        // each other's journals (process-global hook, same pattern as
        // FAIL_CHECKPOINT_AFTER_REGIONS).
        #[cfg(test)]
        if crate::core::FAIL_JOURNAL_APPEND
            .lock()
            .ok()
            .and_then(|armed| armed.clone())
            .is_some_and(|dir| *dir == self.dir.parent().unwrap_or(&self.dir).to_path_buf())
        {
            return Err(CoreError::Storage(
                "injected journal append failure".to_owned(),
            ));
        }
        let mut payload = String::new();
        for entry in entries {
            if let JournalEntry::Entity {
                villager: Some(state),
                ..
            } = entry
            {
                crate::villager::persistence::validate_state(state)
                    .map_err(|error| CoreError::Storage(error.to_string()))?;
            }
            if let JournalEntry::Entity {
                projectile: Some(state),
                pos,
                vel,
                yaw,
                pitch,
                ..
            } = entry
            {
                crate::projectiles::persistence::validate_state(state)
                    .map_err(|error| CoreError::Storage(error.to_owned()))?;
                if pos.iter().chain(vel.iter()).any(|value| !value.is_finite())
                    || !yaw.is_finite()
                    || !pitch.is_finite()
                {
                    return Err(CoreError::Storage("nonfinite projectile motion".into()));
                }
            }
            payload.push_str(
                &serde_json::to_string(entry).map_err(|err| CoreError::Storage(err.to_string()))?,
            );
            payload.push('\n');
        }
        let bytes = payload.into_bytes();
        let crc = crc32fast::hash(&bytes);
        let mut frame = Vec::with_capacity(HEADER_LEN + bytes.len());
        frame.extend_from_slice(&BATCH_MAGIC.to_le_bytes());
        frame.extend_from_slice(&BATCH_VERSION.to_le_bytes());
        frame.extend_from_slice(&tick_lo.to_le_bytes());
        frame.extend_from_slice(&tick_hi.to_le_bytes());
        frame.extend_from_slice(
            &u32::try_from(bytes.len())
                .map_err(|_| CoreError::Storage("batch too large".to_owned()))?
                .to_le_bytes(),
        );
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(&bytes);
        let name = format!("batch-{tick_lo:020}-{tick_hi:020}.bin");
        let tmp = self.dir.join(format!("{name}.tmp"));
        let final_path = self.dir.join(&name);
        {
            let mut file = File::create(&tmp).map_err(|err| CoreError::Storage(err.to_string()))?;
            file.write_all(&frame)
                .map_err(|err| CoreError::Storage(err.to_string()))?;
            file.sync_all()
                .map_err(|err| CoreError::Storage(err.to_string()))?;
        }
        std::fs::rename(&tmp, &final_path).map_err(|err| CoreError::Storage(err.to_string()))?;
        self.appended = Some((self.appended.map_or(tick_lo, |(lo, _)| lo), tick_hi));
        self.pending_fsync += 1;
        if self.pending_fsync >= self.fsync_every {
            self.flush()?;
        }
        Ok(())
    }

    /// Fsync the journal directory so all appended batches are durable.
    pub fn flush(&mut self) -> Result<(), CoreError> {
        fsync_dir(&self.dir).map_err(|err| CoreError::Storage(err.to_string()))?;
        self.pending_fsync = 0;
        Ok(())
    }
}

/// Parse `batch-<lo>-<hi>.bin`.
fn parse_batch_name(name: &str) -> Option<(u64, u64)> {
    let rest = name.strip_prefix("batch-")?.strip_suffix(".bin")?;
    let (lo, hi) = rest.split_once('-')?;
    Some((lo.parse().ok()?, hi.parse().ok()?))
}

/// Read complete published batches after the manifest, failing closed on damage.
/// Incomplete, unpublished `.tmp` files are ignored.
pub fn read_complete_batches_after(
    journal_dir: &Path,
    manifest_tick: u64,
) -> Result<Vec<(u64, u64, Vec<JournalEntry>)>, CoreError> {
    let mut out = Vec::new();
    visit_complete_batches_after(journal_dir, manifest_tick, |batch| {
        out.push(batch);
        Ok(())
    })?;
    Ok(out)
}

/// Decode and consume one published batch at a time. Recovery memory is bounded
/// by one batch and the latest recovered images, never the complete WAL history.
pub(crate) fn visit_complete_batches_after(
    journal_dir: &Path,
    manifest_tick: u64,
    mut visit: impl FnMut((u64, u64, Vec<JournalEntry>)) -> Result<(), CoreError>,
) -> Result<u64, CoreError> {
    let entries = match std::fs::read_dir(journal_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(manifest_tick),
        Err(err) => return Err(CoreError::Storage(err.to_string())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| CoreError::Storage(err.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((lo, hi)) = parse_batch_name(&name) {
            names.push((lo, hi, name));
        }
    }
    names.sort();
    let mut previous_tick = manifest_tick;
    for (lo, hi, name) in &names {
        if *hi <= manifest_tick {
            continue;
        }
        let path = journal_dir.join(name);
        let expected = previous_tick.checked_add(1).ok_or_else(|| {
            CoreError::Storage(format!("journal {}: tick range overflow", path.display()))
        })?;
        if *lo > expected {
            return Err(CoreError::Storage(format!(
                "journal {}: journal hole: ticks {expected}..={} have no batch",
                path.display(),
                lo - 1,
            )));
        }
        if *lo != expected {
            return Err(CoreError::Storage(format!(
                "journal {}: noncontiguous batch starts at {lo}, expected {expected}",
                path.display(),
            )));
        }
        let bytes = std::fs::read(&path)
            .map_err(|err| CoreError::Storage(format!("journal {}: {err}", path.display())))?;
        let entries = decode_batch(&bytes, *lo, *hi).map_err(|err| err.at_path(&path))?;
        drop(bytes);
        visit((*lo, *hi, entries))?;
        previous_tick = *hi;
    }
    Ok(previous_tick)
}
/// Verify the WAL covers `(prev_tick, through]` contiguously: every tick in
/// the range must sit inside some complete batch file. A missing batch (a
/// failed append the tick thread never retried) is a hole no fsync can
/// fill — the caller must refuse the checkpoint ack. Torn tails fail too.
pub fn verify_batches_through(
    journal_dir: &Path,
    prev_tick: u64,
    through: u64,
) -> Result<(), CoreError> {
    let mut intervals: Vec<(u64, u64)> = Vec::new();
    let entries =
        std::fs::read_dir(journal_dir).map_err(|err| CoreError::Storage(err.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|err| CoreError::Storage(err.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("batch-") && name.ends_with(".bin")) {
            continue;
        }
        let Some((lo, hi)) = parse_batch_name(&name) else {
            continue;
        };
        if hi <= prev_tick || lo > through {
            continue;
        }
        let bytes = std::fs::read(journal_dir.join(&name))
            .map_err(|err| CoreError::Storage(err.to_string()))?;
        decode_batch(&bytes, lo, hi).map_err(|err| err.at_path(&journal_dir.join(&name)))?;
        intervals.push((lo, hi));
    }
    intervals.sort();
    let mut covered = prev_tick;
    for (lo, hi) in intervals {
        if lo > covered + 1 {
            return Err(CoreError::Storage(format!(
                "journal hole: ticks {}..={} have no batch",
                covered + 1,
                lo - 1
            )));
        }
        covered = covered.max(hi);
        if covered >= through {
            break;
        }
    }
    if covered < through {
        return Err(CoreError::Storage(format!(
            "journal hole: ticks {}..={through} have no batch",
            covered + 1
        )));
    }
    Ok(())
}
#[derive(Debug)]
enum BatchDecodeError {
    Framing(String),
    Record {
        index: usize,
        offset: usize,
        kind: Option<String>,
        source: serde_json::Error,
    },
}

impl BatchDecodeError {
    fn at_path(self, path: &Path) -> CoreError {
        let detail = match self {
            Self::Framing(reason) => {
                format!("record 0 offset 0: {reason}")
            }
            Self::Record {
                index,
                offset,
                kind,
                source,
            } => format!(
                "record {index} offset {offset} kind {}: {source}",
                kind.as_deref().unwrap_or("unknown")
            ),
        };
        CoreError::Storage(format!("journal {}: {detail}", path.display()))
    }
}

fn decode_batch(
    bytes: &[u8],
    expect_lo: u64,
    expect_hi: u64,
) -> Result<Vec<JournalEntry>, BatchDecodeError> {
    if bytes.len() < HEADER_LEN {
        return Err(BatchDecodeError::Framing("incomplete batch header".into()));
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let lo = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let hi = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let len = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
    let crc = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
    if magic != BATCH_MAGIC || version != BATCH_VERSION || lo != expect_lo || hi != expect_hi {
        return Err(BatchDecodeError::Framing(format!(
            "invalid header: magic {magic:#x}, version {version}, ticks {lo}..={hi}, expected {expect_lo}..={expect_hi}"
        )));
    }
    let actual_len = bytes.len() - HEADER_LEN;
    if actual_len < len {
        return Err(BatchDecodeError::Framing(format!(
            "incomplete payload: expected {len} bytes, found {actual_len}"
        )));
    }
    if actual_len > len {
        return Err(BatchDecodeError::Framing(format!(
            "trailing bytes: expected {len} payload bytes, found {actual_len}"
        )));
    }
    let payload = &bytes[HEADER_LEN..];
    let actual_crc = crc32fast::hash(payload);
    if actual_crc != crc {
        return Err(BatchDecodeError::Framing(format!(
            "CRC mismatch: expected {crc:#x}, found {actual_crc:#x}"
        )));
    }
    let mut entries = Vec::new();
    let mut offset = HEADER_LEN;
    for (index, line) in payload.split(|byte| *byte == b'\n').enumerate() {
        if !line.is_empty() {
            let entry = serde_json::from_slice(line).map_err(|source| {
                let kind = serde_json::from_slice::<serde_json::Value>(line)
                    .ok()
                    .and_then(|value| match value {
                        serde_json::Value::Object(object) if object.len() == 1 => {
                            object.into_iter().next().map(|(kind, _)| kind)
                        }
                        serde_json::Value::String(kind) => Some(kind),
                        _ => None,
                    })
                    .or_else(|| {
                        // An incomplete object may still expose its enum tag.
                        let colon = line.iter().position(|byte| *byte == b':')?;
                        let key = std::str::from_utf8(&line[..colon]).ok()?.trim();
                        serde_json::from_str::<String>(key.strip_prefix('{')?.trim()).ok()
                    });
                BatchDecodeError::Record {
                    index,
                    offset,
                    kind,
                    source,
                }
            })?;
            entries.push(entry);
        }
        offset += line.len() + 1;
    }
    Ok(entries)
}

/// Absolute counter at an interval boundary. Active intervals advance through
/// the recovered WAL tick, not wall time or the next player's join tick.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct InhabitedTimeImage {
    pub chunk: [i32; 2],
    pub value: i64,
    pub tick: u64,
    pub active: bool,
}

impl InhabitedTimeImage {
    pub(crate) fn value_at(self, tick: u64) -> i64 {
        let elapsed = if self.active {
            tick.saturating_sub(self.tick)
        } else {
            0
        };
        self.value
            .saturating_add(i64::try_from(elapsed).unwrap_or(i64::MAX))
    }

    pub(crate) fn apply(self, chunk: &mut mc_world::Chunk, tick: u64) {
        let value = self.value_at(tick);
        chunk.extras.retain(|(key, _)| key != "InhabitedTime");
        chunk
            .extras
            .push(("InhabitedTime".into(), mc_nbt::Tag::Long(value)));
    }
}

/// 001 slice0 active-only per-chunk debt clock (001-d/e).
/// Absolute catch-up debt at a WAL/checkpoint tick. `last_sim` advances only
/// while the chunk is active+resident; `remaining` holds pre-existing debt
/// consumed on activation (scope0: zero effects, durable zero image).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SimDebtImage {
    pub chunk: [i32; 2],
    pub last_sim: u64,
    pub remaining: u64,
    pub tick: u64,
    /// Open interval at this image's tick mirrors
    /// [`InhabitedTimeImage::active`]: the recovered batch clock supplies
    /// elapsed ticks (`last_sim_at`), never wall time or re-accrual.
    pub active: bool,
}

impl SimDebtImage {
    pub fn last_sim_at(self, tick: u64) -> u64 {
        if self.active {
            self.last_sim.max(tick)
        } else {
            self.last_sim
        }
    }
}

/// Native checkpoint image and durable player record ticks.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CheckpointManifest {
    pub tick: u64,
    /// Required native checkpoint subdomain schema versions.
    pub domain_versions: BTreeMap<String, u32>,
    #[serde(default)]
    pub dimensions: BTreeMap<crate::dimensions::Dimension, Box<CheckpointManifest>>,
    #[serde(default)]
    pub end_fight: Option<crate::end_fight::Fight>,
    #[serde(default)]
    pub raids: Option<crate::raids::Raids>,
    #[serde(default)]
    pub wandering_trader: Option<crate::mob_lanes::TraderSpawner>,
    /// Durable ticket post-images by owner: centre, (capture tick, radius).
    /// Portal transfer tickets are transient and excluded from checkpoint captures.
    #[serde(default)]
    pub tickets: BTreeMap<String, ([i32; 2], (u64, i32))>,
    /// Player UUID -> durable native record tick, including offline players.
    #[serde(default)]
    pub players: std::collections::BTreeMap<String, u64>,
    /// Generated world spawn block coordinates; absent only in old manifests.
    #[serde(default)]
    pub world_spawn: Option<[i32; 3]>,
    #[serde(default)]
    pub world_spawn_rotation: [f32; 2],
    #[serde(default)]
    pub generated_entity_claims: std::collections::BTreeSet<String>,
    /// Captured together with the checkpoint tick; never rebuilt from it.
    #[serde(default)]
    pub climate: Option<crate::weather::ClimateImage>,
    #[serde(default)]
    pub border: crate::command_border::Border,
    #[serde(default)]
    pub difficulty: crate::survival::Difficulty,
    /// Absolute deadlines keep the scheduler clock stable across restart.
    #[serde(default)]
    pub block_ticks: crate::block_ticks::BlockTicksImage,
    #[serde(default)]
    pub fluid_ticks: crate::fluids::FluidTicksImage,
    /// Open inhabited-time intervals at the same tick as the chunk capture.
    #[serde(default)]
    pub inhabited_time: Vec<InhabitedTimeImage>,
    /// 001 slice0 per-chunk debt images at the checkpoint tick; only chunks
    /// with pending (dirty) or nonzero debt are listed.
    #[serde(default)]
    pub debt: Vec<SimDebtImage>,
    #[serde(default)]
    pub gamerules: BTreeMap<String, String>,
}

/// Prune batches fully at or below `manifest_tick`.
pub fn prune_batches_through(journal_dir: &Path, manifest_tick: u64) {
    let Ok(entries) = std::fs::read_dir(journal_dir) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("batch-") && name.ends_with(".bin")) {
            continue;
        }
        if let Some((_, hi)) = parse_batch_name(&name)
            && hi <= manifest_tick
        {
            let _ = std::fs::remove_file(journal_dir.join(&name));
        }
    }
}

/// Compact player post-image shared by the journal and checkpoints.
pub type PlayerImage = ([f64; 3], f32, f32, f32, u8, u64);

/// Furnace progress image for block-entity post-images.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
pub struct FurnaceImage {
    pub lit_remaining: i32,
    pub lit_total: i32,
    pub cook_progress: i32,
    pub cook_total: i32,
}

/// Replayed full player record.
#[derive(Debug, Clone)]
pub struct ReplayPlayer {
    /// Unknown player root fields, encoded as a named NBT compound.
    pub opaque_player_fields: Vec<u8>,
    pub statistics: mc_data::statistics::PlayerStatistics,
    /// Exact vanilla advancements document, including unknown keys.
    pub advancement_progress: Arc<serde_json::Value>,
    pub unlocked_recipes: Arc<BTreeSet<String>>,
    pub dimension: crate::dimensions::Dimension,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub exhaustion: f32,
    pub air: i32,
    pub experience_level: i32,
    pub experience_progress: f32,
    pub experience_total: i32,
    pub game_mode: u8,
    pub selected: u8,
    pub inventory: Vec<mc_data::ItemStack>,
    /// Cursor stack custody (never folded away).
    pub carried: mc_data::ItemStack,
    /// Open menu crafting grid custody (never folded away).
    pub grid: Vec<mc_data::ItemStack>,
    /// Bed head or forced command-selected respawn cell.
    pub respawn: Option<[i32; 3]>,
    pub respawn_dimension: crate::dimensions::Dimension,
    pub respawn_forced: bool,
    pub respawn_yaw: f32,
    pub respawn_pitch: f32,
    pub active_effects: crate::effects::ActiveEffects,
    pub enchantment_seed: i32,
    pub warden_warning: crate::shrieker::Warning,
    pub current_impulse_impact_pos: Option<[f64; 3]>,
    pub fall_flying: bool,
    pub burn_ticks: u32,
    pub portal: crate::portals::PortalState,
    pub tick: u64,
}

impl PartialEq for ReplayPlayer {
    fn eq(&self, other: &Self) -> bool {
        self.eq_except_statistics(other) && self.statistics == other.statistics
    }
}

impl ReplayPlayer {
    /// Statistics have their own compact WAL path; every other durable field
    /// still selects the complete Player postimage.
    pub(crate) fn eq_except_statistics(&self, other: &Self) -> bool {
        // No `..`: adding a durable field must also update this comparison.
        let Self {
            opaque_player_fields: _,
            statistics: _,
            advancement_progress: _,
            unlocked_recipes: _,
            dimension: _,
            pos: _,
            yaw: _,
            pitch: _,
            health: _,
            food: _,
            saturation: _,
            exhaustion: _,
            air: _,
            experience_level: _,
            experience_progress: _,
            experience_total: _,
            game_mode: _,
            selected: _,
            inventory: _,
            carried: _,
            grid: _,
            respawn: _,
            respawn_dimension: _,
            respawn_forced: _,
            respawn_yaw: _,
            respawn_pitch: _,
            active_effects: _,
            enchantment_seed: _,
            warden_warning: _,
            current_impulse_impact_pos: _,
            fall_flying: _,
            burn_ticks: _,
            portal: _,
            tick: _,
        } = self;
        (Arc::ptr_eq(&self.advancement_progress, &other.advancement_progress)
            || self.advancement_progress == other.advancement_progress)
            && (Arc::ptr_eq(&self.unlocked_recipes, &other.unlocked_recipes)
                || self.unlocked_recipes == other.unlocked_recipes)
            && self.opaque_player_fields == other.opaque_player_fields
            && self.dimension == other.dimension
            && self.pos == other.pos
            && self.yaw == other.yaw
            && self.pitch == other.pitch
            && self.health == other.health
            && self.food == other.food
            && self.saturation == other.saturation
            && self.exhaustion == other.exhaustion
            && self.air == other.air
            && self.experience_level == other.experience_level
            && self.experience_progress == other.experience_progress
            && self.experience_total == other.experience_total
            && self.game_mode == other.game_mode
            && self.selected == other.selected
            && self.inventory == other.inventory
            && self.carried == other.carried
            && self.grid == other.grid
            && self.respawn == other.respawn
            && self.respawn_dimension == other.respawn_dimension
            && self.respawn_forced == other.respawn_forced
            && self.respawn_yaw == other.respawn_yaw
            && self.respawn_pitch == other.respawn_pitch
            && self.active_effects == other.active_effects
            && self.enchantment_seed == other.enchantment_seed
            && self.warden_warning == other.warden_warning
            && self.current_impulse_impact_pos == other.current_impulse_impact_pos
            && self.fall_flying == other.fall_flying
            && self.burn_ticks == other.burn_ticks
            && self.portal == other.portal
            && self.tick == other.tick
    }
}

pub(crate) fn empty_advancement_progress() -> Arc<serde_json::Value> {
    Arc::new(serde_json::Value::Object(serde_json::Map::new()))
}

// A present JSON null is retained document data, not an omitted post-image.
fn deserialize_advancement_progress<'de, D>(
    deserializer: D,
) -> Result<Option<Arc<serde_json::Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(|value| Some(Arc::new(value)))
}
/// Replayed block-entity record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ReplayBlockEntity {
    pub slots: Vec<mc_data::ItemStack>,
    pub furnace: Option<FurnaceImage>,
    pub tick: u64,
}

/// Journal-safe image of [`EntityAppearance`](crate::boundary::EntityAppearance):
/// all non-motion entity state (carried item, equipment, baby, pose, flags,
/// health, custom name, variant data). `ItemStack` round-trips through JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
pub struct EntityAppearanceImage {
    pub item: Option<mc_data::ItemStack>,
    #[serde(default)]
    pub experience_orb: Option<(i32, i32)>,
    #[serde(default)]
    pub spawn_data: i32,
    pub equipment: Vec<Option<mc_data::ItemStack>>,
    pub baby: bool,
    pub pose: u8,
    pub shared_flags: u8,
    pub health: f32,
    pub custom_name: Option<String>,
    #[serde(default)]
    pub custom_name_visible: bool,
    pub extra: Vec<(u8, EntityDataValueImage)>,
}

/// Journal-safe image of [`EntityDataValue`](crate::boundary::EntityDataValue).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum EntityDataValueImage {
    Byte(i8),
    VarInt(i32),
    Float(f32),
    Rotations([f32; 3]),
    VarLong(i64),
    Boolean(bool),
    String(String),
    BlockState(u32),
    WeatheringCopper(u8),
    CopperGolemState(u8),
    Item(Option<mc_data::ItemStack>),
    OptionalBlockPos(Option<[i32; 3]>),
    EntityEffectParticle(i32),
    OptionalUnsignedInt(Option<u32>),
    OptionalLivingEntityReference(Option<uuid::Uuid>),
    VillagerData {
        kind: i32,
        profession: i32,
        level: i32,
    },
}

impl From<&crate::boundary::EntityAppearance> for EntityAppearanceImage {
    fn from(appearance: &crate::boundary::EntityAppearance) -> Self {
        Self {
            item: appearance.item.clone(),
            experience_orb: appearance.experience_orb,
            spawn_data: appearance.spawn_data,
            equipment: appearance.equipment.to_vec(),
            baby: appearance.baby,
            pose: appearance.pose,
            shared_flags: appearance.shared_flags,
            health: appearance.health,
            custom_name: appearance.custom_name.clone(),
            custom_name_visible: appearance.custom_name_visible,
            extra: appearance
                .extra
                .iter()
                .map(|(index, value)| (*index, EntityDataValueImage::from(value)))
                .collect(),
        }
    }
}

impl From<&crate::boundary::EntityDataValue> for EntityDataValueImage {
    fn from(value: &crate::boundary::EntityDataValue) -> Self {
        match value {
            crate::boundary::EntityDataValue::Byte(v) => Self::Byte(*v),
            crate::boundary::EntityDataValue::VarInt(v) => Self::VarInt(*v),
            crate::boundary::EntityDataValue::VarLong(v) => Self::VarLong(*v),
            crate::boundary::EntityDataValue::Float(v) => Self::Float(*v),
            crate::boundary::EntityDataValue::Rotations(v) => Self::Rotations(*v),
            crate::boundary::EntityDataValue::Boolean(v) => Self::Boolean(*v),
            crate::boundary::EntityDataValue::WeatheringCopper(v) => Self::WeatheringCopper(*v),
            crate::boundary::EntityDataValue::CopperGolemState(v) => Self::CopperGolemState(*v),
            crate::boundary::EntityDataValue::String(v) => Self::String(v.clone()),
            crate::boundary::EntityDataValue::BlockState(v) => Self::BlockState(*v),
            crate::boundary::EntityDataValue::Item(v) => Self::Item(v.clone()),
            crate::boundary::EntityDataValue::OptionalBlockPos(v) => Self::OptionalBlockPos(*v),
            crate::boundary::EntityDataValue::EntityEffectParticle(v) => {
                Self::EntityEffectParticle(*v)
            }
            crate::boundary::EntityDataValue::OptionalUnsignedInt(v) => {
                Self::OptionalUnsignedInt(*v)
            }
            crate::boundary::EntityDataValue::OptionalLivingEntityReference(v) => {
                Self::OptionalLivingEntityReference(*v)
            }
            crate::boundary::EntityDataValue::VillagerData {
                kind,
                profession,
                level,
            } => Self::VillagerData {
                kind: *kind,
                profession: *profession,
                level: *level,
            },
        }
    }
}

/// Encode a complete player image without touching filesystem state.
pub(crate) fn player_to_tag(
    record: &ReplayPlayer,
    items: &mc_data::items::ItemRegistry,
) -> Result<mc_nbt::Tag, CoreError> {
    use mc_data::player_nbt::{VanillaPlayer, player_to_fields};
    let mut opaque_fields = if record.opaque_player_fields.is_empty() {
        Vec::new()
    } else {
        let mut cursor = std::io::Cursor::new(&record.opaque_player_fields);
        let (_, tag) = mc_nbt::read_named(&mut cursor)
            .map_err(|err| CoreError::Storage(format!("player opaque fields: {err}")))?;
        if cursor.position() != record.opaque_player_fields.len() as u64 {
            return Err(CoreError::Storage(
                "player opaque fields: trailing NBT bytes".into(),
            ));
        }
        let mc_nbt::Tag::Compound(fields) = tag else {
            return Err(CoreError::Storage(
                "player opaque fields: expected compound".into(),
            ));
        };
        fields
    };
    // Exclude every owned key, even when its fresh optional value is absent.
    opaque_fields.retain(|(key, _)| !player_owned_field(key));
    let mut inventory = Vec::new();
    let mut equipment = Vec::new();
    for (index, stack) in record.inventory.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        if let Some(slot) = v2_slot_to_storage(index) {
            inventory.push((slot, stack.clone()));
        } else if let Some(name) = v2_slot_to_equipment(index) {
            equipment.push((name.to_owned(), stack.clone()));
        }
    }
    let player = VanillaPlayer {
        pos: record.pos,
        yaw: record.yaw,
        pitch: record.pitch,
        health: record.health,
        food: record.food,
        saturation: record.saturation,
        exhaustion: record.exhaustion,
        air: record.air,
        selected: record.selected,
        xp_level: record.experience_level,
        xp_progress: record.experience_progress,
        xp_total: record.experience_total,
        game_mode: i32::from(record.game_mode),
        inventory,
        equipment,
        carried: record.carried.clone(),
        grid: record.grid.clone(),
    };
    let mut fields =
        player_to_fields(items, &player).map_err(|err| CoreError::Storage(err.to_string()))?;
    fields.push((
        "Dimension".into(),
        mc_nbt::Tag::String(record.dimension.name().into()),
    ));
    fields.extend(active_effects_to_fields(&record.active_effects));
    fields.extend(crate::portals::fields(record.portal));
    fields.push(("XpSeed".into(), mc_nbt::Tag::Int(record.enchantment_seed)));
    let mut warning = record.warden_warning;
    warning.advance_tick(record.tick);
    fields.push(("warden_spawn_tracker".into(), warning.nbt()));
    fields.push((
        "SolarisWardenWarning".into(),
        mc_nbt::Tag::ByteArray(
            serde_json::to_vec(&warning)
                .map_err(|err| CoreError::Storage(format!("player warning: {err}")))?
                .into_iter()
                .map(|byte| byte as i8)
                .collect(),
        ),
    ));
    if let Some(pos) = record.current_impulse_impact_pos {
        fields.push((
            "current_explosion_impact_pos".into(),
            mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::DOUBLE,
                elements: pos.into_iter().map(mc_nbt::Tag::Double).collect(),
            }),
        ));
    }
    fields.push((
        "FallFlying".into(),
        mc_nbt::Tag::Byte(i8::from(record.fall_flying)),
    ));
    fields.push((
        "Fire".into(),
        mc_nbt::Tag::Short(i16::try_from(record.burn_ticks).unwrap_or(i16::MAX)),
    ));
    // Vanilla `DataVersion` (26.1.2 world version, shared with chunk/entity
    // writers) so vanilla runs the right DataFixer chain on our files.
    fields.push((
        "DataVersion".into(),
        mc_nbt::Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
    ));
    fields.push((
        "SolarisStatistics".into(),
        mc_nbt::Tag::ByteArray(
            serde_json::to_vec(&record.statistics)
                .map_err(|err| CoreError::Storage(format!("player statistics: {err}")))?
                .into_iter()
                .map(|byte| byte as i8)
                .collect(),
        ),
    ));
    // Solaris custody keys (vanilla ignores unknown keys).
    for (key, value) in
        solaris_custody_fields(items, &player).map_err(|err| CoreError::Storage(err.to_string()))?
    {
        fields.push((key, value));
    }
    if let Some(position) = record.respawn {
        fields.push((
            "respawn".into(),
            mc_nbt::Tag::Compound(vec![
                (
                    "dimension".into(),
                    mc_nbt::Tag::String(record.respawn_dimension.name().into()),
                ),
                ("pos".into(), mc_nbt::Tag::IntArray(position.to_vec())),
                ("yaw".into(), mc_nbt::Tag::Float(record.respawn_yaw)),
                ("pitch".into(), mc_nbt::Tag::Float(record.respawn_pitch)),
                (
                    "forced".into(),
                    mc_nbt::Tag::Byte(i8::from(record.respawn_forced)),
                ),
            ]),
        ));
    }
    if record.respawn.is_none() {
        fields.push((
            "solaris:RespawnState".into(),
            mc_nbt::Tag::Compound(vec![
                (
                    "dimension".into(),
                    mc_nbt::Tag::String(record.respawn_dimension.name().into()),
                ),
                ("yaw".into(), mc_nbt::Tag::Float(record.respawn_yaw)),
                ("pitch".into(), mc_nbt::Tag::Float(record.respawn_pitch)),
                (
                    "forced".into(),
                    mc_nbt::Tag::Byte(i8::from(record.respawn_forced)),
                ),
            ]),
        ));
    }
    fields.push(("solaris:Tick".into(), mc_nbt::Tag::Long(record.tick as i64)));
    fields.push((
        "solaris:BurnTicks".into(),
        mc_nbt::Tag::Long(i64::from(record.burn_ticks)),
    ));
    fields.push((
        "SolarisAdvancements".into(),
        // Vanilla's complete document can exceed NBT's 64 KiB string limit.
        mc_nbt::Tag::ByteArray(
            serde_json::to_vec(&record.advancement_progress)
                .map_err(|err| CoreError::Storage(format!("player advancements: {err}")))?
                .into_iter()
                .map(|byte| byte as i8)
                .collect(),
        ),
    ));
    fields.push((
        "SolarisUnlockedRecipes".into(),
        mc_nbt::Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::STRING,
            elements: record
                .unlocked_recipes
                .iter()
                .cloned()
                .map(mc_nbt::Tag::String)
                .collect(),
        }),
    ));
    fields.extend(opaque_fields);
    Ok(mc_nbt::Tag::Compound(fields))
}

/// v2 inventory index -> vanilla storage slot (0-35). Armor (36-39) and
/// offhand (40) ride the vanilla `equipment` compound, never `Inventory`.
fn v2_slot_to_storage(index: usize) -> Option<u8> {
    match index {
        0..=35 => Some(index as u8),
        _ => None,
    }
}

/// Vanilla storage slot (0-35) -> v2 inventory index.
fn storage_slot_to_v2(slot: u8) -> Option<usize> {
    match slot {
        0..=35 => Some(usize::from(slot)),
        _ => None,
    }
}

/// v2 armor/offhand index -> vanilla `equipment` slot name.
fn v2_slot_to_equipment(index: usize) -> Option<&'static str> {
    match index {
        36 => Some("feet"),
        37 => Some("legs"),
        38 => Some("chest"),
        39 => Some("head"),
        40 => Some("offhand"),
        _ => None,
    }
}

/// Vanilla `equipment` slot name -> v2 inventory index.
fn equipment_to_v2_slot(name: &str) -> Option<usize> {
    match name {
        "feet" => Some(36),
        "legs" => Some(37),
        "chest" => Some(38),
        "head" => Some(39),
        "offhand" => Some(40),
        _ => None,
    }
}

/// Solaris custody keys: cursor + crafting grid as vanilla item compounds.
fn solaris_custody_fields(
    items: &mc_data::items::ItemRegistry,
    player: &mc_data::player_nbt::VanillaPlayer,
) -> Result<Vec<(String, mc_nbt::Tag)>, mc_data::item_nbt::ItemNbtError> {
    use mc_data::item_nbt::stack_to_fields;
    let mut out = Vec::new();
    if !player.carried.is_empty() {
        out.push((
            "solaris:Carried".to_owned(),
            mc_nbt::Tag::Compound(stack_to_fields(items, &player.carried)?),
        ));
    }
    let grid: Vec<mc_nbt::Tag> = player
        .grid
        .iter()
        // Empty compounds retain cell positions through the shared item codec.
        .map(|stack| {
            if stack.is_empty() {
                Ok(mc_nbt::Tag::Compound(Vec::new()))
            } else {
                stack_to_fields(items, stack).map(mc_nbt::Tag::Compound)
            }
        })
        .collect::<Result<_, _>>()?;
    if !grid.is_empty() {
        out.push((
            "solaris:CraftingGrid".to_owned(),
            mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: if grid.is_empty() {
                    mc_nbt::tag_type::END
                } else {
                    mc_nbt::tag_type::COMPOUND
                },
                elements: grid,
            }),
        ));
    }
    Ok(out)
}

/// Decode a player image without touching filesystem state.
pub(crate) fn player_from_tag(
    root: &mc_nbt::Tag,
    items: &mc_data::items::ItemRegistry,
) -> Result<ReplayPlayer, CoreError> {
    use mc_data::player_nbt::{VanillaPlayer, player_from_fields};
    let failure =
        |stage: &str, error: String| CoreError::Storage(format!("player record: {stage}: {error}"));
    let mc_nbt::Tag::Compound(entries) = root else {
        return Err(failure("NBT", "root is not a compound".into()));
    };
    let player = player_from_fields(items, VanillaPlayer::default(), entries)
        .map_err(|err| failure("player fields", err.to_string()))?;
    let mut inventory = vec![mc_data::ItemStack::EMPTY; 41];
    for (slot, stack) in player.inventory {
        if let Some(index) = storage_slot_to_v2(slot) {
            inventory[index] = stack;
        }
    }
    for (name, stack) in player.equipment {
        if let Some(index) = equipment_to_v2_slot(&name) {
            inventory[index] = stack;
        }
    }
    let find = |name: &str| {
        entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    };
    let advancement_progress = match find("SolarisAdvancements") {
        Some(mc_nbt::Tag::ByteArray(json)) => {
            let bytes: Vec<_> = json.iter().map(|byte| *byte as u8).collect();
            Arc::new(
                serde_json::from_slice(&bytes)
                    .map_err(|err| failure("SolarisAdvancements", err.to_string()))?,
            )
        }
        None => empty_advancement_progress(),
        Some(_) => {
            return Err(failure(
                "SolarisAdvancements",
                "expected JSON byte array".into(),
            ));
        }
    };
    // Native custody wins; vanilla imports project only the owned recipe set.
    // The rest of recipeBook remains in opaque_player_fields.
    let recipe_list = match find("SolarisUnlockedRecipes") {
        Some(tag) => Some(tag),
        None => match find("recipeBook") {
            Some(mc_nbt::Tag::Compound(book)) => book
                .iter()
                .find(|(key, _)| key == "recipes")
                .map(|(_, tag)| tag),
            None => None,
            Some(_) => return Err(failure("recipeBook", "expected compound".into())),
        },
    };
    let unlocked_recipes = Arc::new(match recipe_list {
        Some(mc_nbt::Tag::List(list))
            if list.element_type == mc_nbt::tag_type::STRING || list.elements.is_empty() =>
        {
            list.elements
                .iter()
                .map(|tag| match tag {
                    mc_nbt::Tag::String(recipe) => Ok(recipe.clone()),
                    _ => Err(failure("unlocked recipes", "expected string list".into())),
                })
                .collect::<Result<BTreeSet<_>, _>>()?
        }
        None => BTreeSet::new(),
        Some(_) => return Err(failure("unlocked recipes", "expected string list".into())),
    });
    let carried = match find("solaris:Carried") {
        Some(mc_nbt::Tag::Compound(fields)) => mc_data::item_nbt::stack_from_fields(items, fields)
            .map_err(|err| failure("solaris:Carried slot 0", err.to_string()))?
            .unwrap_or(mc_data::ItemStack::EMPTY),
        Some(_) => {
            return Err(failure(
                "solaris:Carried slot 0",
                "expected item compound".into(),
            ));
        }
        None => mc_data::ItemStack::EMPTY,
    };
    let grid = match find("solaris:CraftingGrid") {
        Some(mc_nbt::Tag::List(list)) => {
            let mut grid = Vec::with_capacity(list.elements.len());
            for (slot, element) in list.elements.iter().enumerate() {
                let mc_nbt::Tag::Compound(fields) = element else {
                    return Err(failure(
                        &format!("solaris:CraftingGrid slot {slot}"),
                        "expected item compound".into(),
                    ));
                };
                let stack = mc_data::item_nbt::stack_from_fields(items, fields).map_err(|err| {
                    failure(
                        &format!("solaris:CraftingGrid slot {slot}"),
                        err.to_string(),
                    )
                })?;
                grid.push(stack.unwrap_or(mc_data::ItemStack::EMPTY));
            }
            grid
        }
        Some(_) => return Err(failure("solaris:CraftingGrid", "expected item list".into())),
        None => Vec::new(),
    };
    let respawn_fields = match find("respawn").or_else(|| find("solaris:RespawnState")) {
        Some(mc_nbt::Tag::Compound(fields)) => Some(fields),
        Some(_) => return Err(failure("respawn", "expected compound".into())),
        None if [
            "solaris:Respawn",
            "SpawnX",
            "SpawnY",
            "SpawnZ",
            "SpawnDimension",
            "SpawnForced",
            "SpawnAngle",
            "SpawnPitch",
        ]
        .iter()
        .any(|key| find(key).is_some()) =>
        {
            return Err(failure("respawn", "unsupported legacy respawn keys; Minecraft 26.1 requires respawn compound (no legacy fallback)".into()));
        }
        None => None,
    };
    let respawn_field = |key| {
        respawn_fields.and_then(|fields| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
        })
    };
    let respawn = match respawn_field("pos") {
        Some(mc_nbt::Tag::IntArray(cell)) if cell.len() == 3 => Some([cell[0], cell[1], cell[2]]),
        None if respawn_fields.is_none() || find("solaris:RespawnState").is_some() => None,
        _ => return Err(failure("respawn.pos", "expected three integers".into())),
    };
    let respawn_dimension = match respawn_field("dimension") {
        None if respawn_fields.is_none() => crate::dimensions::Dimension::Overworld,
        Some(mc_nbt::Tag::String(name)) => crate::dimensions::Dimension::parse(name)
            .filter(|dimension| dimension.name() == name)
            .ok_or_else(|| failure("respawn.dimension", format!("unknown dimension {name}")))?,
        _ => return Err(failure("respawn.dimension", "expected string".into())),
    };
    let respawn_forced = match respawn_field("forced") {
        None => false,
        Some(mc_nbt::Tag::Byte(value)) if matches!(value, 0 | 1) => *value != 0,
        _ => return Err(failure("respawn.forced", "expected boolean".into())),
    };
    let spawn_angle = |key, bound: f32| -> Result<f32, CoreError> {
        match respawn_field(key) {
            None if respawn_fields.is_none() => Ok(0.0),
            Some(mc_nbt::Tag::Float(value))
                if value.is_finite() && (-bound..=bound).contains(value) =>
            {
                Ok(*value)
            }
            _ => Err(failure(
                &format!("respawn.{key}"),
                format!("expected finite float in -{bound}..={bound}"),
            )),
        }
    };
    let opaque_fields: Vec<_> = entries
        .iter()
        .filter(|(key, _)| !player_owned_field(key))
        .cloned()
        .map(|(key, mut tag)| {
            if key == "recipeBook"
                && let mc_nbt::Tag::Compound(book) = &mut tag
            {
                book.retain(|(name, _)| name != "recipes");
            }
            (key, tag)
        })
        .collect();
    let mut opaque_player_fields = Vec::new();
    if !opaque_fields.is_empty() {
        mc_nbt::write_named(
            &mut opaque_player_fields,
            "",
            &mc_nbt::Tag::Compound(opaque_fields),
        )
        .map_err(|err| failure("opaque fields", err.to_string()))?;
    }
    let mut record = ReplayPlayer {
        opaque_player_fields,
        statistics: match find("SolarisStatistics") {
            None => mc_data::statistics::PlayerStatistics::default(),
            Some(mc_nbt::Tag::ByteArray(bytes)) => {
                let bytes: Vec<_> = bytes.iter().map(|byte| *byte as u8).collect();
                serde_json::from_slice(&bytes)
                    .map_err(|err| failure("SolarisStatistics", err.to_string()))?
            }
            Some(_) => return Err(failure("SolarisStatistics", "expected byte array".into())),
        },
        advancement_progress,
        unlocked_recipes,
        dimension: match find("Dimension") {
            None => crate::dimensions::Dimension::Overworld,
            Some(mc_nbt::Tag::String(name)) => crate::dimensions::Dimension::parse(name)
                .filter(|dimension| dimension.name() == name)
                .ok_or_else(|| failure("Dimension", format!("unknown dimension {name}")))?,
            Some(_) => return Err(failure("Dimension", "expected string".into())),
        },
        pos: player.pos,
        yaw: player.yaw,
        pitch: player.pitch,
        health: player.health,
        food: player.food,
        saturation: player.saturation,
        exhaustion: player.exhaustion,
        air: player.air,
        experience_level: player.xp_level,
        experience_progress: player.xp_progress,
        experience_total: player.xp_total,
        game_mode: u8::try_from(player.game_mode).unwrap_or(0),
        selected: player.selected,
        inventory,
        carried,
        grid,
        respawn,
        respawn_dimension,
        respawn_forced,
        respawn_yaw: spawn_angle("yaw", 180.0)?,
        respawn_pitch: spawn_angle("pitch", 90.0)?,
        active_effects: active_effects_from_fields(entries),
        enchantment_seed: entries
            .iter()
            .find_map(|(key, value)| match (key.as_str(), value) {
                ("XpSeed", mc_nbt::Tag::Int(seed)) => Some(*seed),
                _ => None,
            })
            .unwrap_or(0),
        current_impulse_impact_pos: crate::mace::read_impact(entries)
            .map_err(|error| failure("current_explosion_impact_pos", error.into()))?,
        fall_flying: match find("FallFlying") {
            None => false,
            Some(mc_nbt::Tag::Byte(0)) => false,
            Some(mc_nbt::Tag::Byte(1)) => true,
            Some(_) => return Err(failure("FallFlying", "expected boolean byte".into())),
        },
        warden_warning: match crate::warden::unique(entries, "SolarisWardenWarning")
            .map_err(|error| failure("SolarisWardenWarning", error))?
        {
            Some(mc_nbt::Tag::ByteArray(json)) => {
                let bytes: Vec<_> = json.iter().map(|byte| *byte as u8).collect();
                serde_json::from_slice(&bytes)
                    .map_err(|error| failure("SolarisWardenWarning", error.to_string()))?
            }
            Some(_) => {
                return Err(failure(
                    "SolarisWardenWarning",
                    "expected JSON byte array".into(),
                ));
            }
            None => crate::shrieker::Warning::read(entries)
                .map_err(|error| failure("warden_spawn_tracker", error))?,
        },
        burn_ticks: match find("solaris:BurnTicks") {
            Some(mc_nbt::Tag::Long(ticks)) => u32::try_from(*ticks)
                .map_err(|_| failure("solaris:BurnTicks", "expected u32".into()))?,
            Some(_) => return Err(failure("solaris:BurnTicks", "expected long".into())),
            None => entries
                .iter()
                .find_map(|(key, value)| match (key.as_str(), value) {
                    ("Fire", mc_nbt::Tag::Short(ticks)) => Some(u32::from((*ticks).max(0) as u16)),
                    _ => None,
                })
                .unwrap_or(0),
        },
        portal: crate::portals::from_fields(entries)?,
        tick: match find("solaris:Tick") {
            Some(mc_nbt::Tag::Long(tick)) => *tick as u64,
            Some(_) => return Err(failure("solaris:Tick", "expected long".into())),
            None => 0,
        },
    };
    record.statistics.advance_tick(record.tick);
    record.warden_warning.advance_tick(record.tick);
    Ok(record)
}

/// Root keys projected into the player model and regenerated on each capture.
fn player_owned_field(key: &str) -> bool {
    matches!(
        key,
        "Pos"
            | "Rotation"
            | "Health"
            | "foodLevel"
            | "foodSaturationLevel"
            | "foodExhaustionLevel"
            | "Air"
            | "SelectedItemSlot"
            | "XpLevel"
            | "XpP"
            | "XpTotal"
            | "playerGameType"
            | "Inventory"
            | "equipment"
            | "Dimension"
            | "active_effects"
            | "AbsorptionAmount"
            | "PortalCooldown"
            | "solaris:PortalTime"
            | "XpSeed"
            | "warden_spawn_tracker"
            | "SolarisWardenWarning"
            | "current_explosion_impact_pos"
            | "FallFlying"
            | "Fire"
            | "DataVersion"
            | "SolarisStatistics"
            | "solaris:Carried"
            | "solaris:CraftingGrid"
            | "respawn"
            | "solaris:RespawnState"
            | "solaris:Tick"
            | "solaris:BurnTicks"
            | "SolarisAdvancements"
            | "SolarisUnlockedRecipes"
            | "solaris:Respawn"
            | "SpawnX"
            | "SpawnY"
            | "SpawnZ"
            | "SpawnDimension"
            | "SpawnForced"
            | "SpawnAngle"
            | "SpawnPitch"
    )
}

/// Vanilla living-entity effect keys, shared by player and mob checkpoints.
pub(crate) fn active_effects_to_fields(
    effects: &crate::effects::ActiveEffects,
) -> Vec<(String, mc_nbt::Tag)> {
    use mc_nbt::{ListTag, Tag, tag_type};
    vec![
        (
            "active_effects".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: active_effects_to_nbt(effects),
            }),
        ),
        ("AbsorptionAmount".into(), Tag::Float(effects.absorption)),
    ]
}

pub(crate) fn active_effects_to_nbt(effects: &crate::effects::ActiveEffects) -> Vec<mc_nbt::Tag> {
    use mc_nbt::Tag;
    fn details(effect: &crate::effects::ActiveEffect, include_id: bool) -> Tag {
        let mut fields = vec![
            ("amplifier".into(), Tag::Int(i32::from(effect.amplifier))),
            ("duration".into(), Tag::Int(effect.duration)),
            ("ambient".into(), Tag::Byte(i8::from(effect.ambient))),
            (
                "show_particles".into(),
                Tag::Byte(i8::from(effect.show_particles)),
            ),
            ("show_icon".into(), Tag::Byte(i8::from(effect.show_icon))),
        ];
        if include_id {
            fields.push((
                "id".into(),
                Tag::String(format!("minecraft:{}", effect.id.name())),
            ));
        }
        if let Some(hidden) = &effect.hidden {
            fields.push(("hidden_effect".into(), details(hidden, false)));
        }
        Tag::Compound(fields)
    }
    effects
        .entries
        .iter()
        .map(|effect| details(effect, true))
        .collect()
}

pub(crate) fn active_effects_from_fields(
    fields: &[(String, mc_nbt::Tag)],
) -> crate::effects::ActiveEffects {
    let find = |name| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    };
    let entries = match find("active_effects") {
        Some(mc_nbt::Tag::List(list)) => list.elements.as_slice(),
        _ => &[],
    };
    let absorption = match find("AbsorptionAmount") {
        Some(mc_nbt::Tag::Float(value)) => *value,
        _ => 0.0,
    };
    active_effects_from_nbt(entries, absorption)
}

pub(crate) fn active_effects_from_nbt(
    effects: &[mc_nbt::Tag],
    absorption: f32,
) -> crate::effects::ActiveEffects {
    use crate::effects::{ActiveEffect, ActiveEffects, EffectId};
    use mc_nbt::Tag;
    fn find<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    }
    fn details(fields: &[(String, Tag)], id: EffectId) -> ActiveEffect {
        let integer = |name| match find(fields, name) {
            Some(Tag::Int(value)) => *value,
            Some(Tag::Byte(value)) => i32::from(*value as u8),
            _ => 0,
        };
        let boolean = |name, default| match find(fields, name) {
            Some(Tag::Byte(value)) => *value != 0,
            _ => default,
        };
        let show_particles = boolean("show_particles", true);
        let mut effect = ActiveEffect::new(
            id,
            integer("duration"),
            integer("amplifier").clamp(0, 255) as u8,
        );
        effect.ambient = boolean("ambient", false);
        effect.show_particles = show_particles;
        effect.show_icon = boolean("show_icon", show_particles);
        effect.hidden = match find(fields, "hidden_effect") {
            Some(Tag::Compound(hidden)) => Some(Box::new(details(hidden, id))),
            _ => None,
        };
        effect
    }
    let entries = effects
        .iter()
        .filter_map(|tag| {
            let Tag::Compound(fields) = tag else {
                return None;
            };
            let Some(Tag::String(name)) = find(fields, "id") else {
                return None;
            };
            EffectId::from_name(name).map(|id| details(fields, id))
        })
        .collect();
    ActiveEffects {
        entries,
        absorption,
    }
}

/// Open (and truncate) a file for the journal thread's ownership.
pub fn open_journal_stream(path: &Path) -> Result<File, CoreError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| CoreError::Storage(err.to_string()))
}

/// Drain helper for tests: read all entries of one batch file.
pub fn read_batch_file(path: &Path) -> Option<Vec<JournalEntry>> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (lo, hi) = parse_batch_name(&name)?;
    let bytes = std::fs::read(path).ok()?;
    decode_batch(&bytes, lo, hi).ok()
}

/// Replayed state: absolute post-images keyed by domain.
#[derive(Debug, Default)]
pub struct Replay {
    pub dimensions: BTreeMap<crate::dimensions::Dimension, Box<Replay>>,
    pub(crate) plugins: crate::plugin::PluginState,
    /// Block post-images.
    pub blocks: BTreeMap<(i32, i32, i32), u32>,
    /// Player post-images by uuid string.
    pub players: BTreeMap<String, ReplayPlayer>,
    /// Block-entity post-images by block position.
    pub block_entities: BTreeMap<(i32, i32, i32), ReplayBlockEntity>,
    pub opaque_block_entities: BTreeMap<(i32, i32, i32), Option<Vec<u8>>>,
    /// Entity post-images by wire id.
    pub entities: BTreeMap<i32, ReplayEntity>,
    /// Ticket post-images by owner: centre, (tick, radius).
    pub tickets: BTreeMap<String, ([i32; 2], (u64, i32))>,
    /// Latest scoreboard image, including an empty-board reset.
    pub scoreboard: Option<crate::scoreboard::Scoreboard>,
    pub end_fight: Option<crate::end_fight::Fight>,
    pub raids: Option<crate::raids::Raids>,
    pub wandering_trader: Option<crate::mob_lanes::TraderSpawner>,
    pub generated_entity_claims: std::collections::BTreeSet<String>,
    pub block_ticks: crate::block_ticks::BlockTicksImage,
    pub fluid_ticks: crate::fluids::FluidTicksImage,
    pub inhabited_time: BTreeMap<[i32; 2], InhabitedTimeImage>,
    /// 001 slice0 per-chunk catch-up debt (authority, active-only clock).
    pub sim_debt: BTreeMap<[i32; 2], SimDebtImage>,
    /// Transient cumulative active time while folding the existing debt images.
    pub(crate) simulation_clocks: BTreeMap<[i32; 2], ReplayChunkClock>,
    pub climate: Option<crate::weather::ClimateImage>,
    pub border: crate::command_border::Border,
    pub difficulty: crate::survival::Difficulty,
    pub gamerules: BTreeMap<String, String>,
    pub settlements: BTreeMap<u64, crate::settlements::SettlementRecord>,
    pub settlement_removed: BTreeSet<u64>,
    pub settlement_high_water: u64,
    /// Highest tick seen.
    pub tick: u64,
}

#[derive(Debug)]
pub(crate) struct ReplayChunkClock {
    points: Vec<ReplayClockPoint>,
}

#[derive(Debug)]
struct ReplayClockPoint {
    tick: u64,
    elapsed: u64,
    active: bool,
}

impl ReplayChunkClock {
    fn new(tick: u64, active: bool) -> Self {
        Self {
            points: vec![ReplayClockPoint {
                tick,
                elapsed: 0,
                active,
            }],
        }
    }

    fn through(&self, tick: u64) -> u64 {
        let index = self.points.partition_point(|point| point.tick <= tick);
        let Some(point) = index.checked_sub(1).map(|index| &self.points[index]) else {
            return 0;
        };
        point.elapsed + if point.active { tick - point.tick } else { 0 }
    }

    fn between(&self, from: u64, to: u64) -> u64 {
        self.through(to).saturating_sub(self.through(from))
    }

    fn observe(&mut self, image: SimDebtImage) {
        if self
            .points
            .last()
            .is_some_and(|point| point.active == image.active)
        {
            return;
        }
        // Inactive images may be drained later than the actual exit. Their
        // last simulated tick, not their publication tick, closes the interval.
        let tick = if image.active {
            image.tick
        } else {
            image.tick.min(image.last_sim.saturating_add(1))
        };
        let elapsed = if image.active {
            self.through(tick.saturating_sub(1)) + 1
        } else {
            self.through(image.last_sim)
        };
        self.points.push(ReplayClockPoint {
            tick,
            elapsed,
            active: image.active,
        });
    }

    fn advance_villager(&self, villager: &mut crate::villager::VillagerState, from: u64, to: u64) {
        for (index, point) in self.points.iter().enumerate() {
            if !point.active {
                continue;
            }
            let start = from.max(point.tick.saturating_sub(1));
            let end = self
                .points
                .get(index + 1)
                .map_or(to, |next| to.min(next.tick.saturating_sub(1)));
            if start < end {
                crate::villager::persistence::advance_clocks(villager, start, end);
            }
        }
    }
}

fn replay_entity_chunk(pos: [f64; 3]) -> [i32; 2] {
    [
        (pos[0].floor() as i32).div_euclid(16),
        (pos[2].floor() as i32).div_euclid(16),
    ]
}

/// One replayed entity record. Ordinary intra-chunk mob motion is checkpoint
/// durable; crossing images advance the position for chunk clock attribution.
/// Projectile images also advance motion with retained custody/payload,
/// preventing a replay from rewinding a missile across an already committed hit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplayEntity {
    pub uuid: String,
    pub type_id: u32,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub ground: bool,
    pub health: f32,
    pub alive: bool,
    pub age: u32,
    pub breed_cooldown: u32,
    pub pickup_delay: u32,
    pub item: Option<mc_data::ItemStack>,
    pub appearance: EntityAppearanceImage,
    #[serde(default)]
    pub active_effects: crate::effects::ActiveEffects,
    #[serde(
        default,
        deserialize_with = "crate::villager::persistence::deserialize_optional_state"
    )]
    pub villager: Option<crate::villager::VillagerState>,
    #[serde(default)]
    pub state: EntityStateImage,
    #[serde(
        default,
        deserialize_with = "crate::projectiles::persistence::deserialize_optional_state"
    )]
    pub projectile: Option<crate::projectiles::ProjectileState>,
    pub tick: u64,
}

/// Native living state shared by full birth images and compact timer updates.
/// Nonplayer protection deadlines are durable; player joins/respawns rearm
/// protection separately. Attack cooldowns, hurt windows, horizontal impulses
/// and combat credit intentionally remain restart-local.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EntityStateImage {
    /// Original named NBT compound; typed checkpoint fields override these keys.
    pub opaque_entity_fields: Vec<u8>,
    /// Lifetime age at this image's journal tick; not an age-only dirty key.
    pub age: u32,
    pub protected_until: u64,
    pub persistent: bool,
    /// Checkpoint-durable idle/despawn counter; never a WAL dirty key by itself.
    pub no_action_time: u32,
    pub air_supply: i32,
    pub burn_ticks: u32,
    pub love: u32,
    pub breed_cooldown: u32,
    pub entity_life: crate::entity_life::EntityLifeState,
    pub loot: crate::entity_loot::EntityLootState,
    pub controls: crate::entity_controls::Controls,
    pub falling: Option<crate::falling::FallingState>,
    pub tnt: Option<crate::tnt::PrimedTntState>,
    #[serde(deserialize_with = "crate::special_mobs::persistence::deserialize")]
    pub special_mob: Option<crate::special_mobs::Retained>,
    #[serde(deserialize_with = "crate::bees::deserialize")]
    pub bee: Option<crate::bees::BeeState>,
    #[serde(deserialize_with = "crate::allay::deserialize")]
    pub allay: Option<crate::allay::AllayState>,
}

impl Default for EntityStateImage {
    fn default() -> Self {
        Self {
            age: 0,
            opaque_entity_fields: Vec::new(),
            protected_until: 0,
            persistent: false,
            no_action_time: 0,
            air_supply: 300,
            burn_ticks: 0,
            love: 0,
            breed_cooldown: 0,
            entity_life: Default::default(),
            loot: Default::default(),
            controls: Default::default(),
            falling: None,
            tnt: None,
            special_mob: None,
            bee: None,
            allay: None,
        }
    }
}

impl EntityStateImage {
    pub(crate) fn from_row(body: &crate::entities::Body, row: &crate::entities::ColdRow) -> Self {
        Self {
            age: row.age,
            opaque_entity_fields: row.opaque_entity_fields.clone(),
            protected_until: row.protected_until,
            persistent: row.persistent,
            no_action_time: row.no_action_time,
            air_supply: body.air_supply,
            burn_ticks: body.burn_ticks,
            love: row.love,
            breed_cooldown: row.breed_cooldown,
            entity_life: row.entity_life.clone(),
            loot: row.loot.clone(),
            controls: row.controls,
            falling: row.falling,
            tnt: row.tnt,
            special_mob: row.special_mob.as_deref().cloned(),
            bee: row.bee.as_deref().cloned(),
            allay: row.allay.as_deref().cloned(),
        }
    }

    /// Deterministic lifecycle clocks are derived from batch ticks. Feeding,
    /// age-lock changes, egg resets and all other events still differ from the
    /// projection and retain an absolute image; random intervals are not replayed.
    pub(crate) fn advance_clocks(&mut self, type_id: u32, baby: bool, elapsed: u64) {
        let elapsed = elapsed.min(u64::from(u32::MAX)) as u32;
        self.breed_cooldown = self.breed_cooldown.saturating_sub(elapsed);
        self.love = self.love.saturating_sub(elapsed);
        if self.entity_life.child_ticks > 0 && !self.entity_life.age_lock.locked {
            self.entity_life.child_ticks = self.entity_life.child_ticks.saturating_sub(elapsed);
            self.love = 0;
        }
        if !self.entity_life.age_lock.locked
            && mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .is_some_and(|contract| contract.name == "minecraft:tadpole")
        {
            self.entity_life.age_lock.tadpole_age = self
                .entity_life
                .age_lock
                .tadpole_age
                .saturating_add(elapsed);
        }
        if self.entity_life.harvest_ticks != 0
            && self.entity_life.child_ticks == 0
            && !baby
            && mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .is_some_and(|contract| contract.name == "minecraft:chicken")
        {
            self.entity_life.harvest_ticks = self.entity_life.harvest_ticks.saturating_sub(elapsed);
        }
    }

    /// Whether [`Self::advance_clocks`] can change this image over time.
    /// Live lifecycle clocks advance the same projection without dirty marks;
    /// stopped (dead) rows need rechecks while their image remains sensitive.
    /// Keep in step with `advance_clocks`.
    pub(crate) fn clock_sensitive(&self, type_id: u32, baby: bool) -> bool {
        let life = &self.entity_life;
        let named = |name: &str| {
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .is_some_and(|contract| contract.name == name)
        };
        self.breed_cooldown > 0
            || self.love > 0
            || (life.child_ticks > 0 && !life.age_lock.locked)
            || (!life.age_lock.locked && named("minecraft:tadpole"))
            || (life.harvest_ticks != 0
                && life.child_ticks == 0
                && !baby
                && named("minecraft:chicken"))
    }

    pub(crate) fn matches(
        &self,
        body: &crate::entities::Body,
        row: &crate::entities::ColdRow,
    ) -> bool {
        self.protected_until == row.protected_until
            && self.persistent == row.persistent
            && self.air_supply == body.air_supply
            && self.burn_ticks == body.burn_ticks
            && self.love == row.love
            && self.breed_cooldown == row.breed_cooldown
            && self.entity_life == row.entity_life
            && self.loot == row.loot
            && self.controls == row.controls
            && self.falling == row.falling
            && self.tnt == row.tnt
            && self.special_mob.as_ref() == row.special_mob.as_deref()
            && self.bee.as_ref() == row.bee.as_deref()
            && self.allay.as_ref() == row.allay.as_deref()
            && self.opaque_entity_fields == row.opaque_entity_fields
    }

    pub(crate) fn from_record(
        registry: &crate::registry::CoreRegistry,
        record: &mc_data::entity_nbt::VanillaEntity,
    ) -> Result<Self, CoreError> {
        if record.id == "minecraft:armor_stand" {
            crate::armor_stand::validate_fields(&record.extra_fields)?;
        }
        let ageable = registry
            .entity_types
            .id_of(
                &mc_data::Identifier::parse(&record.id)
                    .map_err(|err| CoreError::Storage(err.to_string()))?,
            )
            .and_then(|id| registry.blueprint(id))
            .is_some_and(|blueprint| {
                blueprint
                    .goals
                    .iter()
                    .any(|goal| goal.kind == crate::blueprint::GoalKind::Breed)
            });
        let ageable = ageable || crate::entity_life::old_ageable(&record.id);
        let (entity_life, love) = crate::entity_life::from_record(record, ageable);
        let mut image = Self {
            age: if ageable { 0 } else { record.age.max(0) as u32 },
            entity_life,
            love,
            breed_cooldown: record
                .breed_cooldown
                .max(if ageable { record.age.max(0) } else { 0 })
                as u32,
            loot: crate::entity_loot::from_fields(&record.extra_fields)?,
            falling: crate::falling::from_record(record, registry).map_err(CoreError::Storage)?,
            tnt: crate::tnt::from_record(registry, record)?,
            special_mob: crate::special_mobs::persistence::from_record(record)?,
            bee: crate::bees::from_record(record)?,
            allay: crate::allay::from_record(&registry.items, record)?,
            ..Default::default()
        };
        crate::mounts::persistence::from_fields(&mut image.entity_life, &record.extra_fields)?;
        image.entity_life.age_lock = crate::golden_dandelion::from_fields(&record.extra_fields)?;
        if record.id == "minecraft:tadpole" {
            image.entity_life.age_lock.tadpole_age = u32::try_from(record.age).map_err(|_| {
                CoreError::Storage("tadpole Age: expected nonnegative TAG_Int".into())
            })?;
        }
        image.entity_life.portal = crate::portals::from_fields(&record.extra_fields)?;
        image.entity_life.vehicle = crate::vehicles::from_record(registry, record)?.map(Box::new);
        image.entity_life.dolphin = crate::dolphin::from_record(record)?;
        crate::entity_loot::extra_equipment_from_fields(
            &registry.items,
            &record.extra_fields,
            &mut image.loot,
        )?;
        for (key, tag) in &record.extra_fields {
            match (key.as_str(), tag) {
                ("CustomNameVisible", mc_nbt::Tag::Byte(_)) => {}
                ("CustomNameVisible", _) => {
                    return Err(CoreError::Storage(
                        "CustomNameVisible: expected byte".into(),
                    ));
                }
                ("solaris:LifetimeAge", mc_nbt::Tag::Long(value)) => {
                    image.age = u32::try_from(*value).map_err(|_| {
                        CoreError::Storage("solaris:LifetimeAge: expected u32".into())
                    })?;
                }
                ("solaris:LifetimeAge", _) => {
                    return Err(CoreError::Storage(
                        "solaris:LifetimeAge: expected long".into(),
                    ));
                }
                ("solaris:ProtectedUntil", mc_nbt::Tag::Long(value)) => {
                    image.protected_until = *value as u64;
                }
                ("solaris:ProtectedUntil", _) => {
                    return Err(CoreError::Storage(
                        "solaris:ProtectedUntil: expected long".into(),
                    ));
                }
                ("PersistenceRequired", mc_nbt::Tag::Byte(value)) => image.persistent = *value != 0,
                ("solaris:NoActionTime", mc_nbt::Tag::Long(value)) => {
                    image.no_action_time = u32::try_from(*value)
                        .map_err(|_| CoreError::Storage("invalid entity idle timer".into()))?;
                }
                ("Air", mc_nbt::Tag::Short(value)) => image.air_supply = i32::from(*value),
                ("Air", mc_nbt::Tag::Int(value)) => image.air_supply = *value,
                ("Fire", mc_nbt::Tag::Short(value)) => image.burn_ticks = (*value).max(0) as u32,
                _ => {}
            }
        }
        image.controls = crate::entity_controls::Controls::from_fields(&record.extra_fields)?;
        // Keep only fields outside runtime authority. In particular optional
        // typed fields must not be resurrected after their resident value clears.
        let mut owned = image.to_fields(&registry.blocks);
        if let Some(allay) = &image.allay {
            owned.extend(crate::allay::to_fields(allay, &registry.items));
        }
        if let Some(vehicle) = &image.entity_life.vehicle {
            owned.extend(crate::vehicles::to_fields(vehicle, &registry.items));
        }
        let projectile_kind = crate::projectiles::ProjectileKind::from_name(&record.id);
        let villager = matches!(
            record.id.as_str(),
            "minecraft:villager" | "minecraft:zombie_villager" | "minecraft:wandering_trader"
        );
        let retained: Vec<_> = record
            .extra_fields
            .iter()
            .filter(|(key, _)| {
                !(owned.iter().any(|(typed, _)| typed == key)
                    || projectile_kind
                        .is_some_and(|kind| crate::projectiles::persistence::owns_field(kind, key))
                    || villager && crate::villager::persistence::owns_field(key)
                    || image.entity_life.vehicle.is_some() && key == "Items"
                    || matches!(
                        key.as_str(),
                        "CustomName"
                            | "CustomNameVisible"
                            | "equipment"
                            | "RaidId"
                            | "Wave"
                            | "CanJoinRaid"
                            | "DragonPhase"
                            | "DragonDeathTime"
                            | "ShowBottom"
                            | "beam_target"
                    ))
            })
            .cloned()
            .collect();
        if !retained.is_empty() {
            mc_nbt::write_named(
                &mut image.opaque_entity_fields,
                "",
                &mc_nbt::Tag::Compound(retained),
            )
            .map_err(|err| CoreError::Storage(format!("entity opaque fields: {err}")))?;
        }
        Ok(image)
    }

    pub(crate) fn to_fields(&self, blocks: &mc_world::BlockRegistry) -> Vec<(String, mc_nbt::Tag)> {
        let mut fields = crate::entity_life::to_fields(&self.entity_life, self.love);
        fields.extend(crate::entity_loot::to_fields(&self.loot));
        fields.extend(self.controls.to_fields());
        if let Some(dolphin) = &self.entity_life.dolphin {
            fields.extend(crate::dolphin::to_fields(dolphin));
        }
        if let Some(value) = &self.special_mob {
            fields.extend(crate::special_mobs::persistence::to_fields(value));
        }
        if let Some(bee) = &self.bee {
            fields.extend(crate::bees::to_fields(bee));
        }
        if let Some(falling) = &self.falling {
            fields.extend(crate::falling::to_fields(falling, blocks));
        }
        if let Some(tnt) = &self.tnt {
            fields.extend(crate::tnt::to_fields(tnt, blocks));
        }
        fields.extend([
            (
                "solaris:LifetimeAge".into(),
                mc_nbt::Tag::Long(i64::from(self.age)),
            ),
            (
                "solaris:ProtectedUntil".into(),
                mc_nbt::Tag::Long(self.protected_until as i64),
            ),
            (
                "Fire".into(),
                mc_nbt::Tag::Short(self.burn_ticks.min(i16::MAX as u32) as i16),
            ),
            (
                "PersistenceRequired".into(),
                mc_nbt::Tag::Byte(i8::from(self.persistent)),
            ),
            (
                "solaris:NoActionTime".into(),
                mc_nbt::Tag::Long(i64::from(self.no_action_time)),
            ),
            (
                "Air".into(),
                mc_nbt::Tag::Short(self.air_supply.clamp(i16::MIN as i32, i16::MAX as i32) as i16),
            ),
        ]);
        fields
    }
}

/// Fold batches into a [`Replay`]. Later batches overwrite earlier ones
/// (absolute post-images converge).
#[must_use]
/// Fold journal batches over a checkpoint base (one fold function).
/// The base holds the checkpoint's absolute post-images (entities with
/// motion, block entities, players); each batch entry overwrites or merges
/// over it, so tail removals delete base records, tail vitals update base
/// health, and intra-chunk entity images keep base motion. A default [`Replay`] is
/// an empty base (fresh world).
pub fn replay_batches(replay: Replay, batches: &[(u64, u64, Vec<JournalEntry>)]) -> Replay {
    let mut replay = apply_batch_images(replay, batches);
    advance_replay_clocks(&mut replay);
    replay
}

/// Fold absolute images without scanning unrelated retained entities per batch.
pub(crate) fn apply_batch_images(
    mut replay: Replay,
    batches: &[(u64, u64, Vec<JournalEntry>)],
) -> Replay {
    if replay.simulation_clocks.is_empty() {
        for (&chunk, image) in &replay.sim_debt {
            replay
                .simulation_clocks
                .insert(chunk, ReplayChunkClock::new(replay.tick, image.active));
        }
    }
    for (lo, hi, entries) in batches {
        replay.tick = replay.tick.max(*hi);
        let _ = lo;
        // Debt is emitted after entity images. Fold its boundaries first so
        // even a compact vitals anchor sees this batch's complete activity.
        for entry in entries {
            if let JournalEntry::SimDebt { image } = entry {
                if replay
                    .sim_debt
                    .get(&image.chunk)
                    .is_some_and(|old| image.tick <= old.tick)
                {
                    continue;
                }
                replay
                    .simulation_clocks
                    .entry(image.chunk)
                    .or_insert_with(|| ReplayChunkClock::new(0, false))
                    .observe(*image);
                replay.sim_debt.insert(image.chunk, *image);
            }
        }
        for entry in entries {
            match entry {
                JournalEntry::Plugins { value } => {
                    replay.plugins.apply_delta(value);
                }
                JournalEntry::EndFight { value } => {
                    replay.end_fight = Some(value.clone());
                }
                JournalEntry::Dimension { dimension, entries } => {
                    if *dimension == crate::dimensions::Dimension::Overworld {
                        replay = apply_batch_images(replay, &[(*lo, *hi, entries.clone())]);
                    } else {
                        let base = replay
                            .dimensions
                            .remove(dimension)
                            .map(|base| *base)
                            .unwrap_or_default();
                        replay.dimensions.insert(
                            *dimension,
                            Box::new(apply_batch_images(base, &[(*lo, *hi, entries.clone())])),
                        );
                    }
                }
                JournalEntry::GameRules { values } => {
                    replay.gamerules.clone_from(values);
                }
                JournalEntry::BlockTicks {
                    image,
                    next_sequence,
                } => {
                    replay.block_ticks.apply(image.clone(), *next_sequence);
                }
                JournalEntry::FluidTicks { image } => {
                    replay.fluid_ticks = image.clone();
                }
                JournalEntry::InhabitedTime { image } => {
                    replay.inhabited_time.insert(image.chunk, *image);
                }
                JournalEntry::SimDebt { .. } => {}
                JournalEntry::GeneratedEntityClaim { claim } => {
                    replay.generated_entity_claims.insert(claim.clone());
                }
                JournalEntry::Climate { value } => {
                    replay.climate = Some(*value);
                }
                JournalEntry::Border { value } => {
                    replay.border = *value;
                }
                JournalEntry::Difficulty { value } => {
                    replay.difficulty = *value;
                }
                JournalEntry::Scoreboard { value } => {
                    replay.scoreboard = Some(value.clone());
                }
                JournalEntry::Raids { value } => {
                    replay.raids = Some(value.clone());
                }
                JournalEntry::WanderingTrader { value } => {
                    replay.wandering_trader = Some(*value);
                }
                JournalEntry::Block { pos, state } => {
                    replay.blocks.insert((pos[0], pos[1], pos[2]), *state);
                }
                JournalEntry::Statistics {
                    uuid,
                    changes,
                    timers,
                    tick,
                } => {
                    if let Some(player) = replay.players.get_mut(uuid) {
                        player.statistics.apply_changes(changes);
                        if let Some(timers) = timers {
                            player.statistics.apply_timers(*timers);
                        }
                        player.statistics.advance_tick(*tick);
                        player.tick = *tick;
                    }
                }
                JournalEntry::Player {
                    uuid,
                    opaque_player_fields,
                    statistics,
                    advancement_progress,
                    unlocked_recipes,
                    dimension,
                    pos,
                    yaw,
                    pitch,
                    health,
                    food,
                    saturation,
                    exhaustion,
                    air,
                    experience_level,
                    experience_progress,
                    experience_total,
                    game_mode,
                    selected,
                    inventory,
                    carried,
                    grid,
                    respawn,
                    respawn_dimension,
                    respawn_forced,
                    respawn_yaw,
                    respawn_pitch,
                    active_effects,
                    enchantment_seed,
                    warden_warning,
                    current_impulse_impact_pos,
                    fall_flying,
                    burn_ticks,
                    portal,
                    tick,
                } => {
                    let mut statistics = statistics.as_ref().clone();
                    statistics.advance_tick(*tick);
                    let previous = replay.players.get(uuid);
                    let advancement_progress = advancement_progress
                        .clone()
                        .or_else(|| previous.map(|player| Arc::clone(&player.advancement_progress)))
                        .unwrap_or_else(empty_advancement_progress);
                    let unlocked_recipes = unlocked_recipes
                        .clone()
                        .or_else(|| previous.map(|player| Arc::clone(&player.unlocked_recipes)))
                        .unwrap_or_default();
                    replay.players.insert(
                        uuid.clone(),
                        ReplayPlayer {
                            opaque_player_fields: opaque_player_fields.clone(),
                            statistics,
                            advancement_progress,
                            unlocked_recipes,
                            dimension: *dimension,
                            pos: *pos,
                            yaw: *yaw,
                            pitch: *pitch,
                            health: *health,
                            food: *food,
                            saturation: *saturation,
                            exhaustion: *exhaustion,
                            air: *air,
                            experience_level: *experience_level,
                            experience_progress: *experience_progress,
                            experience_total: *experience_total,
                            game_mode: *game_mode,
                            selected: *selected,
                            inventory: inventory.clone(),
                            carried: carried.clone(),
                            grid: grid.clone(),
                            respawn: *respawn,
                            respawn_dimension: *respawn_dimension,
                            respawn_forced: *respawn_forced,
                            respawn_yaw: *respawn_yaw,
                            respawn_pitch: *respawn_pitch,
                            active_effects: active_effects.clone(),
                            enchantment_seed: *enchantment_seed,
                            warden_warning: *warden_warning,
                            current_impulse_impact_pos: *current_impulse_impact_pos,
                            fall_flying: *fall_flying,
                            burn_ticks: *burn_ticks,
                            portal: *portal,
                            tick: *tick,
                        },
                    );
                }
                JournalEntry::BlockEntity {
                    pos,
                    slots,
                    furnace,
                    tick,
                } => {
                    replay.block_entities.insert(
                        (pos[0], pos[1], pos[2]),
                        ReplayBlockEntity {
                            slots: slots.clone(),
                            furnace: furnace.clone(),
                            tick: *tick,
                        },
                    );
                }
                JournalEntry::RemoveBlockEntity { pos, .. } => {
                    replay.block_entities.remove(&(pos[0], pos[1], pos[2]));
                    replay
                        .opaque_block_entities
                        .insert((pos[0], pos[1], pos[2]), None);
                }
                JournalEntry::OpaqueBlockEntity { pos, nbt } => {
                    replay
                        .opaque_block_entities
                        .insert((pos[0], pos[1], pos[2]), nbt.clone());
                }
                JournalEntry::Entity {
                    id,
                    uuid,
                    type_id,
                    health,
                    alive,
                    age,
                    breed_cooldown,
                    pickup_delay,
                    item,
                    appearance,
                    active_effects,
                    villager,
                    state,
                    projectile,
                    tick,
                    pos: spawn_pos,
                    vel: flight_vel,
                    yaw: spawn_yaw,
                    pitch: spawn_pitch,
                } => {
                    if replay
                        .entities
                        .get(id)
                        .is_some_and(|record| *tick < record.tick)
                    {
                        continue;
                    }
                    if *alive {
                        // Ordinary intra-chunk motion stays checkpoint-only;
                        // chunk crossings and retained body motion advance together.
                        let base = replay.entities.get(id);
                        let (pos, vel, yaw, pitch, ground) = if projectile.is_some()
                            || state.tnt.is_some()
                            || state.falling.is_some()
                            || state.entity_life.vehicle.is_some()
                            || crate::end_fight::dragon_type(*type_id)
                            || crate::piglin::name(*type_id) == "minecraft:enderman"
                            || base.is_some_and(|record| {
                                replay_entity_chunk(record.pos) != replay_entity_chunk(*spawn_pos)
                            }) {
                            (
                                *spawn_pos,
                                *flight_vel,
                                *spawn_yaw,
                                *spawn_pitch,
                                projectile.as_ref().is_some_and(|record| record.stuck)
                                    || state.tnt.is_some_and(|record| record.on_ground)
                                    || state
                                        .entity_life
                                        .vehicle
                                        .as_ref()
                                        .is_some_and(|v| v.on_ground),
                            )
                        } else {
                            match base {
                                Some(record) => (
                                    record.pos,
                                    record.vel,
                                    record.yaw,
                                    record.pitch,
                                    record.ground,
                                ),
                                None => (*spawn_pos, [0.0; 3], *spawn_yaw, *spawn_pitch, false),
                            }
                        };
                        replay.entities.insert(
                            *id,
                            ReplayEntity {
                                uuid: uuid.clone(),
                                type_id: *type_id,
                                pos,
                                vel,
                                yaw,
                                pitch,
                                ground,
                                health: *health,
                                alive: *alive,
                                age: *age,
                                breed_cooldown: *breed_cooldown,
                                pickup_delay: *pickup_delay,
                                item: item.clone(),
                                appearance: appearance.clone(),
                                active_effects: active_effects.clone(),
                                villager: villager.as_deref().cloned(),
                                state: EntityStateImage {
                                    age: *age,
                                    ..state.as_ref().clone()
                                },
                                projectile: projectile.as_deref().cloned(),
                                tick: *tick,
                            },
                        );
                    } else {
                        replay.entities.remove(id);
                    }
                }
                JournalEntry::EntityState { id, state, tick } => {
                    if let Some(record) = replay.entities.get_mut(id) {
                        if *tick < record.tick {
                            continue;
                        }
                        if let Some(villager) = &mut record.villager
                            && let Some(clock) = replay
                                .simulation_clocks
                                .get(&replay_entity_chunk(record.pos))
                        {
                            clock.advance_villager(villager, record.tick, *tick);
                        }
                        record.age = state.age;
                        record.state = state.as_ref().clone();
                        record.breed_cooldown = state.breed_cooldown;
                        record.tick = *tick;
                    }
                }
                JournalEntry::EntityVitals {
                    id,
                    health,
                    alive,
                    tick,
                } => {
                    if let Some(record) = replay.entities.get_mut(id) {
                        if *tick < record.tick {
                            continue;
                        }
                        if *alive {
                            let clock = replay
                                .simulation_clocks
                                .get(&replay_entity_chunk(record.pos));
                            let elapsed =
                                clock.map_or(0, |clock| clock.between(record.tick, *tick));
                            record.state.advance_clocks(
                                record.type_id,
                                record.appearance.baby,
                                elapsed,
                            );
                            if let Some(villager) = &mut record.villager
                                && let Some(clock) = clock
                            {
                                clock.advance_villager(villager, record.tick, *tick);
                            }
                            record.breed_cooldown = record.state.breed_cooldown;
                            record.age = record.age.wrapping_add(elapsed as u32);
                            record.state.age = record.age;
                            record.health = *health;
                            record.appearance.health = *health;
                            record.tick = *tick;
                        } else {
                            replay.entities.remove(id);
                        }
                    }
                }
                JournalEntry::RemoveEntity { id, tick } => {
                    if replay
                        .entities
                        .get(id)
                        .is_none_or(|record| *tick >= record.tick)
                    {
                        replay.entities.remove(id);
                    }
                }
                JournalEntry::Ticket {
                    owner,
                    chunk,
                    tick,
                    radius,
                } => {
                    replay
                        .tickets
                        .insert(owner.clone(), (*chunk, (*tick, *radius)));
                }
                JournalEntry::Settlement {
                    image, high_water, ..
                } => {
                    replay.settlement_high_water = replay
                        .settlement_high_water
                        .max(*high_water)
                        .max(image.id.0);
                    match &image.value {
                        Some(record) => {
                            replay.settlements.insert(image.id.0, record.clone());
                            replay.settlement_removed.remove(&image.id.0);
                        }
                        None => {
                            replay.settlements.remove(&image.id.0);
                            replay.settlement_removed.insert(image.id.0);
                        }
                    }
                }
            }
        }
    }
    replay
}

/// Project each retained anchor once after the complete streamed tail is folded.
pub(crate) fn advance_replay_clocks(replay: &mut Replay) {
    for record in replay.entities.values_mut() {
        if record.tick < replay.tick {
            let clock = replay
                .simulation_clocks
                .get(&replay_entity_chunk(record.pos));
            let elapsed = clock.map_or(0, |clock| clock.between(record.tick, replay.tick));
            record
                .state
                .advance_clocks(record.type_id, record.appearance.baby, elapsed);
            if let Some(villager) = &mut record.villager
                && let Some(clock) = clock
            {
                clock.advance_villager(villager, record.tick, replay.tick);
            }
            record.breed_cooldown = record.state.breed_cooldown;
            record.age = record.age.wrapping_add(elapsed as u32);
            record.tick = replay.tick;
        }
        record.state.age = record.age;
    }
    for player in replay.players.values_mut() {
        player.warden_warning.advance_tick(replay.tick);
    }
    // 001 slice0: active debt intervals advance through the recovered WAL
    // tick (never `remaining`, never wall time); inactive images freeze.
    for image in replay.sim_debt.values_mut() {
        image.last_sim = image.last_sim_at(replay.tick);
        image.tick = replay.tick;
    }
    for clock in replay.simulation_clocks.values_mut() {
        let active = clock.points.last().is_some_and(|point| point.active);
        clock.points.clear();
        clock.points.push(ReplayClockPoint {
            tick: replay.tick,
            elapsed: 0,
            active,
        });
    }
    for dimension in replay.dimensions.values_mut() {
        advance_replay_clocks(dimension);
    }
}

/// Block position helpers for journal entries.
#[must_use]
pub fn block_pos_array(pos: BlockPos) -> [i32; 3] {
    [pos.x, pos.y, pos.z]
}

/// Chunk position helpers for journal entries.
#[must_use]
pub fn chunk_pos_array(pos: ChunkPos) -> [i32; 2] {
    [pos.x, pos.z]
}

/// Entity id helpers for journal entries.
#[must_use]
pub fn entity_id_int(id: EntityId) -> i32 {
    id.0
}

/// Read a file fully (recovery helper).
#[must_use]
pub fn read_file_bytes(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}
