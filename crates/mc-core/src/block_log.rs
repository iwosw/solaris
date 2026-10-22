//! Block log (CoreProtect-like): attributed pre-images of block changes and
//! container transactions, with `/co inspect|lookup|rollback|restore`.
//!
//! Capture runs on the tick thread, first writer per cell per tick: the merge
//! loop attributes each accepted intent's staged writes to its player or mob
//! (`#creeper`, `#tnt`), fire attributes `#fire`, plugin block sets attribute
//! `#plugin:<id>`, and container clicks record per-slot deltas. The journal
//! seals them against the tick's final world into numbered records. Each
//! tick's new records and rollback marks ride the tick WAL
//! (`JournalEntry::BlockLog`); the native writer upserts them into the
//! `block_log` table before the checkpoint that may prune that WAL, so
//! recovery is table rows plus the WAL tail. Retention bounds memory and rows.
//!
//! Rollback and restore are one tick-thread command: every record is fenced
//! on its current block type or exact slot contents (a later change by
//! someone else is skipped, never overwritten), staged like `/fill`, and
//! journaled through the normal block/container post-images.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use mc_data::ItemStack;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use serde::{Deserialize, Serialize};

use crate::boundary::{BlockPos, ChunkPos, ConnectionId, EntityId, PlayerChange};
use crate::commands::CommandError;
use crate::config::CoreError;
use crate::dimensions::Dimension;
use crate::entities::EntityKind;
use crate::tick::TickState;

/// `[block_log]` operator policy. `enabled` gates capture only; retained
/// records stay queryable and are pruned by age and count either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    /// Records older than this are pruned; zero keeps them until `max_records`.
    pub retention_secs: u64,
    /// Per-dimension record cap; the oldest records are pruned first.
    pub max_records: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_secs: 30 * 24 * 60 * 60,
            max_records: 200_000,
        }
    }
}

/// Block-entity pre-image restored together with a rolled-back block.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EntityImage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots: Option<Vec<ItemStack>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbt: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Change {
    /// Global block states before and after the tick, plus the destroyed
    /// block entity (container slots / opaque NBT) when there was one.
    Block {
        before: u32,
        after: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<EntityImage>,
    },
    /// Changed container slots: `(slot, before, after)`.
    Container {
        slots: Vec<(u16, ItemStack, ItemStack)>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Per-dimension monotonic number; the table key and mark target.
    pub seq: u64,
    /// Unix seconds.
    pub time: u64,
    /// Player name or `#source`.
    pub actor: Arc<str>,
    pub pos: [i32; 3],
    pub change: Change,
    #[serde(default, skip_serializing_if = "is_false")]
    pub rolled_back: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One tick's journaled delta: new records plus rollback/restore marks.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Delta {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<Record>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<(u64, bool)>,
}

#[derive(Debug)]
struct PendingBlock {
    pos: BlockPos,
    actor: Arc<str>,
    before: u32,
    entity: Option<EntityImage>,
}

#[derive(Debug)]
struct PendingContainer {
    cell: (i32, i32, i32),
    actor: Arc<str>,
    slots: Vec<(u16, ItemStack, ItemStack)>,
}

/// Dimension-local log owned by its [`TickState`].
#[derive(Debug)]
pub struct BlockLog {
    settings: Settings,
    records: VecDeque<Record>,
    next_seq: u64,
    names: HashSet<Arc<str>>,
    blocks: Vec<PendingBlock>,
    noted: HashSet<BlockPos>,
    containers: Vec<PendingContainer>,
    marks: Vec<(u64, bool)>,
    unpersisted: Vec<Delta>,
    inspecting: HashSet<EntityId>,
    /// Block-entity pre-images installed after this tick's block defaults.
    restores: Vec<(BlockPos, EntityImage)>,
}

/// Fixtures that never hydrate from configuration do not capture.
impl Default for BlockLog {
    fn default() -> Self {
        Self::new(Settings {
            enabled: false,
            ..Settings::default()
        })
    }
}

impl BlockLog {
    pub(crate) fn new(settings: Settings) -> Self {
        Self {
            settings,
            records: VecDeque::new(),
            next_seq: 0,
            names: HashSet::default(),
            blocks: Vec::new(),
            noted: HashSet::default(),
            containers: Vec::new(),
            marks: Vec::new(),
            unpersisted: Vec::new(),
            inspecting: HashSet::default(),
            restores: Vec::new(),
        }
    }

    /// Rebuild from table rows followed by the WAL tail, in order.
    pub(crate) fn restore(settings: Settings, deltas: Vec<Delta>) -> Self {
        let mut records = BTreeMap::new();
        // A WAL-tail mark may outlive its evicted record; never reuse its seq.
        let mut next_seq = 0;
        for delta in deltas {
            for record in delta.records {
                next_seq = next_seq.max(record.seq + 1);
                records.insert(record.seq, record);
            }
            for (seq, rolled_back) in delta.marks {
                next_seq = next_seq.max(seq + 1);
                if let Some(record) = records.get_mut(&seq) {
                    record.rolled_back = rolled_back;
                }
            }
        }
        let mut log = Self::new(settings);
        log.next_seq = next_seq;
        for mut record in records.into_values() {
            record.actor = log.intern(&record.actor);
            log.records.push_back(record);
        }
        log.evict(unix_now());
        log
    }

    fn intern(&mut self, name: &str) -> Arc<str> {
        if let Some(name) = self.names.get(name) {
            return Arc::clone(name);
        }
        let name: Arc<str> = Arc::from(name);
        self.names.insert(Arc::clone(&name));
        name
    }

    pub(crate) fn evict(&mut self, now: u64) {
        while self.records.len() > self.settings.max_records {
            self.records.pop_front();
        }
        if self.settings.retention_secs > 0 {
            let cutoff = now.saturating_sub(self.settings.retention_secs);
            while self
                .records
                .front()
                .is_some_and(|record| record.time < cutoff)
            {
                self.records.pop_front();
            }
        }
    }

    /// Lowest retained sequence; every row below it may be deleted.
    fn floor(&self) -> u64 {
        self.records
            .front()
            .map_or(self.next_seq, |record| record.seq)
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

// ---------------------------------------------------------------- capture

/// Player name, or `#<entity type>` for mobs, TNT and projectiles.
fn actor(state: &mut TickState, entity: EntityId) -> Option<Arc<str>> {
    if let Some(name) = state.connections.name_of(entity) {
        return Some(state.block_log.intern(name));
    }
    let slot = state.entities.slot_of(entity)?;
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return None;
    };
    let name = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)?.name;
    let name = format!("#{}", name.strip_prefix("minecraft:").unwrap_or(name));
    Some(state.block_log.intern(&name))
}

/// Merge hook: attribute the writes one accepted intent staged from `from`.
pub(crate) fn note_merge(state: &mut TickState, entity: EntityId, from: usize) {
    if !state.block_log.settings.enabled || state.merge_block_writes.len() <= from {
        return;
    }
    let Some(actor) = actor(state, entity) else {
        return;
    };
    for index in from..state.merge_block_writes.len() {
        let pos = state.merge_block_writes[index].0;
        note(state, pos, &actor);
    }
}

/// Fire hook: burning a block away, spreading and burning out. Aging keeps
/// the block type and is not a change.
pub(crate) fn note_fire(state: &mut TickState, pos: BlockPos, next: u32) {
    if !state.block_log.settings.enabled {
        return;
    }
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return;
    };
    if same_block(&state.registry.blocks, current, next) {
        return;
    }
    let actor = state.block_log.intern("#fire");
    note(state, pos, &actor);
}

/// Plugin `world_blocks` hook, before the native write.
pub(crate) fn note_plugin(state: &mut TickState, pos: BlockPos, plugin: &str) {
    if !state.block_log.settings.enabled {
        return;
    }
    let actor = state.block_log.intern(&format!("#plugin:{plugin}"));
    note(state, pos, &actor);
}

fn note(state: &mut TickState, pos: BlockPos, actor: &Arc<str>) {
    if state.block_log.noted.contains(&pos) {
        return;
    }
    let Some(before) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return;
    };
    state.block_log.noted.insert(pos);
    let entity = entity_image(state, pos);
    state.block_log.blocks.push(PendingBlock {
        pos,
        actor: Arc::clone(actor),
        before,
        entity,
    });
}

fn entity_image(state: &TickState, pos: BlockPos) -> Option<EntityImage> {
    let slots = state
        .block_entities
        .get(&(pos.x, pos.y, pos.z))
        .map(|record| record.slots.clone());
    let nbt = state
        .world
        .chunk(chunk_of(pos))
        .and_then(|chunk| chunk.block_entities.get(&pos).cloned());
    (slots.is_some() || nbt.is_some()).then_some(EntityImage { slots, nbt })
}

fn chunk_of(pos: BlockPos) -> ChunkPos {
    ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    }
}

/// Container click hook: `menu` is the post-click image about to be written
/// back over `cells` (same split as the write-back).
pub(crate) fn note_container(
    state: &mut TickState,
    entity: EntityId,
    cells: &[(i32, i32, i32)],
    menu: &[ItemStack],
) {
    if !state.block_log.settings.enabled || cells.is_empty() {
        return;
    }
    let Some(name) = state.connections.name_of(entity) else {
        return;
    };
    let actor = state.block_log.intern(name);
    let mut rest = menu;
    for &cell in cells {
        let Some(record) = state.block_entities.get(&cell) else {
            continue;
        };
        let (mine, tail) = rest.split_at(record.slots.len().min(rest.len()));
        rest = tail;
        if record.slots.as_slice() == mine {
            continue;
        }
        let log = &mut state.block_log;
        let index = match log
            .containers
            .iter()
            .position(|pending| pending.cell == cell && pending.actor == actor)
        {
            Some(index) => index,
            None => {
                log.containers.push(PendingContainer {
                    cell,
                    actor: Arc::clone(&actor),
                    slots: Vec::new(),
                });
                log.containers.len() - 1
            }
        };
        let pending = &mut log.containers[index];
        for (slot, (before, after)) in record.slots.iter().zip(mine).enumerate() {
            if before == after {
                continue;
            }
            let slot = slot as u16;
            match pending.slots.iter_mut().find(|entry| entry.0 == slot) {
                Some(entry) => entry.2 = after.clone(),
                None => pending.slots.push((slot, before.clone(), after.clone())),
            }
        }
    }
}

/// Seal this tick's captures against the final world and journal them with
/// any rollback marks.
pub(crate) fn journal(state: &mut TickState, entries: &mut Vec<crate::journal::JournalEntry>) {
    let log = &state.block_log;
    if log.blocks.is_empty() && log.containers.is_empty() && log.marks.is_empty() {
        return;
    }
    let now = unix_now();
    let mut records = Vec::new();
    for pending in std::mem::take(&mut state.block_log.blocks) {
        let pos = pending.pos;
        let Some(after) = state.world.block_at(pos.x, pos.y, pos.z) else {
            continue;
        };
        if after == pending.before {
            continue;
        }
        records.push(Record {
            seq: 0,
            time: now,
            actor: pending.actor,
            pos: [pos.x, pos.y, pos.z],
            change: Change::Block {
                before: pending.before,
                after,
                entity: pending.entity,
            },
            rolled_back: false,
        });
    }
    let log = &mut state.block_log;
    log.noted.clear();
    for pending in std::mem::take(&mut log.containers) {
        let slots: Vec<_> = pending
            .slots
            .into_iter()
            .filter(|(_, before, after)| before != after)
            .collect();
        if !slots.is_empty() {
            records.push(Record {
                seq: 0,
                time: now,
                actor: pending.actor,
                pos: [pending.cell.0, pending.cell.1, pending.cell.2],
                change: Change::Container { slots },
                rolled_back: false,
            });
        }
    }
    for record in &mut records {
        record.seq = log.next_seq;
        log.next_seq += 1;
    }
    log.records.extend(records.iter().cloned());
    log.evict(now);
    let delta = Delta {
        records,
        marks: std::mem::take(&mut log.marks),
    };
    if delta.records.is_empty() && delta.marks.is_empty() {
        return;
    }
    entries.push(crate::journal::JournalEntry::BlockLog {
        value: Box::new(delta.clone()),
    });
    log.unpersisted.push(delta);
}

// ------------------------------------------------------------ persistence

const RECORD_ROW: u8 = 0;
const MARK_ROW: u8 = 1;

/// Native-writer work frozen with one checkpoint capture.
#[derive(Debug, Default)]
pub(crate) struct Persist {
    deltas: Vec<(Dimension, Delta)>,
    floors: Vec<(Dimension, u64)>,
}

/// Every dimension's deltas since the previous capture plus retention floors.
pub(crate) fn checkpoint(state: &mut TickState) -> Persist {
    let now = unix_now();
    let mut persist = Persist::default();
    capture(state, now, &mut persist);
    for world in state.other_dimensions.values_mut() {
        capture(world, now, &mut persist);
    }
    persist
}

fn capture(state: &mut TickState, now: u64, persist: &mut Persist) {
    let dimension = state.dimension;
    let log = &mut state.block_log;
    log.evict(now);
    persist
        .deltas
        .extend(log.unpersisted.drain(..).map(|delta| (dimension, delta)));
    persist.floors.push((dimension, log.floor()));
}

/// Native-writer retry state: rows stay queued until one table write
/// succeeds, so a failed checkpoint cannot drop records its WAL still covers.
#[derive(Debug, Default)]
pub(crate) struct Backlog {
    deltas: Vec<(Dimension, Delta)>,
    floors: BTreeMap<Dimension, u64>,
    written: BTreeMap<Dimension, u64>,
}

impl Backlog {
    pub(crate) fn absorb(&mut self, persist: Persist) {
        self.deltas.extend(persist.deltas);
        self.floors.extend(persist.floors);
    }

    /// Upsert queued rows and prune below the floors before the checkpoint
    /// commit; rows are keyed by sequence, so a retry is idempotent. Rows
    /// store block states and items by name, never by runtime id.
    pub(crate) fn write(
        &mut self,
        store: &mc_world::native::NativeStore,
        blocks: &mc_world::BlockRegistry,
        items: &mc_data::items::ItemRegistry,
    ) -> Result<(), CoreError> {
        if self.deltas.is_empty() && self.floors == self.written {
            return Ok(());
        }
        let mut rows = Vec::new();
        for (dimension, delta) in &self.deltas {
            for record in &delta.records {
                let value = encode_record(record, blocks, items)
                    .map_err(|error| CoreError::Storage(format!("block log: {error}")))?;
                rows.push((key(RECORD_ROW, *dimension, record.seq), value));
            }
            for &(seq, rolled_back) in &delta.marks {
                rows.push((key(MARK_ROW, *dimension, seq), vec![u8::from(rolled_back)]));
            }
        }
        let prune: Vec<_> = self
            .floors
            .iter()
            .flat_map(|(&dimension, &floor)| {
                [RECORD_ROW, MARK_ROW]
                    .map(|kind| (key(kind, dimension, 0), key(kind, dimension, floor)))
            })
            .collect();
        store
            .write_block_log(&rows, &prune)
            .map_err(|error| CoreError::Storage(format!("block log: {error}")))?;
        self.deltas.clear();
        self.written.clone_from(&self.floors);
        Ok(())
    }
}

fn key(kind: u8, dimension: Dimension, seq: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(10);
    key.push(kind);
    key.push(crate::native_checkpoint::dimension_id(dimension) as u8);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

/// Checkpointed rows per dimension, records before marks. Like the native
/// block dictionary, a row naming a block state or item this version does
/// not know, or a malformed row, fails closed.
pub(crate) fn load(
    reader: &mc_world::native::NativeCheckpointReader,
    blocks: &mc_world::BlockRegistry,
    items: &mc_data::items::ItemRegistry,
) -> Result<BTreeMap<Dimension, Delta>, CoreError> {
    let mut out: BTreeMap<Dimension, Delta> = BTreeMap::new();
    reader
        .visit_block_log(|key, value| {
            let invalid = |reason: &str| {
                mc_world::native::NativeError::Source(format!("block log row: {reason}"))
            };
            let [kind, dimension, seq @ ..] = key else {
                return Err(invalid("short key"));
            };
            let seq: [u8; 8] = seq.try_into().map_err(|_| invalid("key length"))?;
            let seq = u64::from_be_bytes(seq);
            let dimension = match dimension {
                0 => Dimension::Overworld,
                1 => Dimension::Nether,
                2 => Dimension::End,
                _ => return Err(invalid("dimension")),
            };
            let delta = out.entry(dimension).or_default();
            match (*kind, value) {
                (RECORD_ROW, _) => delta.records.push(
                    decode_record(value, seq, blocks, items)
                        .map_err(|error| invalid(&format!("{dimension:?} seq {seq}: {error}")))?,
                ),
                (MARK_ROW, [flag @ (0 | 1)]) => delta.marks.push((seq, *flag == 1)),
                _ => return Err(invalid("kind")),
            }
            Ok(())
        })
        .map_err(|error| CoreError::Storage(error.to_string()))?;
    Ok(out)
}

/// One record as an NBT compound: block states as `{Name, Properties}`
/// (the Anvil palette form) and items as vanilla item compounds.
fn encode_record(
    record: &Record,
    blocks: &mc_world::BlockRegistry,
    items: &mc_data::items::ItemRegistry,
) -> Result<Vec<u8>, String> {
    use mc_nbt::Tag;
    let mut fields = vec![
        ("time".into(), Tag::Long(record.time as i64)),
        ("actor".into(), Tag::String(record.actor.to_string())),
        ("pos".into(), Tag::IntArray(record.pos.to_vec())),
        (
            "rolled_back".into(),
            Tag::Byte(i8::from(record.rolled_back)),
        ),
    ];
    match &record.change {
        Change::Block {
            before,
            after,
            entity,
        } => {
            fields.push(("before".into(), state_tag(blocks, *before)?));
            fields.push(("after".into(), state_tag(blocks, *after)?));
            if let Some(entity) = entity {
                if let Some(slots) = &entity.slots {
                    fields.push(("size".into(), Tag::Int(slots.len() as i32)));
                    let mut stacks = Vec::new();
                    for (slot, stack) in slots.iter().enumerate() {
                        if let Some(Tag::Compound(mut item)) = item_tag(items, stack)? {
                            item.push(("Slot".into(), Tag::Int(slot as i32)));
                            stacks.push(Tag::Compound(item));
                        }
                    }
                    fields.push(("slots".into(), compound_list(stacks)));
                }
                if let Some(nbt) = &entity.nbt {
                    fields.push((
                        "nbt".into(),
                        Tag::ByteArray(nbt.iter().map(|byte| *byte as i8).collect()),
                    ));
                }
            }
        }
        Change::Container { slots } => {
            let mut changes = Vec::with_capacity(slots.len());
            for (slot, before, after) in slots {
                let mut change = vec![("Slot".into(), Tag::Int(i32::from(*slot)))];
                for (name, stack) in [("before", before), ("after", after)] {
                    if let Some(item) = item_tag(items, stack)? {
                        change.push((name.into(), item));
                    }
                }
                changes.push(Tag::Compound(change));
            }
            fields.push(("container".into(), compound_list(changes)));
        }
    }
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &Tag::Compound(fields)).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn decode_record(
    bytes: &[u8],
    seq: u64,
    blocks: &mc_world::BlockRegistry,
    items: &mc_data::items::ItemRegistry,
) -> Result<Record, String> {
    use mc_nbt::Tag;
    let mut input = bytes;
    let (_, root) = mc_nbt::read_named(&mut input).map_err(|e| e.to_string())?;
    let Tag::Compound(fields) = root else {
        return Err("record is not a compound".into());
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    };
    let (Some(Tag::Long(time)), Some(Tag::String(actor)), Some(Tag::IntArray(pos))) =
        (field("time"), field("actor"), field("pos"))
    else {
        return Err("record header".into());
    };
    let pos: [i32; 3] = pos.as_slice().try_into().map_err(|_| "record position")?;
    let change = if let Some(Tag::List(changes)) = field("container") {
        let mut slots = Vec::with_capacity(changes.elements.len());
        for change in &changes.elements {
            let Tag::Compound(change) = change else {
                return Err("container change".into());
            };
            let get = |name: &str| {
                change
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, tag)| tag)
            };
            let Some(Tag::Int(slot)) = get("Slot") else {
                return Err("container slot".into());
            };
            let (before, after) = (
                item_from(items, get("before"))?,
                item_from(items, get("after"))?,
            );
            slots.push((
                u16::try_from(*slot).map_err(|_| "container slot")?,
                before,
                after,
            ));
        }
        Change::Container { slots }
    } else {
        let (Some(before), Some(after)) = (field("before"), field("after")) else {
            return Err("block change".into());
        };
        let (before, after) = (state_from(blocks, before)?, state_from(blocks, after)?);
        let slots = match (field("size"), field("slots")) {
            (Some(Tag::Int(size)), Some(Tag::List(stacks))) => {
                let mut slots =
                    vec![ItemStack::EMPTY; usize::try_from(*size).map_err(|_| "slot count")?];
                for stack in &stacks.elements {
                    let Tag::Compound(item) = stack else {
                        return Err("entity slot".into());
                    };
                    let Some((_, Tag::Int(slot))) = item.iter().find(|(key, _)| key == "Slot")
                    else {
                        return Err("entity slot index".into());
                    };
                    let Some(target) = usize::try_from(*slot)
                        .ok()
                        .and_then(|slot| slots.get_mut(slot))
                    else {
                        return Err("entity slot index".into());
                    };
                    *target = item_from(items, Some(stack))?;
                }
                Some(slots)
            }
            (None, None) => None,
            _ => return Err("entity slots".into()),
        };
        let nbt = match field("nbt") {
            Some(Tag::ByteArray(bytes)) => Some(bytes.iter().map(|byte| *byte as u8).collect()),
            None => None,
            _ => return Err("entity nbt".into()),
        };
        Change::Block {
            before,
            after,
            entity: (slots.is_some() || nbt.is_some()).then_some(EntityImage { slots, nbt }),
        }
    };
    Ok(Record {
        seq,
        time: *time as u64,
        actor: Arc::from(actor.as_str()),
        pos,
        change,
        rolled_back: matches!(field("rolled_back"), Some(Tag::Byte(1))),
    })
}

fn compound_list(elements: Vec<mc_nbt::Tag>) -> mc_nbt::Tag {
    mc_nbt::Tag::List(if elements.is_empty() {
        mc_nbt::ListTag::empty()
    } else {
        mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::COMPOUND,
            elements,
        }
    })
}

fn state_tag(blocks: &mc_world::BlockRegistry, id: u32) -> Result<mc_nbt::Tag, String> {
    use mc_nbt::Tag;
    let state = blocks
        .by_id(mc_world::BlockStateId(id))
        .ok_or_else(|| format!("unknown block state {id}"))?;
    Ok(Tag::Compound(vec![
        (
            "Name".into(),
            Tag::String(state.block.id.as_str().to_owned()),
        ),
        (
            "Properties".into(),
            Tag::Compound(
                state
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), Tag::String(value.clone())))
                    .collect(),
            ),
        ),
    ]))
}

/// The exact recorded state, like the native block dictionary: an unknown
/// block, property or value is an error naming the stored state.
fn state_from(blocks: &mc_world::BlockRegistry, tag: &mc_nbt::Tag) -> Result<u32, String> {
    use mc_nbt::Tag;
    let Tag::Compound(fields) = tag else {
        return Err("block state is not a compound".into());
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    };
    let Some(Tag::String(name)) = field("Name") else {
        return Err("block state name".into());
    };
    let mut properties = Vec::new();
    if let Some(Tag::Compound(values)) = field("Properties") {
        for (key, value) in values {
            let Tag::String(value) = value else {
                return Err(format!("block state {name} property {key}"));
            };
            properties.push((key.clone(), value.clone()));
        }
    }
    let unknown = || format!("unknown block state {name} {properties:?}");
    let identifier = mc_data::Identifier::parse(name.as_str()).map_err(|_| unknown())?;
    let state = blocks
        .by_name_and_props(&identifier, &properties)
        .ok_or_else(unknown)?;
    // `by_name_and_props` ignores extra keys; the stored set must be exact.
    let resolved = blocks.by_id(state).ok_or_else(unknown)?;
    if resolved.properties.len() != properties.len() {
        return Err(unknown());
    }
    Ok(state.0)
}

/// Vanilla item compound; `None` for an empty stack.
fn item_tag(
    items: &mc_data::items::ItemRegistry,
    stack: &ItemStack,
) -> Result<Option<mc_nbt::Tag>, String> {
    if stack.is_empty() {
        return Ok(None);
    }
    mc_data::item_nbt::stack_to_fields(items, stack)
        .map(|fields| Some(mc_nbt::Tag::Compound(fields)))
        .map_err(|error| error.to_string())
}

/// An absent tag is an empty slot; an unknown item is an error naming it.
fn item_from(
    items: &mc_data::items::ItemRegistry,
    tag: Option<&mc_nbt::Tag>,
) -> Result<ItemStack, String> {
    let Some(tag) = tag else {
        return Ok(ItemStack::EMPTY);
    };
    let mc_nbt::Tag::Compound(fields) = tag else {
        return Err("item is not a compound".into());
    };
    mc_data::item_nbt::stack_from_fields(items, fields)
        .map(|stack| stack.unwrap_or(ItemStack::EMPTY))
        .map_err(|error| error.to_string())
}

// -------------------------------------------------------------- commands

const DEFAULT_RADIUS: i32 = 10;
const PAGE_LINES: usize = 10;
const USAGE: &str = "Usage: /co inspect | /co lookup [u:<players>] [t:<time>] [r:<radius>] [page] | /co rollback|restore [u:<players>] t:<time> [r:<radius>]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Help,
    Inspect,
    Lookup,
    Rollback,
    Restore,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockLogCommand {
    action: Action,
    /// Lower-cased player names or `#sources`.
    users: Vec<String>,
    since_secs: Option<u64>,
    radius: Option<i32>,
    page: usize,
}

pub(crate) fn parse(args: &[&str]) -> Result<BlockLogCommand, CommandError> {
    let usage = || CommandError::Message(USAGE.to_owned());
    let (action, rest) = match args.split_first() {
        None => (Action::Help, &[][..]),
        Some((word, rest)) => (
            match *word {
                "help" => Action::Help,
                "i" | "inspect" => Action::Inspect,
                "l" | "lookup" => Action::Lookup,
                "rb" | "rollback" => Action::Rollback,
                "rs" | "restore" => Action::Restore,
                _ => return Err(usage()),
            },
            rest,
        ),
    };
    if matches!(action, Action::Help | Action::Inspect) && !rest.is_empty() {
        return Err(usage());
    }
    let mut command = BlockLogCommand {
        action,
        users: Vec::new(),
        since_secs: None,
        radius: None,
        page: 1,
    };
    for word in rest {
        if let Some(users) = word.strip_prefix("u:") {
            command.users.extend(
                users
                    .split(',')
                    .filter(|user| !user.is_empty())
                    .map(str::to_ascii_lowercase),
            );
        } else if let Some(time) = word.strip_prefix("t:") {
            command.since_secs = Some(duration(time).ok_or_else(|| {
                CommandError::Message(format!(
                    "Invalid time {time:?}; use e.g. t:30m, t:1h30m, t:2d"
                ))
            })?);
        } else if let Some(radius) = word.strip_prefix("r:") {
            command.radius = Some(
                radius
                    .parse::<i32>()
                    .ok()
                    .filter(|radius| *radius >= 0)
                    .ok_or_else(|| CommandError::Message(format!("Invalid radius {radius:?}")))?,
            );
        } else if action == Action::Lookup
            && let Ok(page) = word.parse::<usize>()
            && page > 0
        {
            command.page = page;
        } else {
            return Err(usage());
        }
    }
    if matches!(action, Action::Rollback | Action::Restore) && command.since_secs.is_none() {
        return Err(CommandError::Message(
            "Specify how far back to go with t:<time>, e.g. t:1h".to_owned(),
        ));
    }
    Ok(command)
}

/// `1h30m`, `2d`, `45s`, `1w`, `2.5h`.
fn duration(text: &str) -> Option<u64> {
    let mut total = 0.0_f64;
    let mut number = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() || character == '.' {
            number.push(character);
            continue;
        }
        let unit = match character {
            'w' => 604_800.0,
            'd' => 86_400.0,
            'h' => 3_600.0,
            'm' => 60.0,
            's' => 1.0,
            _ => return None,
        };
        total += number.parse::<f64>().ok()? * unit;
        number.clear();
    }
    (number.is_empty() && total > 0.0 && total.is_finite()).then_some(total as u64)
}

pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &BlockLogCommand,
) {
    let lines = run(state, executor, command).unwrap_or_else(|error| vec![error]);
    for message in lines {
        state.push_player(connection, PlayerChange::Chat { message });
    }
}

fn run(
    state: &mut TickState,
    executor: EntityId,
    command: &BlockLogCommand,
) -> Result<Vec<String>, String> {
    let now = unix_now();
    match command.action {
        Action::Help => Ok(vec![USAGE.to_owned()]),
        Action::Inspect => {
            if !state.players.contains_key(&executor) {
                return Err("Only players can inspect".to_owned());
            }
            Ok(vec![if state.block_log.inspecting.insert(executor) {
                "Inspector enabled: click a block to see its history".to_owned()
            } else {
                state.block_log.inspecting.remove(&executor);
                "Inspector disabled".to_owned()
            }])
        }
        Action::Lookup => {
            let selected = select(state, executor, command, now)?;
            let pages = selected.len().div_ceil(PAGE_LINES).max(1);
            let page = command.page.min(pages);
            let mut lines = vec![format!(
                "----- Block log: {} result(s), page {page}/{pages} -----",
                selected.len()
            )];
            lines.extend(
                selected
                    .iter()
                    .rev()
                    .skip((page - 1) * PAGE_LINES)
                    .take(PAGE_LINES)
                    .map(|&index| describe(state, &state.block_log.records[index], now)),
            );
            Ok(lines)
        }
        Action::Rollback | Action::Restore => {
            let restore = command.action == Action::Restore;
            let selected: Vec<usize> = select(state, executor, command, now)?
                .into_iter()
                .filter(|&index| state.block_log.records[index].rolled_back == restore)
                .collect();
            let (applied, skipped) = apply(state, &selected, restore)?;
            Ok(vec![format!(
                "{} complete: {applied} change(s) {}, {skipped} skipped (changed since)",
                if restore { "Restore" } else { "Rollback" },
                if restore { "restored" } else { "rolled back" },
            )])
        }
    }
}

/// Indices of matching records in ascending sequence order.
fn select(
    state: &TickState,
    executor: EntityId,
    command: &BlockLogCommand,
    now: u64,
) -> Result<Vec<usize>, String> {
    let radius = command.radius.or_else(|| {
        (command.users.is_empty() && command.action != Action::Lookup).then_some(DEFAULT_RADIUS)
    });
    let center = match radius {
        Some(_) => Some(
            crate::command_console::pose(state, executor)
                .filter(|_| executor != crate::command_console::EXECUTOR)
                .ok_or("A radius needs an in-world executor; give u:<player> instead")?
                .0,
        ),
        None => None,
    };
    let cutoff = command.since_secs.map(|since| now.saturating_sub(since));
    Ok(state
        .block_log
        .records
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            cutoff.is_none_or(|cutoff| record.time >= cutoff)
                && (command.users.is_empty()
                    || command
                        .users
                        .iter()
                        .any(|user| record.actor.eq_ignore_ascii_case(user)))
                && center.zip(radius).is_none_or(|(center, radius)| {
                    (record.pos[0] - center[0].floor() as i32).abs() <= radius
                        && (record.pos[2] - center[2].floor() as i32).abs() <= radius
                })
        })
        .map(|(index, _)| index)
        .collect())
}

/// Revert (`restore == false`, newest first) or re-apply (oldest first) the
/// selected records in this tick. Unloaded cells refuse the whole command.
fn apply(
    state: &mut TickState,
    selected: &[usize],
    restore: bool,
) -> Result<(usize, usize), String> {
    let limit = crate::command_rules::integer(state, "max_block_modifications").max(0) as usize;
    if selected.len() > limit {
        return Err(format!(
            "Too many changes: {} (maximum {limit}); narrow u:, t: or r:",
            selected.len()
        ));
    }
    if selected.iter().any(|&index| {
        let [x, y, z] = state.block_log.records[index].pos;
        state.world.block_at(x, y, z).is_none()
    }) {
        return Err("Part of the area is not loaded; stand closer or narrow r:".to_owned());
    }
    let mut staged: HashMap<BlockPos, usize> = state
        .merge_block_writes
        .iter()
        .enumerate()
        .map(|(index, (pos, _))| (*pos, index))
        .collect();
    let mut touched = Vec::new();
    let (mut applied, mut skipped) = (0, 0);
    let order: Vec<usize> = if restore {
        selected.to_vec()
    } else {
        selected.iter().rev().copied().collect()
    };
    for index in order {
        let record = state.block_log.records[index].clone();
        let [x, y, z] = record.pos;
        let pos = BlockPos { x, y, z };
        let done = match record.change {
            Change::Block {
                before,
                after,
                entity,
            } => {
                let (from, to) = if restore {
                    (before, after)
                } else {
                    (after, before)
                };
                revert_block(
                    state,
                    &mut staged,
                    pos,
                    from,
                    to,
                    entity.filter(|_| !restore),
                )
            }
            Change::Container { slots } => {
                revert_container(state, pos, &slots, restore, &mut touched)
            }
        };
        if done {
            state.block_log.records[index].rolled_back = !restore;
            state.block_log.marks.push((record.seq, !restore));
            applied += 1;
        } else {
            skipped += 1;
        }
    }
    touched.sort_unstable_by_key(|pos| (pos.x, pos.y, pos.z));
    touched.dedup();
    for pos in touched {
        container_changed(state, pos);
    }
    Ok((applied, skipped))
}

fn revert_block(
    state: &mut TickState,
    staged: &mut HashMap<BlockPos, usize>,
    pos: BlockPos,
    from: u32,
    to: u32,
    entity: Option<EntityImage>,
) -> bool {
    let current = staged
        .get(&pos)
        .map(|&index| state.merge_block_writes[index].1)
        .or_else(|| state.world.block_at(pos.x, pos.y, pos.z));
    let Some(current) = current else {
        return false;
    };
    if !same_block(&state.registry.blocks, current, from) {
        return false;
    }
    state.block_log.restores.retain(|(cell, _)| *cell != pos);
    if current == to {
        return true;
    }
    let family_changes = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))
        .zip(state.registry.blocks.by_id(mc_world::BlockStateId(to)))
        .is_none_or(|(old, new)| {
            old.block.id != new.block.id
                && !crate::copper::same_family(&old.block.id, &new.block.id)
        });
    if family_changes {
        state.stage_command_container_removal(pos);
    }
    match staged.get(&pos) {
        Some(&index) => state.merge_block_writes[index].1 = to,
        None => {
            staged.insert(pos, state.merge_block_writes.len());
            state.merge_block_writes.push((pos, to));
        }
    }
    // Later commands and summons in this input batch see the edited world.
    state.world.set_block(pos.x, pos.y, pos.z, to);
    if let Some(entity) = entity {
        state.block_log.restores.push((pos, entity));
    }
    true
}

/// All-or-nothing per record: every slot must still hold the image this
/// record left (or, for restore, found).
fn revert_container(
    state: &mut TickState,
    pos: BlockPos,
    slots: &[(u16, ItemStack, ItemStack)],
    restore: bool,
    touched: &mut Vec<BlockPos>,
) -> bool {
    let swap = |current: &mut Vec<ItemStack>| {
        let fenced = slots.iter().all(|(slot, before, after)| {
            current.get(usize::from(*slot)) == Some(if restore { before } else { after })
        });
        if fenced {
            for (slot, before, after) in slots {
                current[usize::from(*slot)] = if restore { after } else { before }.clone();
            }
        }
        fenced
    };
    // A chest this same rollback re-creates is the current storage.
    if let Some(current) = state
        .block_log
        .restores
        .iter_mut()
        .find(|(cell, _)| *cell == pos)
        .and_then(|(_, image)| image.slots.as_mut())
    {
        return swap(current);
    }
    let cell = (pos.x, pos.y, pos.z);
    let Some(record) = state.block_entities.get(&cell) else {
        return false;
    };
    let mut slots_now = record.slots.clone();
    if !swap(&mut slots_now) {
        return false;
    }
    if let Some(record) = state.block_entities.get_mut(&cell) {
        record.slots = slots_now;
    }
    touched.push(pos);
    true
}

/// Side effects of a native container write (same as a menu write-back).
fn container_changed(state: &mut TickState, pos: BlockPos) {
    let cell = (pos.x, pos.y, pos.z);
    if let Some(record) = state.block_entities.get(&cell) {
        crate::brewing::slots_replaced(&mut state.brewing, cell, &record.slots);
    }
    crate::redstone::containers::inventory_changed(&mut state.redstone_containers, pos);
    state.world.mark_block_entity_dirty(pos);
    crate::tick::refresh_container_viewers(state, pos);
}

/// Install rolled-back block-entity pre-images over the defaults the block
/// phase created (after `block_entity_defaults::after_blocks`).
pub(crate) fn after_blocks(state: &mut TickState) {
    if state.block_log.restores.is_empty() {
        return;
    }
    for (pos, image) in std::mem::take(&mut state.block_log.restores) {
        if let Some(slots) = image.slots
            && let Some(record) = state.block_entities.get_mut(&(pos.x, pos.y, pos.z))
            && record.slots.len() == slots.len()
        {
            record.slots = slots;
            container_changed(state, pos);
        }
        if let Some(nbt) = image.nbt
            && state
                .world
                .chunk(chunk_of(pos))
                .is_some_and(|chunk| chunk.block_entities.contains_key(&pos))
        {
            state.world.set_opaque_block_entity(pos, Some(nbt.clone()));
            state.opaque_changes.push((pos, Some(nbt)));
        }
    }
}

/// Inspector click: print the history at `pos` instead of acting on it.
/// Returns whether the click was consumed; `print` is false for the dig
/// follow-ups (finish/abort) of a click that was already answered.
pub(crate) fn inspect(
    state: &mut TickState,
    connection: ConnectionId,
    entity: EntityId,
    pos: BlockPos,
    print: bool,
) -> bool {
    if !state.block_log.inspecting.contains(&entity) {
        return false;
    }
    if !print {
        return true;
    }
    let now = unix_now();
    let mut lines = vec![format!(
        "----- Block log at {} {} {} -----",
        pos.x, pos.y, pos.z
    )];
    lines.extend(
        state
            .block_log
            .records
            .iter()
            .rev()
            .filter(|record| record.pos == [pos.x, pos.y, pos.z])
            .take(PAGE_LINES)
            .map(|record| describe(state, record, now)),
    );
    if lines.len() == 1 {
        lines.push("No changes recorded here".to_owned());
    }
    for message in lines {
        state.push_player(connection, PlayerChange::Chat { message });
    }
    true
}

// ----------------------------------------------------------- presentation

fn describe(state: &TickState, record: &Record, now: u64) -> String {
    let age = now.saturating_sub(record.time);
    let ago = match age {
        0..60 => format!("{age}s"),
        60..3_600 => format!("{:.1}m", age as f64 / 60.0),
        3_600..86_400 => format!("{:.2}h", age as f64 / 3_600.0),
        _ => format!("{:.2}d", age as f64 / 86_400.0),
    };
    let what = match &record.change {
        Change::Block { before, after, .. } => {
            let name = |id: u32| {
                state
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(id))
                    .map_or_else(
                        || format!("#{id}"),
                        |block| block.block.id.path().to_owned(),
                    )
            };
            if is_air(&state.registry.blocks, *after) {
                format!("broke {}", name(*before))
            } else if is_air(&state.registry.blocks, *before) {
                format!("placed {}", name(*after))
            } else {
                format!("changed {} to {}", name(*before), name(*after))
            }
        }
        Change::Container { slots } => {
            let mut net: BTreeMap<String, i64> = BTreeMap::new();
            let item = |stack: &ItemStack| {
                state
                    .registry
                    .items
                    .name_of(stack.item_id)
                    .map_or_else(|| format!("#{}", stack.item_id), |id| id.path().to_owned())
            };
            for (_, before, after) in slots {
                if !before.is_empty() {
                    *net.entry(item(before)).or_default() -= i64::from(before.count);
                }
                if !after.is_empty() {
                    *net.entry(item(after)).or_default() += i64::from(after.count);
                }
            }
            let parts: Vec<String> = net
                .into_iter()
                .filter(|(_, count)| *count != 0)
                .map(|(name, count)| {
                    if count < 0 {
                        format!("took {} {name}", -count)
                    } else {
                        format!("added {count} {name}")
                    }
                })
                .collect();
            if parts.is_empty() {
                "rearranged a container".to_owned()
            } else {
                parts.join(", ")
            }
        }
    };
    let [x, y, z] = record.pos;
    format!(
        "{ago} ago - {} {what} at {x} {y} {z}{}",
        record.actor,
        if record.rolled_back {
            " (rolled back)"
        } else {
            ""
        }
    )
}

fn is_air(blocks: &mc_world::BlockRegistry, state: u32) -> bool {
    blocks
        .by_id(mc_world::BlockStateId(state))
        .is_some_and(|block| matches!(block.block.id.path(), "air" | "cave_air" | "void_air"))
}

/// Block type equality (all air variants are one type): fire aging, door
/// swings and redstone power keep the type a fence compares.
fn same_block(blocks: &mc_world::BlockRegistry, a: u32, b: u32) -> bool {
    a == b
        || match (
            blocks.by_id(mc_world::BlockStateId(a)),
            blocks.by_id(mc_world::BlockStateId(b)),
        ) {
            (Some(a), Some(b)) => {
                a.block.id == b.block.id || (is_air(blocks, a.id.0) && is_air(blocks, b.id.0))
            }
            _ => false,
        }
}

#[cfg(test)]
#[path = "block_log_tests.rs"]
mod tests;
