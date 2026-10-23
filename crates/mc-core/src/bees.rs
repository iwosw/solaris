//! Bee pollination, hive custody and harvesting on the native tick owner.
//! Save keys and metadata verified against the 26.1.2 Bee/BeehiveBlockEntity bytecode.
use crate::boundary::{BlockPos, ChangeSet, EntityDataValue, EntityId, Hand};
use crate::config::CoreError;
use crate::entities::EntitySlot;
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use mc_nbt::{ListTag, Tag};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

static BEE_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| crate::piglin::type_id_of("minecraft:bee"));

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeeState {
    #[serde(with = "position_serde")]
    pub hive: Option<BlockPos>,
    #[serde(with = "position_serde")]
    pub flower: Option<BlockPos>,
    pub nectar: bool,
    pub stung: bool,
    pub anger_end: i64,
    pub angry_at: Option<uuid::Uuid>,
    pub pollination: u32,
    pub ticks_since_pollination: u32,
    pub cannot_enter: u32,
    pub crops: u32,
    pub sting_ticks: u32,
    #[serde(skip)]
    pub search_cooldown_until: u64,
}
impl BeeState {
    fn validate(&self) -> Result<(), &'static str> {
        if self.ticks_since_pollination > i32::MAX as u32 || self.cannot_enter > i32::MAX as u32 {
            return Err("Bee: timers expected nonnegative int");
        }
        if self.pollination > 400 {
            return Err("Bee.solaris:BeePollinationTicks: expected 0..=400");
        }
        if self.crops > 10 {
            return Err("Bee.CropsGrownSincePollination: expected 0..=10");
        }
        if self.sting_ticks > 1200 {
            return Err("Bee.sting_ticks: expected 0..=1200");
        }
        Ok(())
    }
}
pub(crate) fn deserialize<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<BeeState>, D::Error> {
    let value = Option::<BeeState>::deserialize(d)?;
    if let Some(v) = &value {
        v.validate().map_err(serde::de::Error::custom)?;
    }
    Ok(value)
}

pub(crate) mod position_serde {
    use super::*;
    pub fn serialize<S: serde::Serializer>(p: &Option<BlockPos>, s: S) -> Result<S::Ok, S::Error> {
        p.map(|p| [p.x, p.y, p.z]).serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<BlockPos>, D::Error> {
        Ok(Option::<[i32; 3]>::deserialize(d)?.map(|[x, y, z]| BlockPos { x, y, z }))
    }
}
pub(crate) fn field<'a>(
    fields: &'a [(String, Tag)],
    key: &str,
) -> Result<Option<&'a Tag>, CoreError> {
    let mut values = fields.iter().filter(|(name, _)| name == key);
    let value = values.next().map(|(_, value)| value);
    if values.next().is_some() {
        return Err(fail(key, "duplicate field"));
    }
    Ok(value)
}
pub(crate) fn fail(key: &str, message: &str) -> CoreError {
    CoreError::Storage(format!("{key}: {message}"))
}
pub(crate) fn position(tag: &Tag, key: &str) -> Result<BlockPos, CoreError> {
    match tag {
        Tag::IntArray(v) if v.len() == 3 => Ok(BlockPos {
            x: v[0],
            y: v[1],
            z: v[2],
        }),
        _ => Err(fail(key, "expected three-int position")),
    }
}
pub(crate) fn pos_tag(p: BlockPos) -> Tag {
    Tag::IntArray(vec![p.x, p.y, p.z])
}
pub(crate) fn uuid(tag: &Tag, key: &str) -> Result<uuid::Uuid, CoreError> {
    let Tag::IntArray(words) = tag else {
        return Err(fail(key, "expected four-int UUID"));
    };
    if words.len() != 4 {
        return Err(fail(key, "expected four-int UUID"));
    }
    let mut bytes = [0; 16];
    for (out, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        out.copy_from_slice(&word.to_be_bytes());
    }
    Ok(uuid::Uuid::from_bytes(bytes))
}
fn integer(fields: &[(String, Tag)], key: &str, default: i32) -> Result<i32, CoreError> {
    match field(fields, key)? {
        None => Ok(default),
        Some(Tag::Int(v)) if *v >= 0 => Ok(*v),
        _ => Err(fail(key, "expected nonnegative int")),
    }
}
fn boolean(fields: &[(String, Tag)], key: &str) -> Result<bool, CoreError> {
    match field(fields, key)? {
        None => Ok(false),
        Some(Tag::Byte(v @ (0 | 1))) => Ok(*v != 0),
        _ => Err(fail(key, "expected boolean byte")),
    }
}
pub(crate) fn from_fields(fields: &[(String, Tag)]) -> Result<BeeState, CoreError> {
    let mut value = BeeState {
        hive: field(fields, "hive_pos")?
            .map(|t| position(t, "Bee.hive_pos"))
            .transpose()?,
        flower: field(fields, "flower_pos")?
            .map(|t| position(t, "Bee.flower_pos"))
            .transpose()?,
        nectar: boolean(fields, "HasNectar")?,
        stung: boolean(fields, "HasStung")?,
        ticks_since_pollination: integer(fields, "TicksSincePollination", 0)? as u32,
        pollination: integer(fields, "solaris:BeePollinationTicks", 0)? as u32,
        cannot_enter: integer(fields, "CannotEnterHiveTicks", 0)? as u32,
        crops: integer(fields, "CropsGrownSincePollination", 0)? as u32,
        anger_end: match field(fields, "anger_end_time")? {
            None => -1,
            Some(Tag::Long(v)) => *v,
            _ => return Err(fail("Bee.anger_end_time", "expected long")),
        },
        angry_at: field(fields, "angry_at")?
            .map(|t| uuid(t, "Bee.angry_at"))
            .transpose()?,
        ..Default::default()
    };
    if let Some(tag) = field(fields, "solaris:BeeStingTicks")? {
        let Tag::Int(ticks) = tag else {
            return Err(fail("Bee.sting_ticks", "expected int"));
        };
        value.sting_ticks =
            u32::try_from(*ticks).map_err(|_| fail("Bee.sting_ticks", "expected 0..=1200"))?;
    }
    value.validate().map_err(|e| fail("Bee", e))?;
    Ok(value)
}
pub(crate) fn from_record(
    record: &mc_data::entity_nbt::VanillaEntity,
) -> Result<Option<BeeState>, CoreError> {
    if record.id == "minecraft:bee" {
        from_fields(&record.extra_fields).map(Some)
    } else {
        Ok(None)
    }
}
pub(crate) fn to_fields(value: &BeeState) -> Vec<(String, Tag)> {
    let mut fields = vec![
        ("HasNectar".into(), Tag::Byte(i8::from(value.nectar))),
        ("HasStung".into(), Tag::Byte(i8::from(value.stung))),
        (
            "TicksSincePollination".into(),
            Tag::Int(value.ticks_since_pollination as i32),
        ),
        (
            "CannotEnterHiveTicks".into(),
            Tag::Int(value.cannot_enter as i32),
        ),
        (
            "CropsGrownSincePollination".into(),
            Tag::Int(value.crops as i32),
        ),
        ("anger_end_time".into(), Tag::Long(value.anger_end)),
        (
            "solaris:BeeStingTicks".into(),
            Tag::Int(value.sting_ticks as i32),
        ),
        (
            "solaris:BeePollinationTicks".into(),
            Tag::Int(value.pollination as i32),
        ),
    ];
    if let Some(p) = value.hive {
        fields.push(("hive_pos".into(), pos_tag(p)));
    }
    if let Some(p) = value.flower {
        fields.push(("flower_pos".into(), pos_tag(p)));
    }
    if let Some(id) = value.angry_at {
        fields.push((
            "angry_at".into(),
            crate::special_mobs::persistence::uuid_tag(id),
        ));
    }
    fields
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Occupant {
    data: Vec<(String, Tag)>,
    ticks: i32,
    minimum: i32,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Hive {
    bees: Vec<Occupant>,
    flower: Option<BlockPos>,
    // Accepted item placement is persisted after its block write reaches the world.
    pending_placement: bool,
}
pub(crate) type Hives = BTreeMap<(i32, i32, i32), Hive>;
fn key(p: BlockPos) -> (i32, i32, i32) {
    (p.x, p.y, p.z)
}
impl Hive {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let mut input = bytes;
        let tag = mc_nbt::read_network(&mut input).map_err(|e| fail("Beehive", &e.to_string()))?;
        if !input.is_empty() {
            return Err(fail("Beehive", "trailing NBT bytes"));
        }
        let Tag::Compound(fields) = tag else {
            return Err(fail("Beehive", "expected compound"));
        };
        let mut hive = Hive {
            flower: field(&fields, "flower_pos")?
                .map(|t| position(t, "Beehive.flower_pos"))
                .transpose()?,
            ..Default::default()
        };
        field(&fields, "bees")?;
        if let Some((_, tag)) = fields.into_iter().find(|(name, _)| name == "bees") {
            hive.bees = occupants_from_tag(tag)?;
        }
        Ok(hive)
    }
    fn encode(&self, pos: BlockPos) -> Vec<u8> {
        let mut fields = vec![
            ("id".into(), Tag::String("minecraft:beehive".into())),
            ("x".into(), Tag::Int(pos.x)),
            ("y".into(), Tag::Int(pos.y)),
            ("z".into(), Tag::Int(pos.z)),
            ("bees".into(), occupants_tag(&self.bees)),
        ];
        if let Some(p) = self.flower {
            fields.push(("flower_pos".into(), pos_tag(p)));
        }
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("hive NBT");
        bytes
    }
}
fn occupants_from_tag(tag: Tag) -> Result<Vec<Occupant>, CoreError> {
    let Tag::List(list) = tag else {
        return Err(fail("Beehive.bees", "expected compound list"));
    };
    if list.elements.len() > 3 {
        return Err(fail("Beehive.bees", "expected at most three occupants"));
    }
    let mut bees = Vec::with_capacity(list.elements.len());
    for tag in list.elements {
        let Tag::Compound(fields) = tag else {
            return Err(fail("Beehive.bees", "expected compound occupant"));
        };
        let Some(Tag::Compound(data)) = field(&fields, "entity_data")? else {
            return Err(fail("Beehive.bees.entity_data", "expected compound"));
        };
        if field(data, "id")? != Some(&Tag::String("minecraft:bee".into())) {
            return Err(fail(
                "Beehive.bees.entity_data.id",
                "expected minecraft:bee",
            ));
        }
        from_fields(data)?;
        hive_image(data)?;
        if let Some(tag) = field(data, "Health")?
            && !matches!(tag, Tag::Float(value) if value.is_finite() && *value > 0.0)
        {
            return Err(fail(
                "Beehive.bees.entity_data.Health",
                "expected positive finite float",
            ));
        }
        let ticks = integer(&fields, "ticks_in_hive", -1)?;
        let minimum = integer(&fields, "min_ticks_in_hive", -1)?;
        if ticks < 0 || minimum < 0 {
            return Err(fail("Beehive.bees", "missing occupation timers"));
        }
        let (_, Tag::Compound(data)) = fields
            .into_iter()
            .find(|(name, _)| name == "entity_data")
            .expect("validated entity_data")
        else {
            unreachable!("validated compound")
        };
        bees.push(Occupant {
            data,
            ticks,
            minimum,
        });
    }
    Ok(bees)
}

fn item_component(stack: &ItemStack, name: &str) -> Result<Option<Tag>, CoreError> {
    let mut values = stack
        .extended()
        .preserved_components
        .iter()
        .filter(|component| component.name == name);
    let Some(component) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(fail(name, "duplicate component"));
    }
    let mut input = component.value.as_slice();
    let tag =
        mc_nbt::read_network_value(&mut input).map_err(|error| fail(name, &error.to_string()))?;
    if !input.is_empty() {
        return Err(fail(name, "trailing NBT bytes"));
    }
    Ok(Some(tag))
}

/// Loot must use the owner image, not a chunk image whose occupation clock is older.
pub(crate) fn loot_image(state: &TickState, pos: BlockPos) -> Option<Vec<u8>> {
    honey(state, pos)?;
    state
        .hives
        .get(&key(pos))
        .map(|hive| hive.encode(pos))
        .or_else(|| {
            state
                .world
                .chunk(crate::boundary::ChunkPos {
                    x: pos.x.div_euclid(16),
                    z: pos.z.div_euclid(16),
                })?
                .block_entities
                .get(&pos)
                .cloned()
        })
}

/// Read the same owner image that break custody consumes, before its removal.
pub(crate) fn inhabited_bees(state: &TickState, pos: BlockPos) -> Result<Option<usize>, CoreError> {
    if honey(state, pos).is_none() {
        return Ok(None);
    }
    if let Some(hive) = state.hives.get(&key(pos)) {
        return Ok(Some(hive.bees.len()));
    }
    state
        .world
        .chunk(crate::boundary::ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos))
        .map(|bytes| Hive::decode(bytes).map(|hive| hive.bees.len()))
        .transpose()
        .map(|count| Some(count.unwrap_or(0)))
}

/// A broken owner transfers to the first carrying item, or releases its bees.
/// Normalize loot cardinality instead of trusting generated component payloads.
pub(crate) fn break_custody(
    state: &mut TickState,
    pos: BlockPos,
    drops: &mut Vec<ItemStack>,
) -> Result<(), CoreError> {
    if honey(state, pos).is_none() {
        return Ok(());
    }
    let fallback = if state.hives.contains_key(&key(pos)) {
        None
    } else {
        state
            .world
            .chunk(crate::boundary::ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .and_then(|chunk| chunk.block_entities.get(&pos))
            .map(|bytes| Hive::decode(bytes))
            .transpose()?
    };
    let empty = Hive::default();
    let hive = state
        .hives
        .get(&key(pos))
        .or(fallback.as_ref())
        .unwrap_or(&empty);
    // Validate retained images before any item/owner/entity mutation.
    for occupant in &hive.bees {
        from_fields(&occupant.data)?;
        if let Some(image) = hive_image(&occupant.data)? {
            crate::dimensions::recovery::validate_entity(state, 0, &image)?;
        }
    }
    let first = drops.iter().position(|stack| {
        stack.count > 0
            && stack
                .extended()
                .preserved_components
                .iter()
                .any(|component| component.name == "minecraft:bees")
    });
    let mut value = Vec::new();
    if first.is_some() {
        mc_nbt::write_network_value(&mut value, &occupants_tag(&hive.bees))
            .map_err(|error| fail("Beehive.bees", &error.to_string()))?;
    }
    let mut remainder = None;
    for (index, stack) in drops.iter_mut().enumerate() {
        if stack
            .extended()
            .preserved_components
            .iter()
            .any(|component| component.name == "minecraft:bees")
        {
            stack
                .extended_mut()
                .preserved_components
                .retain(|component| component.name != "minecraft:bees");
        }
        if Some(index) == first {
            if stack.count > 1 {
                let mut extra = stack.clone();
                extra.count -= 1;
                remainder = Some(extra);
                stack.count = 1;
            }
            stack.extended_mut().preserved_components.push(
                mc_data::item_stack::PreservedItemComponent {
                    name: "minecraft:bees".into(),
                    value: std::mem::take(&mut value),
                    baseline: None,
                },
            );
        }
    }
    if let Some(stack) = remainder {
        drops.push(stack);
    }
    let hive = state
        .hives
        .remove(&key(pos))
        .or(fallback)
        .unwrap_or_default();
    state.world.set_opaque_block_entity(pos, None);
    state.opaque_changes.push((pos, None));
    if first.is_none() {
        let mut changes = ChangeSet {
            entities_spawned: std::mem::take(&mut state.hostile.spawned),
            ..Default::default()
        };
        for occupant in hive.bees {
            release(state, pos, occupant, None, false, &mut changes);
        }
        state.hostile.spawned = changes.entities_spawned;
    }
    Ok(())
}

fn occupants_tag(bees: &[Occupant]) -> Tag {
    Tag::List(ListTag {
        element_type: 10,
        elements: bees
            .iter()
            .map(|bee| {
                Tag::Compound(vec![
                    ("entity_data".into(), Tag::Compound(bee.data.clone())),
                    ("ticks_in_hive".into(), Tag::Int(bee.ticks)),
                    ("min_ticks_in_hive".into(), Tag::Int(bee.minimum)),
                ])
            })
            .collect(),
    })
}

/// Restore the exact pre-debit stack accepted by merge. Validate before mutation.
pub(crate) fn place_from_item(
    state: &mut TickState,
    intent: &crate::intent::Intent,
) -> Result<(), CoreError> {
    let crate::intent::IntentKind::PlaceBlock {
        pos,
        state: block_id,
    } = intent.kind
    else {
        return Ok(());
    };
    let Some(block) = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(block_id))
    else {
        return Ok(());
    };
    if !matches!(block.block.id.path(), "beehive" | "bee_nest") {
        return Ok(());
    }
    let Some(claim) = intent
        .footprint
        .inv
        .iter()
        .find(|claim| claim.owner == intent.actor)
    else {
        return Ok(());
    };
    let stack = state
        .players
        .get(&claim.owner)
        .and_then(|player| player.inventory.slots.get(usize::from(claim.slot)))
        .ok_or_else(|| fail("Beehive", "missing accepted placement stack"))?;
    let bees = item_component(stack, "minecraft:bees")?
        .map(occupants_from_tag)
        .transpose()?
        .unwrap_or_default();
    let mut placed_id = block_id;
    if let Some(tag) = item_component(stack, "minecraft:block_state")? {
        let Tag::Compound(fields) = tag else {
            return Err(fail("minecraft:block_state", "expected compound"));
        };
        if let Some(tag) = field(&fields, "honey_level")? {
            let Tag::String(level) = tag else {
                return Err(fail(
                    "minecraft:block_state.honey_level",
                    "expected string 0..=5",
                ));
            };
            if !matches!(level.as_str(), "0" | "1" | "2" | "3" | "4" | "5") {
                return Err(fail(
                    "minecraft:block_state.honey_level",
                    "expected string 0..=5",
                ));
            }
            let mut props = block.properties.clone();
            let property = props
                .iter_mut()
                .find(|(name, _)| name == "honey_level")
                .expect("hive honey property");
            property.1 = level.clone();
            placed_id = state
                .registry
                .blocks
                .by_name_and_props(&block.block.id, &props)
                .expect("hive honey state")
                .0;
        }
    }
    let write = state
        .merge_block_writes
        .iter_mut()
        .rev()
        .find(|(p, _)| *p == pos)
        .ok_or_else(|| fail("Beehive", "missing accepted placement write"))?;
    write.1 = placed_id;
    state.hives.insert(
        key(pos),
        Hive {
            bees,
            flower: None,
            pending_placement: true,
        },
    );
    Ok(())
}
pub(crate) fn hydrate_chunk(
    hives: &mut Hives,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) -> Result<(), CoreError> {
    for (p, bytes) in &chunk.block_entities {
        let is_hive = chunk
            .get_block(p.x.rem_euclid(16) as u8, p.y, p.z.rem_euclid(16) as u8)
            .and_then(|id| registry.blocks.by_id(id))
            .is_some_and(|b| matches!(b.block.id.path(), "beehive" | "bee_nest"));
        if is_hive && !hives.contains_key(&key(*p)) {
            hives.insert(key(*p), Hive::decode(bytes)?);
        }
    }
    Ok(())
}
fn persist(state: &mut TickState, pos: BlockPos, hive: &Hive) {
    let bytes = hive.encode(pos);
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
}
pub(crate) fn checkpoint(state: &mut TickState) {
    let hives = std::mem::take(&mut state.hives);
    for (&(x, y, z), hive) in &hives {
        if hive.pending_placement
            && let Some((_, block)) = state
                .merge_block_writes
                .iter()
                .rev()
                .find(|(pos, _)| *pos == BlockPos { x, y, z })
        {
            // Checkpoint can adopt accepted placement before bees::run.
            state.world.set_block(x, y, z, *block);
        }
        state.world.set_opaque_block_entity(
            BlockPos { x, y, z },
            Some(hive.encode(BlockPos { x, y, z })),
        );
    }
    state.hives = hives;
}
pub(crate) fn block(state: &TickState, p: BlockPos) -> Option<&mc_world::BlockState> {
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(state.world.block_at(p.x, p.y, p.z)?))
}
fn honey(state: &TickState, p: BlockPos) -> Option<u8> {
    let b = block(state, p)?;
    if !matches!(b.block.id.path(), "beehive" | "bee_nest") {
        return None;
    }
    b.properties
        .iter()
        .find(|(k, _)| k == "honey_level")?
        .1
        .parse()
        .ok()
}
pub(crate) fn property(state: &TickState, p: BlockPos, name: &str, value: &str) -> Option<u32> {
    let b = block(state, p)?;
    let mut properties = b.properties.clone();
    properties.iter_mut().find(|(k, _)| k == name)?.1 = value.to_owned();
    state
        .registry
        .blocks
        .by_name_and_props(&b.block.id, &properties)
        .map(|id| id.0)
}
pub(crate) fn centre(p: BlockPos) -> [f64; 3] {
    [
        f64::from(p.x) + 0.5,
        f64::from(p.y) + 0.5,
        f64::from(p.z) + 0.5,
    ]
}
pub(crate) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}
pub(crate) fn fly(state: &mut TickState, slot: EntitySlot, target: [f64; 3]) {
    let p = state.entities.pos(slot);
    let length = distance(p, target).sqrt();
    if length > 0.01 {
        let next = std::array::from_fn(|i| p[i] + (target[i] - p[i]) * (0.15 / length).min(1.0));
        let cell = BlockPos {
            x: next[0].floor() as i32,
            y: next[1].floor() as i32,
            z: next[2].floor() as i32,
        };
        if block(state, cell)
            .is_some_and(|b| state.registry.collision_boxes[b.id.0 as usize].is_empty())
        {
            state.entities.set_pos(slot, next);
        }
    }
    state.entities.set_vel(slot, [0.0; 3]);
}
fn nearby(
    state: &TickState,
    p: [f64; 3],
    accept: impl Fn(&mc_world::BlockState) -> bool,
) -> Option<BlockPos> {
    let origin = BlockPos {
        x: p[0].floor() as i32,
        y: p[1].floor() as i32,
        z: p[2].floor() as i32,
    };
    let mut result = None;
    let mut best = f64::INFINITY;
    for x in origin.x - 8..=origin.x + 8 {
        for y in origin.y - 4..=origin.y + 4 {
            for z in origin.z - 8..=origin.z + 8 {
                let cell = BlockPos { x, y, z };
                let d = distance(p, centre(cell));
                if d < best && block(state, cell).is_some_and(&accept) {
                    best = d;
                    result = Some(cell);
                }
            }
        }
    }
    result
}
fn flower(b: &mc_world::BlockState) -> bool {
    matches!(
        b.block.id.path(),
        "dandelion"
            | "poppy"
            | "blue_orchid"
            | "allium"
            | "azure_bluet"
            | "red_tulip"
            | "orange_tulip"
            | "white_tulip"
            | "pink_tulip"
            | "oxeye_daisy"
            | "cornflower"
            | "lily_of_the_valley"
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "torchflower"
            | "pitcher_plant"
            | "flowering_azalea"
            | "flowering_azalea_leaves"
            | "cherry_leaves"
            | "pink_petals"
            | "wildflowers"
            | "cactus_flower"
    )
}
pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot, type_id: u32) {
    if state
        .registry
        .blueprint(type_id)
        .is_some_and(|b| b.name == "minecraft:bee")
    {
        debug_assert!(
            matches!(
                state.entities.body(slot).kind,
                crate::entities::EntityKind::Mob { type_id: kind }
                if kind == type_id
            ),
            "bee component implies the bee mob lane"
        );
        state.entities.row_mut(slot).bee.get_or_insert_with(|| {
            Box::new(BeeState {
                anger_end: -1,
                ..Default::default()
            })
        });
        appearance(state, slot);
    }
}
pub(crate) fn appearance(state: &mut TickState, slot: EntitySlot) {
    if let Some(bee) = state.entities.row(slot).bee.as_ref() {
        let flags = (i8::from(bee.stung) * 4)
            | (i8::from(bee.nectar) * 8)
            | (i8::from(bee.anger_end > state.tick as i64) * 2);
        let anger = bee.anger_end;
        crate::special_mobs::metadata(state, slot, 18, EntityDataValue::Byte(flags));
        crate::special_mobs::metadata(state, slot, 19, EntityDataValue::VarLong(anger));
    }
}
fn release(
    state: &mut TickState,
    pos: BlockPos,
    occupant: Occupant,
    angry: Option<uuid::Uuid>,
    delivered: bool,
    changes: &mut ChangeSet,
) {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:bee").unwrap())
        .expect("bee type");
    let exit = exit_position(state, pos);
    let image = hive_image(&occupant.data).expect("validated hive image");
    let native = image.is_some();
    let id = if let Some(mut image) = image {
        let id = state.entities.alloc_id();
        // Vanilla strips occupant UUIDs. The old body may already have a durable
        // checkpoint tombstone; retain its state, never resurrect its identity.
        image.uuid = state.entity_uuid(id).to_string();
        image.pos = exit;
        image.vel = [0.0; 3];
        image.state.entity_life.child_ticks = image
            .state
            .entity_life
            .child_ticks
            .saturating_sub(occupant.ticks as u32);
        image.breed_cooldown = image.breed_cooldown.saturating_sub(occupant.ticks as u32);
        image.state.breed_cooldown = image.breed_cooldown;
        image.state.love = image.state.love.saturating_sub(occupant.ticks as u32);
        crate::core::restore_entity(state, id.0, &image).expect("validated hive entity recovery");
        id
    } else {
        state.spawn_mob(type_id, exit, 10.0)
    };
    let slot = state.entities.slot_of(id).expect("bee spawned");
    let mut bee = from_fields(&occupant.data).expect("validated hive bee");
    bee.hive = Some(pos);
    if delivered {
        bee.nectar = false;
    }
    bee.crops = 0;
    bee.pollination = 0;
    bee.ticks_since_pollination = 0;
    bee.cannot_enter = 200;
    if let Some(target) = angry {
        bee.angry_at = Some(target);
        bee.anger_end = state.tick.saturating_add(600) as i64;
    }
    let (body, row) = state.entities.parts_mut(slot);
    if let Ok(Some(Tag::Float(health))) = field(&occupant.data, "Health") {
        row.health = *health;
    }
    if !native && let Ok(Some(Tag::Int(age))) = field(&occupant.data, "Age") {
        row.entity_life.child_ticks = age
            .unsigned_abs()
            .min(24000)
            .saturating_sub(occupant.ticks as u32)
            * u32::from(*age < 0);
        row.breed_cooldown = ((*age).max(0) as u32).saturating_sub(occupant.ticks as u32);
    }
    row.bee = Some(Box::new(bee));
    row.persistent = true;
    crate::entity_life::restore_appearance(body, row);
    appearance(state, slot);
    changes.entities_spawned.push(id);
}
fn hive_image(fields: &[(String, Tag)]) -> Result<Option<crate::journal::ReplayEntity>, CoreError> {
    let Some(tag) = field(fields, "solaris:HiveEntity")? else {
        return Ok(None);
    };
    let Tag::String(json) = tag else {
        return Err(fail(
            "Beehive.solaris:HiveEntity",
            "expected entity image string",
        ));
    };
    let image: crate::journal::ReplayEntity = serde_json::from_str(json)
        .map_err(|e| fail("Beehive.solaris:HiveEntity", &e.to_string()))?;
    let bee_type = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(image.type_id)
        .is_some_and(|c| c.name == "minecraft:bee");
    if !bee_type
        || image.state.bee.is_none()
        || !image.alive
        || image.item.is_some()
        || image.projectile.is_some()
        || !image.health.is_finite()
        || image.health <= 0.0
        || uuid::Uuid::parse_str(&image.uuid).is_err()
    {
        return Err(fail(
            "Beehive.solaris:HiveEntity",
            "expected live bee image",
        ));
    }
    let bee = image.state.bee.as_ref().expect("validated variant");
    let vanilla = from_fields(fields)?;
    if bee.nectar != vanilla.nectar || bee.stung != vanilla.stung || bee.flower != vanilla.flower {
        return Err(fail(
            "Beehive.solaris:HiveEntity",
            "native/vanilla bee data mismatch",
        ));
    }
    Ok(Some(image))
}
fn exit_cell(state: &TickState, pos: BlockPos) -> BlockPos {
    let facing = block(state, pos)
        .and_then(|b| b.properties.iter().find(|(k, _)| k == "facing"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("south");
    let (dx, dz) = match facing {
        "north" => (0, -1),
        "west" => (-1, 0),
        "east" => (1, 0),
        _ => (0, 1),
    };
    BlockPos {
        x: pos.x + dx,
        y: pos.y,
        z: pos.z + dz,
    }
}
fn exit_position(state: &TickState, pos: BlockPos) -> [f64; 3] {
    centre(exit_cell(state, pos))
}

fn grow(
    state: &mut TickState,
    slot: EntitySlot,
    bee: &mut BeeState,
    writes: &mut Vec<(BlockPos, u32)>,
) {
    if bee.crops >= 10 || !state.tick.is_multiple_of(20) {
        return;
    }
    let p = state.entities.pos(slot);
    for dy in 1..=2 {
        let cell = BlockPos {
            x: p[0].floor() as i32,
            y: p[1].floor() as i32 - dy,
            z: p[2].floor() as i32,
        };
        let Some(b) = block(state, cell) else {
            continue;
        };
        if !matches!(
            b.block.id.path(),
            "wheat" | "carrots" | "potatoes" | "beetroots" | "melon_stem" | "pumpkin_stem"
        ) {
            continue;
        }
        let Some(age) = b
            .properties
            .iter()
            .find(|(k, _)| k == "age")
            .and_then(|(_, v)| v.parse::<u8>().ok())
        else {
            continue;
        };
        let maximum = if b.block.id.path() == "beetroots" {
            3
        } else {
            7
        };
        if age < maximum
            && let Some(next) = property(state, cell, "age", &(age + 1).to_string())
        {
            state.world.set_block(cell.x, cell.y, cell.z, next);
            writes.push((cell, next));
            bee.crops += 1;
        }
    }
}
pub(crate) fn run(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    changes: &mut ChangeSet,
) {
    let mut hives = std::mem::take(&mut state.hives);
    hives.retain(|&(x, y, z), hive| {
        let p = BlockPos { x, y, z };
        if !state.world.is_active(x.div_euclid(16), z.div_euclid(16)) {
            return true;
        }
        let level = honey(state, p);
        if level.is_none() {
            for occupant in hive.bees.drain(..) {
                release(state, p, occupant, None, false, changes);
            }
            state.world.set_opaque_block_entity(p, None);
            state.opaque_changes.push((p, None));
            return false;
        }
        if hive.pending_placement {
            hive.pending_placement = false;
            persist(state, p, hive);
        }
        let mut exited = false;
        let mut index = 0;
        while index < hive.bees.len() {
            hive.bees[index].ticks = hive.bees[index].ticks.saturating_add(1);
            let front = exit_cell(state, p);
            if hive.bees[index].ticks > hive.bees[index].minimum
                && state.day_time % 24000 < 12000
                && state.weather.0 == crate::commands::Weather::Clear
                && block(state, front)
                    .is_some_and(|b| state.registry.collision_boxes[b.id.0 as usize].is_empty())
            {
                let occupant = hive.bees.remove(index);
                if from_fields(&occupant.data).expect("validated bee").nectar {
                    let level = honey(state, p).unwrap();
                    if level < 5
                        && let Some(next) =
                            property(state, p, "honey_level", &(level + 1).to_string())
                    {
                        state.world.set_block(x, y, z, next);
                        writes.push((p, next));
                    }
                }
                release(state, p, occupant, None, true, changes);
                exited = true;
            } else {
                index += 1;
            }
        }
        if exited {
            persist(state, p, hive);
        }
        true
    });
    let ids: Vec<_> = BEE_TYPE
        .iter()
        .flat_map(|&type_id| state.entities.mob_slots(type_id))
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            (body.alive && row.bee.is_some()).then_some(body.id)
        })
        .collect();
    for id in ids {
        let Some(slot) = state.entities.slot_of(id) else {
            continue;
        };
        let p = state.entities.pos(slot);
        if !state.world.is_active(
            (p[0].floor() as i32).div_euclid(16),
            (p[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let mut boxed = state.entities.row_mut(slot).bee.take().unwrap();
        let bee = &mut *boxed;
        bee.cannot_enter = bee.cannot_enter.saturating_sub(1);
        if !bee.nectar {
            bee.ticks_since_pollination = bee
                .ticks_since_pollination
                .saturating_add(1)
                .min(i32::MAX as u32);
        }
        if bee.stung {
            bee.sting_ticks = bee.sting_ticks.saturating_add(1);
            if bee.sting_ticks >= 1200 {
                state.entities.row_mut(slot).bee = Some(boxed);
                state.entities.row_mut(slot).health = 0.0;
                state.entities.body_mut(slot).alive = false;
                continue;
            }
        } else if bee.anger_end > state.tick as i64 {
            if let Some(target) = bee
                .angry_at
                .and_then(|uuid| state.entities.slot_of_uuid(uuid))
            {
                let target_pos = state.entities.pos(target);
                fly(state, slot, target_pos);
                if distance(p, target_pos) < 2.0 {
                    state.merge_damage.push((
                        target,
                        crate::tick::DamagePiece {
                            physical: None,
                            custom_armor: false,
                            amount: 2.0,
                            kind: crate::survival::DamageKind::Melee,
                            source: Some(crate::intent::DamageSource::Melee {
                                attacker: id,
                                position: p,
                                yaw: 0.0,
                                bonus_knockback: 0.0,
                                sweep: false,
                                fire_ticks: 0,
                                bane_ticks: 0,
                            }),
                        },
                    ));
                }
            }
        } else {
            bee.angry_at = None;
            if bee.hive.is_some_and(|p| honey(state, p).is_none()) {
                bee.hive = None;
            }
            if bee
                .flower
                .is_some_and(|p| !block(state, p).is_some_and(flower))
            {
                bee.flower = None;
                bee.pollination = 0;
            }
            if state.tick.is_multiple_of(20) && state.tick >= bee.search_cooldown_until {
                if bee.hive.is_none() {
                    bee.hive = nearby(state, p, |b| {
                        matches!(b.block.id.path(), "beehive" | "bee_nest")
                    });
                }
                if bee.flower.is_none() {
                    bee.flower = nearby(state, p, flower);
                }
                if bee.hive.is_none() || bee.flower.is_none() {
                    bee.search_cooldown_until = state.tick.saturating_add(200);
                }
            }
            if !bee.nectar {
                if let Some(pos) = bee.flower {
                    let target = centre(pos);
                    fly(state, slot, [target[0], target[1] + 0.4, target[2]]);
                    if distance(p, target) < 2.0 {
                        if bee.pollination == 0 {
                            bee.ticks_since_pollination = 0;
                        }
                        bee.pollination = bee.pollination.saturating_add(1);
                        if bee.pollination >= 400 {
                            bee.nectar = true;
                            bee.ticks_since_pollination = 0;
                            bee.pollination = 0;
                            bee.crops = 0;
                        }
                    }
                }
            } else {
                grow(state, slot, bee, writes);
                if let Some(pos) = bee.hive {
                    fly(state, slot, centre(pos));
                }
            }
        }
        state.entities.row_mut(slot).bee = Some(boxed);
        appearance(state, slot);
    }
    state.hives = hives;
}
/// Transfer only after damage/pickup merge has consumed every dense-slot reference.
pub(crate) fn commit(state: &mut TickState, changes: &mut ChangeSet) {
    let ids: Vec<_> = BEE_TYPE
        .iter()
        .flat_map(|&type_id| state.entities.mob_slots(type_id))
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            // The lane retains dead rows, as the old scan did.
            row.bee.is_some().then_some(body.id)
        })
        .collect();
    let mut hives = std::mem::take(&mut state.hives);
    for id in ids {
        let Some(slot) = state.entities.slot_of_any(id) else {
            continue;
        };
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        let bee = row.bee.as_ref().expect("bee owner");
        if bee.stung && bee.sting_ticks >= 1200 {
            state.entities.remove(slot);
            changes.entities_removed.push(id);
            continue;
        }
        if !body.alive
            || !bee.nectar
            || bee.stung
            || bee.anger_end > state.tick as i64
            || bee.cannot_enter > 0
        {
            continue;
        }
        let Some(pos) = bee.hive.filter(|p| honey(state, *p).is_some()) else {
            continue;
        };
        if distance(state.entities.pos(slot), centre(pos)) >= 2.0 {
            continue;
        }
        let hive = hives.entry(key(pos)).or_default();
        if hive.bees.len() >= 3 {
            continue;
        }
        hive.flower = bee.flower;
        let image = crate::journal::ReplayEntity {
            uuid: row.uuid.to_string(),
            type_id: crate::mounts::type_id(body).expect("bee mob"),
            pos: state.entities.pos(slot),
            vel: state.entities.vel(slot),
            yaw: state.entities.rot(slot)[0],
            pitch: state.entities.rot(slot)[1],
            ground: state.entities.ground(slot),
            health: row.health,
            alive: true,
            age: row.age,
            breed_cooldown: row.breed_cooldown,
            pickup_delay: body.pickup_delay,
            item: None,
            appearance: crate::journal::EntityAppearanceImage::from(row.appearance.as_ref()),
            active_effects: row.active_effects.clone(),
            villager: None,
            state: crate::journal::EntityStateImage::from_row(body, row),
            projectile: None,
            tick: state.tick,
        };
        let mut data = image.state.to_fields(&state.registry.blocks);
        data.push(("id".into(), Tag::String("minecraft:bee".into())));
        data.push(("Health".into(), Tag::Float(row.health)));
        data.push((
            "Age".into(),
            Tag::Int(if row.entity_life.child_ticks > 0 {
                -(row.entity_life.child_ticks as i32)
            } else {
                row.breed_cooldown as i32
            }),
        ));
        // BeehiveBlockEntity.Occupant.of strips these vanilla transient fields.
        data.retain(|(key, _)| {
            !matches!(
                key.as_str(),
                "Air"
                    | "drop_chances"
                    | "equipment"
                    | "Brain"
                    | "CanPickUpLoot"
                    | "DeathTime"
                    | "fall_distance"
                    | "FallFlying"
                    | "Fire"
                    | "HurtByTimestamp"
                    | "HurtTime"
                    | "LeftHanded"
                    | "Motion"
                    | "NoGravity"
                    | "OnGround"
                    | "PortalCooldown"
                    | "Pos"
                    | "Rotation"
                    | "sleeping_pos"
                    | "CannotEnterHiveTicks"
                    | "TicksSincePollination"
                    | "CropsGrownSincePollination"
                    | "hive_pos"
                    | "Passengers"
                    | "leash"
                    | "UUID"
            )
        });
        data.push((
            "solaris:HiveEntity".into(),
            Tag::String(serde_json::to_string(&image).expect("hive entity image")),
        ));
        hive.bees.push(Occupant {
            data,
            ticks: 0,
            minimum: 2400,
        });
        persist(state, pos, hive);
        state.entities.remove(slot);
        changes.entities_removed.push(id);
    }
    state.hives = hives;
}

pub(crate) fn smoke(state: &TickState, p: BlockPos) -> bool {
    for dy in 1..=5 {
        let cell = BlockPos {
            x: p.x,
            y: p.y - dy,
            z: p.z,
        };
        let Some(b) = block(state, cell) else {
            return false;
        };
        if matches!(b.block.id.path(), "campfire" | "soul_campfire") {
            return b.properties.iter().any(|(k, v)| k == "lit" && v == "true");
        }
        if !state.registry.collision_boxes[b.id.0 as usize].is_empty() {
            return false;
        }
    }
    false
}
fn anger(state: &mut TickState, p: [f64; 3], target: uuid::Uuid) {
    let until = state.tick.saturating_add(600) as i64;
    for slot in 0..state.entities.len() {
        let slot = EntitySlot(slot as u32);
        if distance(state.entities.pos(slot), p) <= 16.0 * 16.0
            && let Some(bee) = state.entities.row_mut(slot).bee.as_deref_mut()
            && !bee.stung
        {
            bee.angry_at = Some(target);
            bee.anger_end = until;
        }
    }
}
pub(crate) fn accepted_hit(
    state: &mut TickState,
    victim: EntitySlot,
    source: Option<crate::intent::DamageSource>,
) {
    let attacker = match source {
        Some(
            crate::intent::DamageSource::Melee { attacker, .. }
            | crate::intent::DamageSource::Sweep { attacker, .. }
            | crate::intent::DamageSource::Projectile {
                owner: attacker, ..
            },
        ) => attacker,
        _ => return,
    };
    let Some(slot) = state.entities.slot_of(attacker) else {
        return;
    };
    if state.entities.row(victim).bee.is_some() {
        anger(
            state,
            state.entities.pos(victim),
            state.entities.row(slot).uuid,
        );
    }
    let is_sting = matches!(source, Some(crate::intent::DamageSource::Melee { .. }))
        && state
            .entities
            .row(slot)
            .bee
            .as_ref()
            .is_some_and(|b| !b.stung);
    if is_sting {
        let bee = state.entities.row_mut(slot).bee.as_mut().unwrap();
        bee.stung = true;
        bee.sting_ticks = 0;
        bee.anger_end = -1;
        bee.angry_at = None;
        crate::projectiles::embedded_projectiles::sting(state, victim);
        let duration = match state.difficulty {
            crate::survival::Difficulty::Hard => 360,
            crate::survival::Difficulty::Normal => 200,
            _ => 0,
        };
        if duration > 0 {
            crate::projectiles::payloads::give_effect(
                state,
                state.entities.body(victim).id,
                crate::effects::ActiveEffect::new(crate::effects::EffectId::Poison, duration, 0),
            );
        }
        appearance(state, slot);
    }
}
pub(crate) fn use_on(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    pos: BlockPos,
    held: &ItemStack,
    changes: &mut ChangeSet,
) -> bool {
    let Some(level) = honey(state, pos) else {
        return false;
    };
    let Some(name) = state
        .registry
        .items
        .name_of(held.item_id)
        .filter(|_| !held.is_empty())
    else {
        return false;
    };
    let bottle = name.path() == "glass_bottle";
    let shears = name.path() == "shears";
    if !bottle && !shears {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    if level < 5 || player.spectator() {
        return true;
    }
    let connection = player.connection;
    let uuid = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    let product = state
        .registry
        .items
        .id_of(
            &Identifier::parse(if bottle {
                "minecraft:honey_bottle"
            } else {
                "minecraft:honeycomb"
            })
            .unwrap(),
        )
        .unwrap();
    let rules = state.registry.item_rules();
    let product = ItemStack::new(product, if bottle { 1 } else { 3 });
    let effects = if crate::plugin_inventories::has_reservations_for_player(state, actor) {
        let plan = state.players[&actor].plan_inventory_transition(&[], |player, _| {
            player.harvest_held(&rules, hand, product, bottle)
        });
        if crate::plugin_inventories::below_player_floors(state, actor, plan.player_slots()) {
            return true;
        }
        state
            .players
            .get_mut(&actor)
            .unwrap()
            .apply_inventory_transition(plan, &mut [])
    } else {
        state
            .players
            .get_mut(&actor)
            .unwrap()
            .harvest_held(&rules, hand, product, bottle)
    };
    let next = property(state, pos, "honey_level", "0").unwrap();
    state.world.set_block(pos.x, pos.y, pos.z, next);
    state.merge_block_writes.push((pos, next));
    for change in effects.changes {
        state.push_player(connection, change);
    }
    for stack in effects.dropped {
        changes
            .entities_spawned
            .push(crate::tick::spawn_item_entity(state, centre(pos), stack));
    }
    if !smoke(state, pos) {
        anger(state, centre(pos), uuid);
        let mut hives = std::mem::take(&mut state.hives);
        if let Some(hive) = hives.get_mut(&key(pos)) {
            for occupant in hive.bees.drain(..) {
                release(state, pos, occupant, Some(uuid), false, changes);
            }
            persist(state, pos, hive);
        }
        state.hives = hives;
    }
    crate::advancements::accepted_block(
        state,
        actor,
        "minecraft:item_used_on_block",
        pos,
        Some(next),
        Some(held.clone()),
    );
    true
}
#[cfg(test)]
#[path = "bees_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "advancement_block_use_tests.rs"]
mod advancement_block_use_tests;
