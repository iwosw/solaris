//! Native inventory custody and durable reservation accounting for plugin operations.
use std::collections::{BTreeMap, BTreeSet};

use mc_data::{Identifier, ItemStack};
use mc_script::{ScriptInventoryEndpoint as Endpoint, ScriptOperationFailure as Failure, *};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::boundary::{BlockPos, ConnectionId};
use crate::tick::TickState;

#[path = "settlements/inventory.rs"]
pub(crate) mod native;

const OWNER: &str = "@native-inventories";
const KEY: &str = "metadata";
const SCHEMA: u32 = 1;
const MAX_PROTECTED_RESERVATIONS: usize = 4096;
const MAX_TERMINAL_RESERVATIONS: usize = 256;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    schema: u32,
    revision: u64,
    protected_count: usize,
    retired_next: usize,
    retired_count: usize,
}
impl Default for Metadata {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            revision: 0,
            protected_count: 0,
            retired_next: 0,
            retired_count: 0,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageVersion {
    hash: String,
    revision: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reservation {
    pub(crate) owner: String,
    pub(crate) revision: u64,
    player_uuid: Option<uuid::Uuid>,
    source_cells: Vec<[i32; 3]>,
    pub(crate) snapshot: ScriptInventoryReservationSnapshot,
}

#[derive(Clone)]
pub(crate) struct InventoryImage {
    endpoint: Endpoint,
    pub(crate) slots: BTreeMap<u8, ItemStack>,
    pub(crate) fence: ScriptInventoryFence,
}

fn metadata(state: &TickState) -> Result<Metadata, Failure> {
    let value = match state.plugins.get_domain(OWNER, KEY) {
        Some(value) => Metadata::deserialize(value).map_err(|_| Failure::RuntimeUnavailable)?,
        None if state.plugins.domain_entries(OWNER).next().is_none() => Metadata::default(),
        None => return Err(Failure::RuntimeUnavailable),
    };
    if value.schema != SCHEMA
        || value.revision > MAX_SCRIPT_WORLD_TIME
        || value.protected_count > MAX_PROTECTED_RESERVATIONS
        || value.retired_next >= MAX_TERMINAL_RESERVATIONS
        || value.retired_count > MAX_TERMINAL_RESERVATIONS
        || (value.retired_count < MAX_TERMINAL_RESERVATIONS
            && value.retired_next != value.retired_count)
    {
        return Err(Failure::RuntimeUnavailable);
    }
    Ok(value)
}
fn save_metadata(state: &mut TickState, value: &Metadata) {
    state.plugins.set_domain(
        OWNER,
        KEY,
        serde_json::to_value(value).expect("inventory metadata serialization"),
    );
}
fn image_version(state: &TickState, key: &str) -> Result<Option<ImageVersion>, Failure> {
    state
        .plugins
        .get_domain(OWNER, &format!("image/{key}"))
        .map(|value| ImageVersion::deserialize(value).map_err(|_| Failure::RuntimeUnavailable))
        .transpose()
}
fn stored_reservation(state: &TickState, reference: &str) -> Result<Reservation, Failure> {
    let value = state
        .plugins
        .get_domain(OWNER, &format!("reservation/{reference}"))
        .ok_or(Failure::NotFound)?;
    Reservation::deserialize(value).map_err(|_| Failure::RuntimeUnavailable)
}
pub(crate) fn retains_reservation(state: &TickState, reference: &str) -> bool {
    state
        .plugins
        .get_domain(OWNER, &format!("reservation/{reference}"))
        .is_some()
}
fn active(value: &Reservation) -> bool {
    !value.snapshot.released && value.snapshot.quantities.iter().any(|q| q.remaining > 0)
}
fn protected(value: &Reservation) -> bool {
    // A fully consumed structure hold remains addressable for later empty-material stages.
    // Such pins share the admission cap; only unbound terminal records enter the FIFO.
    active(value) || value.snapshot.bound_to.is_some()
}
fn cell_key(dimension: &str, cell: [i32; 3]) -> String {
    format!("container:{dimension}:{}:{}:{}", cell[0], cell[1], cell[2])
}
fn source_keys(value: &Reservation) -> Vec<String> {
    match &value.snapshot.endpoint {
        Endpoint::PlayerInventory { .. } => value
            .player_uuid
            .map(|uuid| vec![format!("player:{uuid}")])
            .unwrap_or_default(),
        Endpoint::Container { dimension, .. } => {
            let dimension = crate::dimensions::Dimension::parse(dimension)
                .map_or(dimension.as_str(), |dimension| dimension.name());
            value
                .source_cells
                .iter()
                .map(|cell| cell_key(dimension, *cell))
                .collect()
        }
        _ => Vec::new(),
    }
}
fn index_keys(value: &Reservation) -> Vec<String> {
    let reference = &value.snapshot.reservation_ref;
    let mut keys = Vec::new();
    if active(value) {
        keys.extend(
            source_keys(value)
                .into_iter()
                .map(|source| format!("source/{source}/{reference}")),
        );
    }
    if protected(value) && !value.snapshot.released {
        keys.extend(
            source_keys(value)
                .into_iter()
                .map(|source| format!("custody/{source}/{reference}")),
        );
        keys.push(format!(
            "plan/{}/{}/{reference}",
            value.owner, value.snapshot.resource_plan_hash
        ));
    }
    keys
}
fn endpoint_source_keys(state: &TickState, endpoint: &Endpoint) -> Result<Vec<String>, Failure> {
    match endpoint {
        Endpoint::PlayerInventory { player_id } => {
            let uuid = player_world(state, ConnectionId(*player_id))
                .and_then(|target| target.connections.uuid_of(ConnectionId(*player_id)));
            Ok(uuid
                .map(|uuid| vec![format!("player:{uuid}")])
                .unwrap_or_default())
        }
        Endpoint::Container {
            dimension,
            position,
        } => {
            let target = world(state, dimension)?;
            let pos = BlockPos {
                x: position.x,
                y: position.y,
                z: position.z,
            };
            let mut keys = vec![cell_key(target.dimension.name(), [pos.x, pos.y, pos.z])];
            if let Some(partner) = custody_partner(target, pos) {
                keys.push(cell_key(
                    target.dimension.name(),
                    [partner.x, partner.y, partner.z],
                ));
            }
            Ok(keys)
        }
        _ => Ok(Vec::new()),
    }
}
fn endpoint_reservations(
    state: &TickState,
    endpoint: &Endpoint,
) -> Result<Vec<Reservation>, Failure> {
    indexed_reservations(state, endpoint, "source")
}
fn indexed_reservations(
    state: &TickState,
    endpoint: &Endpoint,
    kind: &str,
) -> Result<Vec<Reservation>, Failure> {
    metadata(state)?;
    if state.plugins.domain_entries(OWNER).next().is_none() {
        return Ok(Vec::new());
    }
    let mut references = BTreeSet::new();
    for source in endpoint_source_keys(state, endpoint)? {
        let prefix = format!("{kind}/{source}/");
        for (key, value) in state.plugins.domain_prefix(OWNER, &prefix) {
            if value != &serde_json::Value::Bool(true) {
                return Err(Failure::RuntimeUnavailable);
            }
            references.insert(&key[prefix.len()..]);
        }
    }
    references
        .into_iter()
        .map(|reference| {
            let value = stored_reservation(state, reference)?;
            let current = if kind == "source" {
                active(&value)
            } else {
                protected(&value)
            };
            if !current
                || value.snapshot.released
                || value.snapshot.reservation_ref != reference
                || !reservation_source_matches(state, &value, endpoint)
            {
                return Err(Failure::RuntimeUnavailable);
            }
            Ok(value)
        })
        .collect()
}
fn endpoint_key(state: &TickState, endpoint: &Endpoint) -> String {
    match endpoint {
        Endpoint::PlayerInventory { player_id } => player_world(state, ConnectionId(*player_id))
            .and_then(|target| target.connections.uuid_of(ConnectionId(*player_id)))
            .map(|uuid| format!("player:{uuid}"))
            .unwrap_or_else(|| format!("unloaded-player:{player_id}")),
        Endpoint::Container {
            dimension,
            position,
        } => {
            let cells =
                world(state, dimension).and_then(|target| container_cells(target, position));
            if let Ok(cells) = cells {
                let cells: Vec<_> = cells.iter().map(|p| (p.x, p.y, p.z)).collect();
                return format!("container:{dimension}:{}", hash(&cells));
            }
            serde_json::to_string(endpoint).expect("inventory endpoint serialization")
        }
        _ => serde_json::to_string(endpoint).expect("inventory endpoint serialization"),
    }
}
fn player_world(state: &TickState, connection: ConnectionId) -> Option<&TickState> {
    std::iter::once(state)
        .chain(state.other_dimensions.values().map(Box::as_ref))
        .find(|target| target.connections.entity_of(connection).is_some())
}
fn same_endpoint(state: &TickState, left: &Endpoint, right: &Endpoint) -> bool {
    endpoint_key(state, left) == endpoint_key(state, right)
}
fn image_keys(state: &TickState, endpoint: &Endpoint) -> Vec<String> {
    if let Endpoint::Container {
        dimension,
        position,
    } = endpoint
        && let Ok(cells) =
            world(state, dimension).and_then(|target| container_cells(target, position))
    {
        return cells
            .iter()
            .map(|cell| format!("container:{dimension}:{}:{}:{}", cell.x, cell.y, cell.z))
            .collect();
    }
    vec![endpoint_key(state, endpoint)]
}
fn remember(
    state: &mut TickState,
    endpoint: &Endpoint,
    hash: &str,
    revision: u64,
) -> Result<(), Failure> {
    let metadata = metadata(state)?;
    if state.plugins.get_domain(OWNER, KEY).is_none() {
        save_metadata(state, &metadata);
    }
    for key in image_keys(state, endpoint) {
        let value = ImageVersion {
            hash: hash.to_owned(),
            revision,
        };
        if image_version(state, &key)?
            .is_some_and(|before| before.hash == value.hash && before.revision == value.revision)
        {
            continue;
        }
        state.plugins.set_domain(
            OWNER,
            &format!("image/{key}"),
            serde_json::to_value(value).expect("inventory image serialization"),
        );
    }
    Ok(())
}
pub(crate) fn hash<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("canonical inventory serialization");
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn resource_plan_hash(plan: &ScriptInventoryResourcePlan) -> String {
    hash(plan)
}
fn next(value: u64) -> Result<u64, Failure> {
    value
        .checked_add(1)
        .filter(|v| *v <= MAX_SCRIPT_WORLD_TIME)
        .ok_or(Failure::Capacity)
}

pub(crate) fn world<'a>(state: &'a TickState, dimension: &str) -> Result<&'a TickState, Failure> {
    let dimension = crate::dimensions::Dimension::parse(dimension).ok_or(Failure::NotFound)?;
    if state.dimension == dimension {
        Ok(state)
    } else {
        state
            .other_dimensions
            .get(&dimension)
            .map(Box::as_ref)
            .ok_or(Failure::Unloaded)
    }
}
pub(crate) fn world_mut<'a>(
    state: &'a mut TickState,
    dimension: &str,
) -> Result<&'a mut TickState, Failure> {
    let dimension = crate::dimensions::Dimension::parse(dimension).ok_or(Failure::NotFound)?;
    if state.dimension == dimension {
        Ok(state)
    } else {
        state
            .other_dimensions
            .get_mut(&dimension)
            .map(Box::as_mut)
            .ok_or(Failure::Unloaded)
    }
}
fn player_index(slot: u8) -> Option<usize> {
    match slot {
        9..=35 => Some(usize::from(slot)),
        36..=44 => Some(usize::from(slot - 36)),
        _ => None,
    }
}
pub(crate) fn container_cells(
    state: &TickState,
    position: &ScriptBlockPosition,
) -> Result<Vec<BlockPos>, Failure> {
    let pos = BlockPos {
        x: position.x,
        y: position.y,
        z: position.z,
    };
    let id = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .ok_or(Failure::Unloaded)?;
    let block = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .ok_or(Failure::InvalidRequest)?;
    if !container_name(block.block.id.as_str()) {
        return Err(Failure::NotFound);
    }
    let cells = crate::chests::storage_cells(&state.registry.blocks, pos, &|p| {
        state.world.block_at(p.x, p.y, p.z)
    });
    for cell in &cells {
        if state.world.block_at(cell.x, cell.y, cell.z).is_none() {
            return Err(Failure::Unloaded);
        }
        if !state.block_entities.contains_key(&(cell.x, cell.y, cell.z)) {
            return Err(Failure::NotFound);
        }
    }
    Ok(cells)
}

// Admission also protects a declared half whose partner is unloaded. Unlike
// inventory queries, it must not interpret unavailable custody as free stock.
fn custody_partner(state: &TickState, pos: BlockPos) -> Option<BlockPos> {
    if let Some((_, partner)) =
        crate::chests::storage_partner(&state.registry.blocks, pos, &|cell| {
            state.world.block_at(cell.x, cell.y, cell.z)
        })
    {
        return Some(partner);
    }
    let block = state.registry.blocks.by_id(mc_world::BlockStateId(
        state.world.block_at(pos.x, pos.y, pos.z)?,
    ))?;
    if !block.block.id.as_str().ends_with("chest") {
        return None;
    }
    let property = |name: &str| {
        block
            .properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let facing =
        crate::placement::Face::from_name(property("facing")?).filter(|face| face.horizontal())?;
    let direction = match property("type")? {
        "left" => facing.clockwise(),
        "right" => facing.counter_clockwise(),
        _ => return None,
    };
    let [dx, dy, dz] = direction.offset();
    let partner = BlockPos {
        x: pos.x.checked_add(dx)?,
        y: pos.y.checked_add(dy)?,
        z: pos.z.checked_add(dz)?,
    };
    state
        .world
        .block_at(partner.x, partner.y, partner.z)
        .is_none()
        .then_some(partner)
}
pub(crate) fn container_name(name: &str) -> bool {
    name == "minecraft:barrel"
        || name == "minecraft:hopper"
        || name == "minecraft:dispenser"
        || name == "minecraft:dropper"
        || name == "minecraft:crafter"
        || name == "minecraft:brewing_stand"
        || name == "minecraft:furnace"
        || name == "minecraft:smoker"
        || name == "minecraft:blast_furnace"
        || name.ends_with("chest")
        || name.ends_with("shulker_box")
}

pub(crate) fn load(
    state: &TickState,
    plugin_id: &str,
    endpoint: &Endpoint,
) -> Result<InventoryImage, Failure> {
    let mut slots = BTreeMap::new();
    let identity = match endpoint {
        Endpoint::PlayerInventory { player_id } => {
            let target = player_world(state, ConnectionId(*player_id)).ok_or(Failure::NotFound)?;
            let entity = target
                .connections
                .entity_of(ConnectionId(*player_id))
                .ok_or(Failure::NotFound)?;
            let player = target.players.get(&entity).ok_or(Failure::NotFound)?;
            for slot in 9..=44 {
                slots.insert(
                    slot,
                    player.inventory.slots[player_index(slot).expect("player slot")].clone(),
                );
            }
            target
                .connections
                .uuid_of(player.connection)
                .map(|id| id.to_string())
                .ok_or(Failure::NotFound)?
        }
        Endpoint::Container {
            dimension,
            position,
        } => {
            let target = world(state, dimension)?;
            let mut index = 0_u8;
            let cells = container_cells(target, position)?;
            let mut identity = Vec::new();
            for cell in cells {
                let storage = &target.block_entities[&(cell.x, cell.y, cell.z)];
                identity.push((
                    cell.x,
                    cell.y,
                    cell.z,
                    target.world.block_at(cell.x, cell.y, cell.z),
                ));
                for stack in &storage.slots {
                    if usize::from(index) >= MAX_OWNED_INVENTORY_SLOTS {
                        return Err(Failure::Capacity);
                    }
                    slots.insert(index, stack.clone());
                    index += 1;
                }
            }
            hash(&identity)
        }
        Endpoint::NpcEquipment { handle } | Endpoint::NpcCarry { handle } => {
            let slot = crate::plugin_npcs::owned_slot(state, plugin_id, handle)?;
            let row = state.entities.row(slot);
            match endpoint {
                Endpoint::NpcEquipment { .. } => {
                    for (script, native) in [0, 1, 5, 4, 3, 2].into_iter().enumerate() {
                        slots.insert(
                            script as u8,
                            row.appearance.equipment[native]
                                .clone()
                                .unwrap_or(ItemStack::EMPTY),
                        );
                    }
                }
                _ => {
                    for (i, stack) in row
                        .villager
                        .as_ref()
                        .ok_or(Failure::NotFound)?
                        .inventory
                        .iter()
                        .enumerate()
                    {
                        slots.insert(i as u8, stack.clone().unwrap_or(ItemStack::EMPTY));
                    }
                }
            }
            row.uuid.to_string()
        }
        _ => return Err(Failure::InvalidRequest),
    };
    let snapshot_hash = hash(&(identity, &slots));
    metadata(state)?;
    let mut version: Option<ImageVersion> = None;
    for key in image_keys(state, endpoint) {
        if let Some(candidate) = image_version(state, &key)?
            && version
                .as_ref()
                .is_none_or(|before| candidate.revision >= before.revision)
        {
            version = Some(candidate);
        }
    }
    let revision = match version {
        Some(record) if record.hash == snapshot_hash => record.revision,
        Some(record) => next(record.revision)?,
        None => 1,
    };
    Ok(InventoryImage {
        endpoint: endpoint.clone(),
        slots,
        fence: ScriptInventoryFence::try_new(revision, snapshot_hash)
            .map_err(|_| Failure::InvalidRequest)?,
    })
}

pub(crate) fn to_script_item(
    state: &TickState,
    stack: &ItemStack,
) -> Result<ScriptInventoryItem, Failure> {
    let name = state
        .registry
        .items
        .name_of(stack.item_id)
        .ok_or(Failure::InvalidRequest)?;
    Ok(ScriptInventoryItem::new(
        name.as_str().to_owned(),
        u32::try_from(stack.count).map_err(|_| Failure::InvalidRequest)?,
        stack.damage,
        stack
            .enchantments
            .iter()
            .map(|e| ScriptInventoryEnchantment::new(e.id.as_str().to_owned(), e.level))
            .collect(),
        stack.custom_name.as_deref().map(str::to_owned),
        stack.item_model.as_ref().map(|v| v.as_str().to_owned()),
    )
    .with_components(
        stack.extended().max_damage,
        stack.extended().custom_data.clone(),
        stack
            .stew_effects
            .iter()
            .map(|e| ScriptInventoryStewEffect::new(e.id.as_str().to_owned(), e.duration))
            .collect(),
    ))
}
fn snapshot(
    state: &TickState,
    endpoint: &Endpoint,
    image: &InventoryImage,
) -> Result<ScriptOwnedInventorySnapshot, Failure> {
    let slots = image
        .slots
        .iter()
        .map(|(slot, stack)| {
            Ok(ScriptInventorySlot::new(
                *slot,
                if stack.is_empty() {
                    None
                } else {
                    Some(to_script_item(state, stack)?)
                },
            ))
        })
        .collect::<Result<_, Failure>>()?;
    Ok(ScriptOwnedInventorySnapshot::new(
        endpoint.clone(),
        image.fence.clone(),
        slots,
    ))
}
pub(crate) fn stock(
    state: &TickState,
    image: &InventoryImage,
) -> Result<BTreeMap<String, u64>, Failure> {
    let mut result = BTreeMap::new();
    for stack in image.slots.values().filter(|s| !s.is_empty()) {
        let name = state
            .registry
            .items
            .name_of(stack.item_id)
            .ok_or(Failure::InvalidRequest)?;
        let count = result.entry(name.as_str().to_owned()).or_insert(0_u64);
        *count = count
            .checked_add(stack.count as u64)
            .ok_or(Failure::Capacity)?;
    }
    Ok(result)
}
fn reservation_source_matches(
    state: &TickState,
    reservation: &Reservation,
    endpoint: &Endpoint,
) -> bool {
    match (&reservation.snapshot.endpoint, endpoint) {
        (Endpoint::PlayerInventory { .. }, Endpoint::PlayerInventory { player_id }) => {
            reservation.player_uuid.is_some_and(|uuid| {
                player_world(state, ConnectionId(*player_id))
                    .and_then(|target| target.connections.uuid_of(ConnectionId(*player_id)))
                    == Some(uuid)
            })
        }
        (
            Endpoint::Container {
                dimension: source_dimension,
                ..
            },
            Endpoint::Container {
                dimension,
                position,
            },
        ) if crate::dimensions::Dimension::parse(source_dimension).is_some()
            && crate::dimensions::Dimension::parse(source_dimension)
                == crate::dimensions::Dimension::parse(dimension) =>
        {
            if reservation
                .source_cells
                .contains(&[position.x, position.y, position.z])
            {
                return true;
            }
            world(state, dimension).is_ok_and(|target| {
                custody_partner(
                    target,
                    BlockPos {
                        x: position.x,
                        y: position.y,
                        z: position.z,
                    },
                )
                .is_some_and(|cell| reservation.source_cells.contains(&[cell.x, cell.y, cell.z]))
            })
        }
        _ => false,
    }
}
fn floor(
    state: &TickState,
    endpoint: &Endpoint,
    excluding: Option<&str>,
) -> Result<BTreeMap<String, u64>, Failure> {
    floor_reservations(&endpoint_reservations(state, endpoint)?, excluding)
}
fn floor_reservations(
    reservations: &[Reservation],
    excluding: Option<&str>,
) -> Result<BTreeMap<String, u64>, Failure> {
    let mut result = BTreeMap::new();
    for reservation in reservations {
        if Some(reservation.snapshot.reservation_ref.as_str()) == excluding {
            continue;
        }
        for quantity in &reservation.snapshot.quantities {
            let count = result.entry(quantity.resource_id.clone()).or_insert(0_u64);
            *count = count
                .checked_add(quantity.remaining)
                .filter(|value| *value <= MAX_SCRIPT_WORLD_TIME)
                .ok_or(Failure::Capacity)?;
        }
    }
    Ok(result)
}
#[cfg(test)]
pub(crate) fn reserved(state: &TickState, endpoint: &Endpoint) -> BTreeMap<String, u64> {
    floor(state, endpoint, None).expect("validated native inventory records")
}
pub(crate) fn below_floors(state: &TickState, endpoint: &Endpoint, slots: &[ItemStack]) -> bool {
    let Ok(floors) = floor(state, endpoint, None) else {
        return true;
    };
    for (name, needed) in floors {
        let Ok(id) = Identifier::parse(&name) else {
            return true;
        };
        let Some(id) = state.registry.items.id_of(&id) else {
            return true;
        };
        let actual: u64 = slots
            .iter()
            .filter(|stack| stack.item_id == id && !stack.is_empty())
            .map(|stack| stack.count as u64)
            .sum();
        if actual < needed {
            return true;
        }
    }
    false
}
fn check_floor(
    state: &TickState,
    endpoint: &Endpoint,
    image: &InventoryImage,
    excluding: Option<&str>,
) -> Result<(), Failure> {
    let stock = stock(state, image)?;
    if floor(state, endpoint, excluding)?
        .iter()
        .any(|(name, count)| stock.get(name).copied().unwrap_or(0) < *count)
    {
        return Err(Failure::InsufficientItems);
    }
    Ok(())
}
pub(crate) fn insert(
    state: &TickState,
    image: &mut InventoryImage,
    mut stack: ItemStack,
) -> Result<(), Failure> {
    let mut slots = image.slots.clone();
    for empty in [false, true] {
        for (slot, destination) in &mut slots {
            if stack.is_empty() {
                break;
            }
            if destination.is_empty() != empty {
                continue;
            }
            let room = room_for_slot(state, &image.endpoint, *slot, destination, &stack);
            if room == 0 {
                continue;
            }
            let count = stack.count.min(room as i32);
            if empty {
                *destination = stack.clone();
                destination.count = count;
            } else {
                destination.count += count;
            }
            stack.count -= count;
        }
    }
    if !stack.is_empty() {
        return Err(Failure::Capacity);
    }
    image.slots = slots;
    Ok(())
}
fn room(state: &TickState, destination: &ItemStack, source: &ItemStack) -> u32 {
    if !destination.is_empty()
        && !mc_data::inventory_semantics_26_1_2::can_stack(destination, source)
    {
        return 0;
    }
    let max = state.registry.item_rules().max_stack(source).min(99);
    max.saturating_sub(if destination.is_empty() {
        0
    } else {
        destination.count
    })
    .max(0) as u32
}
fn room_for_slot(
    state: &TickState,
    endpoint: &Endpoint,
    slot: u8,
    destination: &ItemStack,
    source: &ItemStack,
) -> u32 {
    let mut maximum = state.registry.item_rules().max_stack(source).min(99);
    match endpoint {
        Endpoint::NpcEquipment { .. } if slot >= 2 => {
            let required = ["head", "chest", "legs", "feet"][usize::from(slot - 2)];
            if state
                .registry
                .item_facts
                .facts_for_stack(source, &state.registry.items)
                .and_then(|facts| facts.equippable_slot.as_deref())
                != Some(required)
            {
                return 0;
            }
            maximum = 1;
        }
        Endpoint::Container {
            dimension,
            position,
        } => {
            let Ok(target) = world(state, dimension) else {
                return 0;
            };
            let Ok(cells) = container_cells(target, position) else {
                return 0;
            };
            let mut local = usize::from(slot);
            let mut selected = None;
            for cell in cells {
                let size = target.block_entities[&(cell.x, cell.y, cell.z)].slots.len();
                if local < size {
                    selected = Some(cell);
                    break;
                }
                local -= size;
            }
            let Some(cell) = selected else {
                return 0;
            };
            let Some(definition) = target
                .world
                .block_at(cell.x, cell.y, cell.z)
                .and_then(|id| target.registry.blocks.by_id(mc_world::BlockStateId(id)))
            else {
                return 0;
            };
            let rules = target.registry.item_rules();
            match definition.block.id.path() {
                "furnace" | "smoker" | "blast_furnace" => {
                    let kind = match definition.block.id.path() {
                        "smoker" => mc_data::recipes::CookingKind::Smoker,
                        "blast_furnace" => mc_data::recipes::CookingKind::BlastFurnace,
                        _ => mc_data::recipes::CookingKind::Furnace,
                    };
                    if local == crate::inventory::furnace::OUTPUT {
                        return 0;
                    }
                    if local == crate::inventory::furnace::FUEL
                        && !crate::inventory::furnace::is_fuel(&rules, kind, source)
                        && !crate::inventory::furnace::is_empty_bucket(&rules, source)
                    {
                        return 0;
                    }
                    if local == crate::inventory::furnace::FUEL
                        && crate::inventory::furnace::is_empty_bucket(&rules, source)
                    {
                        maximum = 1;
                    }
                }
                "brewing_stand" => {
                    if !rules.brewing_input_allowed(local, source) {
                        return 0;
                    }
                    if local <= 2 {
                        maximum = 1;
                    }
                }
                "crafter" => {
                    if target
                        .crafters
                        .get(&(cell.x, cell.y, cell.z))
                        .is_some_and(|c| c.settings.disabled_slots & (1 << local) != 0)
                    {
                        return 0;
                    }
                }
                name if name.ends_with("shulker_box")
                    && crate::shulker::is_item(&target.registry.items, source) =>
                {
                    return 0;
                }
                _ => {}
            }
        }
        _ => {}
    }
    room(state, destination, source).min(
        maximum
            .saturating_sub(if destination.is_empty() {
                0
            } else {
                destination.count
            })
            .max(0) as u32,
    )
}
pub(crate) fn plan_debit(
    state: &TickState,
    plugin_id: &str,
    endpoint: &Endpoint,
    materials: &BTreeMap<String, u64>,
) -> Result<InventoryImage, Failure> {
    let mut image = load(state, plugin_id, endpoint)?;
    debit(state, &mut image, materials)?;
    Ok(image)
}
fn debit(
    state: &TickState,
    image: &mut InventoryImage,
    materials: &BTreeMap<String, u64>,
) -> Result<(), Failure> {
    for (resource, amount) in materials {
        let id = Identifier::parse(resource).map_err(|_| Failure::InvalidRequest)?;
        let id = state
            .registry
            .items
            .id_of(&id)
            .ok_or(Failure::InvalidRequest)?;
        let mut remaining = *amount;
        for stack in image
            .slots
            .values_mut()
            .filter(|s| !s.is_empty() && s.item_id == id)
        {
            let count = remaining.min(stack.count as u64);
            stack.count -= count as i32;
            remaining -= count;
            if stack.is_empty() {
                *stack = ItemStack::EMPTY;
            }
            if remaining == 0 {
                break;
            }
        }
        if remaining != 0 {
            return Err(Failure::InsufficientItems);
        }
    }
    Ok(())
}

fn authorize_mutation(state: &TickState, owner: &str, endpoint: &Endpoint) -> Result<(), Failure> {
    if let Endpoint::Container {
        dimension,
        position,
    } = endpoint
    {
        let target = world(state, dimension)?;
        for cell in container_cells(target, position)? {
            if !crate::plugin_player::allows_plugin_bounds(
                state,
                owner,
                dimension,
                [cell.x, cell.y, cell.z],
                [cell.x, cell.y, cell.z],
            ) {
                return Err(Failure::Forbidden);
            }
        }
    }
    Ok(())
}
pub(crate) fn commit(
    state: &mut TickState,
    plugin_id: &str,
    images: BTreeMap<Endpoint, InventoryImage>,
) -> Result<(), Failure> {
    let mut physical = std::collections::BTreeSet::new();
    for (endpoint, image) in &images {
        authorize_mutation(state, plugin_id, endpoint)?;
        if !physical.insert(endpoint_key(state, endpoint)) {
            return Err(Failure::InvalidRequest);
        }
        let current = load(state, plugin_id, endpoint)?;
        if current.fence != image.fence || current.slots.keys().ne(image.slots.keys()) {
            return Err(Failure::StaleRevision);
        }
        check_floor(state, endpoint, image, None)?;
        next(current.fence.revision)?;
    }
    apply(state, plugin_id, images)
}
fn apply(
    state: &mut TickState,
    plugin_id: &str,
    images: BTreeMap<Endpoint, InventoryImage>,
) -> Result<(), Failure> {
    // All endpoint resolution, slot shapes and floor checks precede any mutation.
    for (endpoint, image) in &images {
        match endpoint {
            Endpoint::PlayerInventory { player_id } => {
                let dimension = player_world(state, ConnectionId(*player_id))
                    .expect("preflight player dimension")
                    .dimension;
                let target =
                    world_mut(state, dimension.name()).expect("preflight player dimension");
                let entity = target
                    .connections
                    .entity_of(ConnectionId(*player_id))
                    .expect("preflight player");
                let player = target
                    .players
                    .get_mut(&entity)
                    .expect("preflight inventory");
                for (slot, stack) in &image.slots {
                    player.inventory.slots[player_index(*slot).expect("preflight slot")] =
                        stack.clone();
                }
                player.inventory_menu.state_id = player.inventory_menu.state_id.wrapping_add(1);
                let connection = player.connection;
                let change = player.inventory_contents();
                target.push_player(connection, change);
            }
            Endpoint::Container {
                dimension,
                position,
            } => {
                let target = world_mut(state, dimension).expect("preflight dimension");
                let cells = container_cells(target, position).expect("preflight container");
                let mut index = 0_u8;
                for cell in cells {
                    let storage = target
                        .block_entities
                        .get_mut(&(cell.x, cell.y, cell.z))
                        .expect("preflight storage");
                    for stack in &mut storage.slots {
                        *stack = image.slots[&index].clone();
                        index += 1;
                    }
                    let position = mc_world::BlockPos {
                        x: cell.x,
                        y: cell.y,
                        z: cell.z,
                    };
                    crate::redstone::containers::inventory_changed(
                        &mut target.redstone_containers,
                        position,
                    );
                    target.world.mark_block_entity_dirty(position);
                }
            }
            Endpoint::NpcCarry { handle } | Endpoint::NpcEquipment { handle } => {
                let slot = crate::plugin_npcs::owned_slot(state, plugin_id, handle)
                    .expect("preflight npc");
                let row = state.entities.row_mut(slot);
                let slots = match endpoint {
                    Endpoint::NpcCarry { .. } => {
                        &mut row.villager.as_mut().expect("preflight carry").inventory[..]
                    }
                    _ => &mut std::sync::Arc::make_mut(&mut row.appearance).equipment[..],
                };
                for (i, stack) in slots.iter_mut().enumerate() {
                    let script_index = if matches!(endpoint, Endpoint::NpcEquipment { .. }) {
                        [0, 1, 5, 4, 3, 2][i]
                    } else {
                        i
                    };
                    let item = &image.slots[&(script_index as u8)];
                    *stack = (!item.is_empty()).then(|| item.clone());
                }
                state.appearance_changed.push(state.entities.body(slot).id);
            }
            _ => unreachable!("preflight endpoint"),
        }
        let after = load(state, plugin_id, endpoint)?;
        remember(
            state,
            endpoint,
            &after.fence.snapshot_hash,
            next(image.fence.revision)?,
        )?;
        if let Endpoint::Container {
            dimension,
            position,
        } = endpoint
        {
            let target = world_mut(state, dimension).expect("preflight dimension");
            crate::tick::refresh_container_viewers(
                target,
                BlockPos {
                    x: position.x,
                    y: position.y,
                    z: position.z,
                },
            );
        }
    }
    Ok(())
}

pub(crate) fn reservation(
    state: &TickState,
    plugin_id: &str,
    reference: &str,
) -> Result<Reservation, Failure> {
    metadata(state)?;
    let mut value = stored_reservation(state, reference)?;
    if value.owner != plugin_id {
        return Err(Failure::NotFound);
    }
    // Rebind only the requested reservation, never the whole custody ledger.
    if let Some(uuid) = value.player_uuid {
        for target in std::iter::once(state).chain(state.other_dimensions.values().map(Box::as_ref))
        {
            if let Some(connection) = target
                .connections
                .all_connections()
                .find(|connection| target.connections.uuid_of(*connection) == Some(uuid))
            {
                value.snapshot.endpoint = Endpoint::PlayerInventory {
                    player_id: connection.0,
                };
                break;
            }
        }
    }
    Ok(value)
}
struct PreparedReservation {
    value: Reservation,
    image: serde_json::Value,
    metadata: Metadata,
    old_keys: Vec<String>,
    new_keys: Vec<String>,
    retired: Option<(String, Option<String>)>,
    save_metadata: bool,
}
pub(crate) fn save_reservation(state: &mut TickState, value: Reservation) -> Result<(), Failure> {
    let prepared = prepare_reservation(state, value)?;
    apply_reservation(state, prepared);
    Ok(())
}
// Resolve the retention victim and every fallible domain read before a physical debit.
fn prepare_reservation(
    state: &TickState,
    value: Reservation,
) -> Result<PreparedReservation, Failure> {
    let mut metadata = metadata(state)?;
    let reference = &value.snapshot.reservation_ref;
    let before = match stored_reservation(state, reference) {
        Ok(before) => Some(before),
        Err(Failure::NotFound) => None,
        Err(error) => return Err(error),
    };
    let was_protected = before.as_ref().is_some_and(protected);
    let is_protected = protected(&value);
    if before.as_ref().is_some_and(|before| !protected(before)) && is_protected {
        return Err(Failure::StaleRevision);
    }
    if !was_protected && is_protected {
        if metadata.protected_count == MAX_PROTECTED_RESERVATIONS {
            return Err(Failure::Busy);
        }
        metadata.protected_count += 1;
    } else if was_protected && !is_protected {
        metadata.protected_count = metadata
            .protected_count
            .checked_sub(1)
            .ok_or(Failure::RuntimeUnavailable)?;
    }
    let retire = !is_protected && (was_protected || before.is_none());
    let retired_key = retire.then(|| format!("retired/{}", metadata.retired_next));
    let evicted = retired_key
        .as_deref()
        .and_then(|key| state.plugins.get_domain(OWNER, key))
        .map(|value| {
            value
                .as_str()
                .ok_or(Failure::RuntimeUnavailable)
                .map(str::to_owned)
        })
        .transpose()?;
    if retire && evicted.is_some() != (metadata.retired_count == MAX_TERMINAL_RESERVATIONS) {
        return Err(Failure::RuntimeUnavailable);
    }
    if let Some(evicted) = &evicted
        && protected(&stored_reservation(state, evicted)?)
    {
        return Err(Failure::RuntimeUnavailable);
    }
    let old_keys = before.as_ref().map(index_keys).unwrap_or_default();
    let new_keys = index_keys(&value);
    let save_metadata =
        retire || was_protected != is_protected || state.plugins.get_domain(OWNER, KEY).is_none();
    if retire {
        metadata.retired_next = (metadata.retired_next + 1) % MAX_TERMINAL_RESERVATIONS;
        metadata.retired_count = (metadata.retired_count + 1).min(MAX_TERMINAL_RESERVATIONS);
    }
    Ok(PreparedReservation {
        image: serde_json::to_value(&value).expect("inventory reservation serialization"),
        value,
        metadata,
        old_keys,
        new_keys,
        retired: retired_key.map(|key| (key, evicted)),
        save_metadata,
    })
}
fn apply_reservation(state: &mut TickState, prepared: PreparedReservation) -> Reservation {
    let PreparedReservation {
        value,
        image,
        metadata,
        old_keys,
        new_keys,
        retired,
        save_metadata: dirty_metadata,
    } = prepared;
    for key in old_keys.iter().filter(|key| !new_keys.contains(key)) {
        state.plugins.delete_domain(OWNER, key);
    }
    for key in new_keys.iter().filter(|key| !old_keys.contains(key)) {
        state
            .plugins
            .set_domain(OWNER, key, serde_json::Value::Bool(true));
    }
    if let Some((key, evicted)) = retired {
        if let Some(evicted) = evicted {
            state
                .plugins
                .delete_domain(OWNER, &format!("reservation/{evicted}"));
        }
        state.plugins.set_domain(
            OWNER,
            &key,
            serde_json::Value::String(value.snapshot.reservation_ref.clone()),
        );
    }
    state.plugins.set_domain(
        OWNER,
        &format!("reservation/{}", value.snapshot.reservation_ref),
        image,
    );
    if dirty_metadata {
        save_metadata(state, &metadata);
    }
    value
}
pub(crate) fn validate_route_reservations(
    state: &TickState,
    plugin_id: &str,
    refs: &[String],
) -> Result<(), Failure> {
    for reference in refs {
        if reservation(state, plugin_id, reference)?.snapshot.released {
            return Err(Failure::StaleRevision);
        }
    }
    Ok(())
}
pub(crate) fn commit_reserved_debit(
    state: &mut TickState,
    plugin_id: &str,
    mut reservation: Reservation,
    materials: &BTreeMap<String, u64>,
    binding: &str,
) -> Result<(), Failure> {
    if reservation.snapshot.released
        || reservation
            .snapshot
            .bound_to
            .as_deref()
            .is_some_and(|id| id != binding)
    {
        return Err(Failure::Blocked);
    }
    if !protected(&stored_reservation(
        state,
        &reservation.snapshot.reservation_ref,
    )?) {
        return Err(Failure::StaleRevision);
    }
    let endpoint = reservation.snapshot.endpoint.clone();
    authorize_mutation(state, plugin_id, &endpoint)?;
    let image = plan_debit(state, plugin_id, &endpoint, materials)?;
    for (name, count) in materials {
        let quantity = reservation
            .snapshot
            .quantities
            .iter_mut()
            .find(|q| &q.resource_id == name)
            .ok_or(Failure::InsufficientItems)?;
        quantity.remaining = quantity
            .remaining
            .checked_sub(*count)
            .ok_or(Failure::InsufficientItems)?;
        quantity.consumed = quantity
            .consumed
            .checked_add(*count)
            .ok_or(Failure::Capacity)?;
    }
    check_floor(
        state,
        &endpoint,
        &image,
        Some(&reservation.snapshot.reservation_ref),
    )?;
    let available = stock(state, &image)?;
    let others = floor(
        state,
        &endpoint,
        Some(&reservation.snapshot.reservation_ref),
    )?;
    for quantity in &reservation.snapshot.quantities {
        let other = others.get(&quantity.resource_id).copied().unwrap_or(0);
        if available.get(&quantity.resource_id).copied().unwrap_or(0) < other + quantity.remaining {
            return Err(Failure::InsufficientItems);
        }
    }
    reservation.snapshot.bound_to = Some(binding.to_owned());
    reservation.revision = next(reservation.revision)?;
    reservation.snapshot.receipt_watermark = next(reservation.snapshot.receipt_watermark)?;
    next(image.fence.revision)?;
    let prepared = prepare_reservation(state, reservation)?;
    apply(state, plugin_id, BTreeMap::from([(endpoint, image)]))?;
    apply_reservation(state, prepared);
    Ok(())
}

pub(crate) fn execute(
    state: &mut TickState,
    plugin_id: &str,
    request: &ScriptOperationRequest,
) -> ScriptOperationOutcome {
    let ScriptOperation::Inventory { operation } = request.operation() else {
        return ScriptOperationOutcome::rejected(Failure::InvalidRequest);
    };
    match execute_operation(state, plugin_id, operation) {
        Ok((revision, result)) => ScriptOperationOutcome::committed(
            revision,
            ScriptOperationPayload::OwnedInventory {
                result: Box::new(result),
            },
        )
        .expect("validated native inventory result"),
        Err(failure) => ScriptOperationOutcome::rejected(failure),
    }
}
fn execute_operation(
    state: &mut TickState,
    plugin_id: &str,
    operation: &ScriptOwnedInventoryOperation,
) -> Result<(u64, ScriptOwnedInventoryResult), Failure> {
    operation.validate().map_err(|_| Failure::InvalidRequest)?;
    match operation {
        ScriptOwnedInventoryOperation::Query {
            endpoint,
            expected_revision,
        } => {
            let image = load(state, plugin_id, endpoint)?;
            if expected_revision.is_some_and(|v| v != image.fence.revision) {
                return Err(Failure::StaleRevision);
            }
            let result = snapshot(state, endpoint, &image)?;
            remember(
                state,
                endpoint,
                &image.fence.snapshot_hash,
                image.fence.revision,
            )?;
            Ok((
                image.fence.revision,
                ScriptOwnedInventoryResult::Snapshot { inventory: result },
            ))
        }
        ScriptOwnedInventoryOperation::PlacementRoom {
            source,
            source_slot,
            destination,
        } => {
            let source_image = load(state, plugin_id, &source.endpoint)?;
            let destination_image = load(state, plugin_id, &destination.endpoint)?;
            if source_image.fence != source.fence || destination_image.fence != destination.fence {
                return Err(Failure::StaleRevision);
            }
            let stack = source_image
                .slots
                .get(source_slot)
                .ok_or(Failure::InvalidRequest)?;
            if stack.is_empty() {
                return Err(Failure::InsufficientItems);
            }
            let slots = destination_image
                .slots
                .iter()
                .map(|(slot, dest)| {
                    ScriptInventoryPlacementSlot::new(
                        *slot,
                        if same_endpoint(state, &source.endpoint, &destination.endpoint)
                            && source_slot == slot
                        {
                            0
                        } else {
                            room_for_slot(state, &destination.endpoint, *slot, dest, stack)
                        },
                    )
                })
                .collect();
            Ok((
                source_image
                    .fence
                    .revision
                    .max(destination_image.fence.revision),
                ScriptOwnedInventoryResult::PlacementRoom {
                    source: source.clone(),
                    source_slot: *source_slot,
                    destination: destination.clone(),
                    slots,
                },
            ))
        }
        ScriptOwnedInventoryOperation::Reserve {
            endpoint,
            resource_plan,
            expected_revision,
            ..
        } => {
            if !matches!(
                endpoint,
                Endpoint::PlayerInventory { .. } | Endpoint::Container { .. }
            ) {
                return Err(Failure::InvalidRequest);
            }
            if let Endpoint::PlayerInventory { player_id } = endpoint {
                let target =
                    player_world(state, ConnectionId(*player_id)).ok_or(Failure::NotFound)?;
                let entity = target
                    .connections
                    .entity_of(ConnectionId(*player_id))
                    .ok_or(Failure::NotFound)?;
                if target.players[&entity].has_active_use()
                    || target.entities.slot_of(entity).is_some_and(|slot| {
                        crate::physical_defense::item_use(target, slot)
                            .is_some_and(|control| control.equipment_slot.is_some())
                    })
                {
                    return Err(Failure::Busy);
                }
            }
            authorize_mutation(state, plugin_id, endpoint)?;
            let image = load(state, plugin_id, endpoint)?;
            if &image.fence != expected_revision {
                return Err(Failure::StaleRevision);
            }
            let mut meta = metadata(state)?;
            if meta.protected_count == MAX_PROTECTED_RESERVATIONS {
                return Err(Failure::Busy);
            }
            let mut totals = BTreeMap::new();
            for portion in &resource_plan.portions {
                for material in &portion.materials {
                    let count = totals.entry(material.resource_id.clone()).or_insert(0_u64);
                    *count = count
                        .checked_add(material.quantity)
                        .filter(|v| *v <= MAX_SCRIPT_WORLD_TIME)
                        .ok_or(Failure::Capacity)?;
                }
            }
            let available = stock(state, &image)?;
            let reserved = floor(state, endpoint, None)?;
            for (name, count) in &totals {
                if available.get(name).copied().unwrap_or(0)
                    < count + reserved.get(name).copied().unwrap_or(0)
                {
                    return Err(Failure::InsufficientItems);
                }
            }
            meta.revision = next(meta.revision)?;
            let reference = uuid::Uuid::new_v4().simple().to_string();
            let snapshot = ScriptInventoryReservationSnapshot::new(
                reference,
                endpoint.clone(),
                resource_plan_hash(resource_plan),
                totals
                    .into_iter()
                    .map(|(name, count)| {
                        ScriptInventoryReservationQuantity::new(name, count, 0, 0, count)
                    })
                    .collect(),
                None,
                false,
                0,
            );
            let mut prepared = prepare_reservation(
                state,
                Reservation {
                    owner: plugin_id.to_owned(),
                    revision: meta.revision,
                    player_uuid: if let Endpoint::PlayerInventory { player_id } = endpoint {
                        player_world(state, ConnectionId(*player_id))
                            .and_then(|target| target.connections.uuid_of(ConnectionId(*player_id)))
                    } else {
                        None
                    },
                    source_cells: if let Endpoint::Container {
                        dimension,
                        position,
                    } = endpoint
                    {
                        container_cells(world(state, dimension)?, position)?
                            .iter()
                            .map(|cell| [cell.x, cell.y, cell.z])
                            .collect()
                    } else {
                        Vec::new()
                    },
                    snapshot,
                },
            )?;
            prepared.metadata.revision = meta.revision;
            let value = apply_reservation(state, prepared);
            Ok((
                meta.revision,
                ScriptOwnedInventoryResult::Reservation {
                    reservation: value.snapshot,
                },
            ))
        }
        ScriptOwnedInventoryOperation::ReservationStatus { reservation_ref } => {
            let value = reservation(state, plugin_id, reservation_ref)?;
            Ok((
                value.revision,
                ScriptOwnedInventoryResult::Reservation {
                    reservation: value.snapshot,
                },
            ))
        }
        ScriptOwnedInventoryOperation::Release {
            reservation_ref,
            expected_revision,
            ..
        } => {
            let mut value = reservation(state, plugin_id, reservation_ref)?;
            if value.revision != *expected_revision || value.snapshot.released {
                return Err(Failure::StaleRevision);
            }
            if value.snapshot.bound_to.is_some() {
                return Err(Failure::Forbidden);
            }
            for quantity in &mut value.snapshot.quantities {
                quantity.returned += quantity.remaining;
                quantity.remaining = 0;
            }
            value.snapshot.released = true;
            value.revision = next(value.revision)?;
            let prepared = prepare_reservation(state, value)?;
            let value = apply_reservation(state, prepared);
            Ok((
                value.revision,
                ScriptOwnedInventoryResult::Reservation {
                    reservation: value.snapshot,
                },
            ))
        }
        ScriptOwnedInventoryOperation::Transfer {
            actor_id,
            execution_permit,
            transfers,
            expected_revisions,
            ..
        }
        | ScriptOwnedInventoryOperation::ConsumeReservation {
            actor_id,
            execution_permit,
            transfers,
            expected_revisions,
            ..
        } => {
            let mut images = BTreeMap::new();
            for expected in expected_revisions {
                if let Endpoint::PlayerInventory { player_id } = &expected.endpoint
                    && player_id != actor_id
                {
                    return Err(Failure::Forbidden);
                }
                if let Some(handle) = expected.endpoint.npc_handle() {
                    crate::plugin_npcs::validate_permit(
                        state,
                        plugin_id,
                        handle,
                        execution_permit.as_ref(),
                    )?;
                }
                let image = load(state, plugin_id, &expected.endpoint)?;
                authorize_mutation(state, plugin_id, &expected.endpoint)?;
                if images
                    .keys()
                    .any(|other| same_endpoint(state, other, &expected.endpoint))
                {
                    return Err(Failure::InvalidRequest);
                }
                if image.fence != expected.fence {
                    return Err(Failure::StaleRevision);
                }
                images.insert(expected.endpoint.clone(), image);
            }
            let mut consumption = None;
            let mut consumed = BTreeMap::<String, u64>::new();
            if let ScriptOwnedInventoryOperation::ConsumeReservation {
                reservation_ref,
                expected_reservation_revision,
                portion,
                resource_plan,
                ..
            } = operation
            {
                let mut value = reservation(state, plugin_id, reservation_ref)?;
                if value.revision != *expected_reservation_revision || value.snapshot.released {
                    return Err(Failure::StaleRevision);
                }
                if value.snapshot.bound_to.is_some() {
                    return Err(Failure::Forbidden);
                }
                let actor_participates = match &value.snapshot.endpoint {
                    Endpoint::PlayerInventory { player_id } => player_id == actor_id,
                    Endpoint::Container {..} => *actor_id == 0 || images.keys().any(|endpoint|
                        matches!(endpoint,Endpoint::PlayerInventory {player_id} if player_id == actor_id)),
                    _ => false,
                };
                if !actor_participates
                    || !transfers
                        .iter()
                        .any(|transfer| transfer.source == value.snapshot.endpoint)
                {
                    return Err(Failure::Forbidden);
                }
                if value.snapshot.resource_plan_hash != resource_plan_hash(resource_plan) {
                    return Err(Failure::InvalidRequest);
                }
                let next_portion = resource_plan
                    .portions
                    .iter()
                    .enumerate()
                    .skip(value.snapshot.receipt_watermark as usize)
                    .find(|(_, p)| !p.materials.is_empty())
                    .ok_or(Failure::StaleRevision)?;
                if next_portion.0 as u64 + 1 != *portion {
                    return Err(Failure::StaleRevision);
                }
                for material in &next_portion.1.materials {
                    consumed.insert(material.resource_id.clone(), material.quantity);
                }
                value.snapshot.receipt_watermark = *portion;
                value.revision = next(value.revision)?;
                consumption = Some(value);
            }
            let mut actual = BTreeMap::<String, u64>::new();
            for transfer in transfers {
                let source = images
                    .get_mut(&transfer.source)
                    .ok_or(Failure::InvalidRequest)?;
                let source_stack = source
                    .slots
                    .get_mut(&transfer.source_slot)
                    .ok_or(Failure::InvalidRequest)?;
                if source_stack.is_empty() || source_stack.count < transfer.count as i32 {
                    return Err(Failure::InsufficientItems);
                }
                let mut moved = source_stack.clone();
                moved.count = transfer.count as i32;
                if let Some(value) = &consumption {
                    if transfer.destination == value.snapshot.endpoint {
                        return Err(Failure::InvalidRequest);
                    }
                    if transfer.source == value.snapshot.endpoint {
                        let name = state
                            .registry
                            .items
                            .name_of(moved.item_id)
                            .ok_or(Failure::InvalidRequest)?
                            .as_str()
                            .to_owned();
                        *actual.entry(name).or_insert(0) += u64::from(transfer.count);
                    }
                }
                source_stack.count -= moved.count;
                if source_stack.is_empty() {
                    *source_stack = ItemStack::EMPTY;
                }
                let destination = images
                    .get_mut(&transfer.destination)
                    .ok_or(Failure::InvalidRequest)?;
                let dest = destination
                    .slots
                    .get_mut(&transfer.destination_slot)
                    .ok_or(Failure::InvalidRequest)?;
                if room_for_slot(
                    state,
                    &transfer.destination,
                    transfer.destination_slot,
                    dest,
                    &moved,
                ) < transfer.count
                {
                    return Err(Failure::Capacity);
                }
                if dest.is_empty() {
                    *dest = moved;
                } else {
                    dest.count += moved.count;
                }
            }
            if let Some(value) = consumption.as_mut() {
                if consumed != actual {
                    return Err(Failure::InvalidRequest);
                }
                for (name, count) in &consumed {
                    let q = value
                        .snapshot
                        .quantities
                        .iter_mut()
                        .find(|q| &q.resource_id == name)
                        .ok_or(Failure::InsufficientItems)?;
                    q.remaining = q
                        .remaining
                        .checked_sub(*count)
                        .ok_or(Failure::InsufficientItems)?;
                    q.consumed += *count;
                }
            }
            for (endpoint, image) in &images {
                check_floor(
                    state,
                    endpoint,
                    image,
                    consumption
                        .as_ref()
                        .map(|v| v.snapshot.reservation_ref.as_str()),
                )?;
                if let Some(value) = &consumption
                    && &value.snapshot.endpoint == endpoint
                {
                    let after = stock(state, image)?;
                    let others = floor(state, endpoint, Some(&value.snapshot.reservation_ref))?;
                    for q in &value.snapshot.quantities {
                        if after.get(&q.resource_id).copied().unwrap_or(0)
                            < q.remaining + others.get(&q.resource_id).copied().unwrap_or(0)
                        {
                            return Err(Failure::InsufficientItems);
                        }
                    }
                }
                next(image.fence.revision)?;
            }
            let consumption = consumption
                .map(|value| prepare_reservation(state, value))
                .transpose()?;
            let endpoints: Vec<_> = images.keys().cloned().collect();
            apply(state, plugin_id, images)?;
            let inventories = endpoints
                .iter()
                .map(|e| load(state, plugin_id, e).and_then(|i| snapshot(state, e, &i)))
                .collect::<Result<Vec<_>, Failure>>()?;
            let revision = inventories
                .iter()
                .map(|i| i.fence.revision)
                .max()
                .expect("nonempty transfer");
            if let Some(prepared) = consumption {
                let value = apply_reservation(state, prepared);
                Ok((
                    revision,
                    ScriptOwnedInventoryResult::ReservedTransfer {
                        reservation: value.snapshot,
                        inventories,
                    },
                ))
            } else {
                Ok((
                    revision,
                    ScriptOwnedInventoryResult::Transfer {
                        inventories: inventories
                            .into_iter()
                            .map(|i| ScriptInventoryExpectedRevision::new(i.endpoint, i.fence))
                            .collect(),
                    },
                ))
            }
        }
        _ => Err(Failure::InvalidRequest),
    }
}

fn exhausted_image(state: &TickState, endpoint: &Endpoint) -> bool {
    metadata(state).is_err()
        || image_version(state, &endpoint_key(state, endpoint)).map_or(true, |image| {
            image.is_some_and(|image| image.revision == u64::MAX)
        })
}
pub(crate) fn has_reservations_for_player(
    state: &TickState,
    entity: crate::boundary::EntityId,
) -> bool {
    let Some(connection) = state.connections.entity_connection(entity) else {
        return false;
    };
    if state.plugins.domain_entries(OWNER).next().is_none() {
        return false;
    }
    if metadata(state).is_err() {
        return true;
    }
    let Some(uuid) = state.connections.uuid_of(connection) else {
        return true;
    };
    has_source(state, &format!("player:{uuid}"))
}
pub(crate) fn has_reservations_for_block(state: &TickState, pos: BlockPos) -> bool {
    if state.plugins.domain_entries(OWNER).next().is_none() {
        return false;
    }
    if metadata(state).is_err() {
        return true;
    }
    has_source(
        state,
        &cell_key(state.dimension.name(), [pos.x, pos.y, pos.z]),
    ) || custody_partner(state, pos).is_some_and(|partner| {
        has_source(
            state,
            &cell_key(state.dimension.name(), [partner.x, partner.y, partner.z]),
        )
    })
}
fn has_source(state: &TickState, source: &str) -> bool {
    let prefix = format!("source/{source}/");
    state.plugins.domain_prefix(OWNER, &prefix).next().is_some()
}
/// Candidate post-image of one native cell; other double-chest halves retain
/// their current custody. This permits free stock without weakening any hold.
pub(crate) fn below_block_floors(
    state: &TickState,
    pos: BlockPos,
    candidate: &[ItemStack],
) -> bool {
    if state.plugins.domain_entries(OWNER).next().is_none() {
        return false;
    }
    let endpoint = Endpoint::Container {
        dimension: state.dimension.name().to_owned(),
        position: ScriptBlockPosition::new(pos.x, pos.y, pos.z),
    };
    let Ok(reservations) = endpoint_reservations(state, &endpoint) else {
        return true;
    };
    let Ok(floors) = floor_reservations(&reservations, None) else {
        return true;
    };
    if floors.values().all(|remaining| *remaining == 0) {
        return false;
    }
    let mut cells = vec![pos];
    if let Some(partner) = custody_partner(state, pos) {
        cells.push(partner);
    }
    for reservation in &reservations {
        for [x, y, z] in &reservation.source_cells {
            let cell = BlockPos {
                x: *x,
                y: *y,
                z: *z,
            };
            if !cells.contains(&cell) {
                cells.push(cell);
            }
        }
    }
    for cell in &cells {
        if *cell != pos
            && (state.world.block_at(cell.x, cell.y, cell.z).is_none()
                || !state.block_entities.contains_key(&(cell.x, cell.y, cell.z)))
        {
            return true;
        }
    }
    for (name, needed) in floors {
        let Ok(identifier) = Identifier::parse(&name) else {
            return true;
        };
        let Some(resource) = state.registry.items.id_of(&identifier) else {
            return true;
        };
        let actual: u64 = cells
            .iter()
            .flat_map(|cell| {
                if *cell == pos {
                    candidate
                } else {
                    state.block_entities[&(cell.x, cell.y, cell.z)]
                        .slots
                        .as_slice()
                }
            })
            .filter(|stack| stack.item_id == resource && !stack.is_empty())
            .map(|stack| stack.count as u64)
            .sum();
        if actual < needed {
            return true;
        }
    }
    false
}
pub(crate) fn below_player_floors(
    state: &TickState,
    entity: crate::boundary::EntityId,
    candidate: &[ItemStack],
) -> bool {
    let Some(connection) = state.connections.entity_connection(entity) else {
        return false;
    };
    below_floors(
        state,
        &Endpoint::PlayerInventory {
            player_id: connection.0,
        },
        &candidate[..candidate.len().min(36)],
    )
}

/// Evaluates a sparse native cost plan without cloning component-bearing stacks.
pub(crate) fn below_player_debit_floors(
    state: &TickState,
    entity: crate::boundary::EntityId,
    debits: &[(usize, u32)],
    broken_slot: Option<usize>,
) -> bool {
    let Some(connection) = state.connections.entity_connection(entity) else {
        return false;
    };
    let Some(player) = state.players.get(&entity) else {
        return true;
    };
    let endpoint = Endpoint::PlayerInventory {
        player_id: connection.0,
    };
    if exhausted_image(state, &endpoint)
        || broken_slot.is_some_and(|slot| slot >= player.inventory.slots.len())
    {
        return true;
    }
    for &(slot, _) in debits {
        let Some(stack) = player.inventory.slots.get(slot) else {
            return true;
        };
        let removed = debits
            .iter()
            .filter(|(index, _)| *index == slot)
            .map(|(_, count)| u64::from(*count))
            .sum::<u64>();
        if removed > stack.count.max(0) as u64 {
            return true;
        }
    }
    let Ok(floors) = floor(state, &endpoint, None) else {
        return true;
    };
    for (name, needed) in floors {
        let Ok(identifier) = Identifier::parse(&name) else {
            return true;
        };
        let Some(id) = state.registry.items.id_of(&identifier) else {
            return true;
        };
        let mut actual = 0_u64;
        for (index, stack) in player.inventory.slots[..36].iter().enumerate() {
            if stack.item_id != id || stack.is_empty() || broken_slot == Some(index) {
                continue;
            }
            let removed = debits
                .iter()
                .filter(|(slot, _)| *slot == index)
                .map(|(_, count)| u64::from(*count))
                .sum::<u64>();
            actual += stack.count as u64 - removed;
        }
        if actual < needed {
            return true;
        }
    }
    false
}

/// Applies borrowed slot overlays to ordinary stock; no full inventory image.
pub(crate) fn below_player_replacement_floors(
    state: &TickState,
    entity: crate::boundary::EntityId,
    replacements: &[(usize, &ItemStack)],
) -> bool {
    let Some(connection) = state.connections.entity_connection(entity) else {
        return false;
    };
    let Some(player) = state.players.get(&entity) else {
        return true;
    };
    if replacements
        .iter()
        .any(|(slot, _)| *slot >= player.inventory.slots.len())
    {
        return true;
    }
    let endpoint = Endpoint::PlayerInventory {
        player_id: connection.0,
    };
    if replacements.iter().any(|(slot, _)| *slot < 36) && exhausted_image(state, &endpoint) {
        return true;
    }
    let Ok(floors) = floor(state, &endpoint, None) else {
        return true;
    };
    for (name, needed) in floors {
        let Ok(identifier) = Identifier::parse(&name) else {
            return true;
        };
        let Some(id) = state.registry.items.id_of(&identifier) else {
            return true;
        };
        let mut actual = 0_u64;
        for (slot, current) in player.inventory.slots[..36].iter().enumerate() {
            let after = replacements
                .iter()
                .rev()
                .find(|(index, _)| *index == slot)
                .map_or(current, |(_, stack)| *stack);
            if after.item_id == id && !after.is_empty() {
                actual += after.count as u64;
            }
        }
        if actual < needed {
            return true;
        }
    }
    false
}

/// Checks the final aggregate stock of each native owner after ordered claims.
/// Slot replacement is not insertion: a `put_item` overwrites the remainder.
pub(crate) fn below_intent_floors(state: &TickState, claims: &[crate::intent::InvClaim]) -> bool {
    if claims.is_empty() {
        return false;
    }
    if state.plugins.domain_entries(OWNER).next().is_none() {
        return false;
    }
    for (index, claim) in claims.iter().enumerate() {
        if claims[..index]
            .iter()
            .any(|before| before.owner == claim.owner)
            || !claims
                .iter()
                .any(|other| other.owner == claim.owner && other.slot < 36)
        {
            continue;
        }
        let Some(player) = state.players.get(&claim.owner) else {
            return true;
        };
        let Some(connection) = state.connections.entity_connection(claim.owner) else {
            return true;
        };
        let endpoint = Endpoint::PlayerInventory {
            player_id: connection.0,
        };
        if exhausted_image(state, &endpoint) {
            return true;
        }
        let Ok(floors) = floor(state, &endpoint, None) else {
            return true;
        };
        for (name, needed) in floors {
            let Ok(identifier) = Identifier::parse(&name) else {
                return true;
            };
            let Some(resource) = state.registry.items.id_of(&identifier) else {
                return true;
            };
            let mut actual = 0_u64;
            for (slot, stack) in player.inventory.slots[..36].iter().enumerate() {
                let mut item = stack.item_id;
                let mut count = stack.count;
                for update in claims.iter().filter(|update| {
                    update.owner == claim.owner && usize::from(update.slot) == slot
                }) {
                    if update.take < 0 || update.put_count < 0 {
                        return true;
                    }
                    if let Some(put) = update.put_item {
                        item = put;
                        count = update.put_count;
                    } else if item != 0 && count > 0 {
                        count = (count - update.take).max(0);
                    }
                }
                if item == resource && item != 0 && count > 0 {
                    actual += count as u64;
                }
            }
            if actual < needed {
                return true;
            }
        }
    }
    false
}

/// Death/removal unlocks remaining stock in the same tick as its native drops.
/// Accounting records the unspent hold as returned, never mints a replacement.
pub(crate) fn invalidate_player(state: &mut TickState, entity: crate::boundary::EntityId) {
    let Some(connection) = state.connections.entity_connection(entity) else {
        return;
    };
    invalidate(
        state,
        &Endpoint::PlayerInventory {
            player_id: connection.0,
        },
    );
}
pub(crate) fn invalidate_block(state: &mut TickState, pos: BlockPos) {
    if state.plugins.domain_entries(OWNER).next().is_none() {
        return;
    }
    let endpoint = Endpoint::Container {
        dimension: state.dimension.name().to_owned(),
        position: ScriptBlockPosition::new(pos.x, pos.y, pos.z),
    };
    invalidate(state, &endpoint);
}
fn invalidate(state: &mut TickState, endpoint: &Endpoint) {
    let reservations = indexed_reservations(state, endpoint, "custody")
        .expect("validated native inventory records");
    for mut reservation in reservations {
        for quantity in &mut reservation.snapshot.quantities {
            quantity.returned += quantity.remaining;
            quantity.remaining = 0;
        }
        reservation.snapshot.released = true;
        reservation.snapshot.bound_to = None;
        reservation.revision = next(reservation.revision).expect("reservation revision capacity");
        save_reservation(state, reservation).expect("validated reservation retirement");
    }
}

pub(crate) fn matching_reservation(
    state: &TickState,
    owner: &str,
    plan_hash: &str,
    binding: &str,
) -> Result<String, Failure> {
    metadata(state)?;
    let prefix = format!("plan/{owner}/{plan_hash}/");
    for (key, image) in state.plugins.domain_prefix(OWNER, &prefix) {
        if image != &serde_json::Value::Bool(true) {
            return Err(Failure::RuntimeUnavailable);
        }
        let value = stored_reservation(state, &key[prefix.len()..])?;
        if value.owner != owner
            || value.snapshot.resource_plan_hash != plan_hash
            || value.snapshot.released
            || !protected(&value)
        {
            return Err(Failure::RuntimeUnavailable);
        }
        if value
            .snapshot
            .bound_to
            .as_deref()
            .is_none_or(|id| id == binding)
        {
            return Ok(value.snapshot.reservation_ref);
        }
    }
    Err(Failure::NotFound)
}
pub(crate) fn native_npc_inventory_revision(
    state: &TickState,
    _plugin_id: &str,
    handle: &str,
) -> Result<u64, Failure> {
    metadata(state)?;
    let carry = Endpoint::NpcCarry {
        handle: handle.to_owned(),
    };
    let equipment = Endpoint::NpcEquipment {
        handle: handle.to_owned(),
    };
    let mut revision = 1;
    for endpoint in [carry, equipment] {
        if let Some(image) = image_version(state, &endpoint_key(state, &endpoint))? {
            revision = revision.max(image.revision);
        }
    }
    Ok(revision)
}
pub(crate) fn validate(state: &TickState) -> Result<(), crate::config::CoreError> {
    let malformed =
        || crate::config::CoreError::Recovery("invalid native plugin inventory records".into());
    let metadata = metadata(state).map_err(|_| malformed())?;
    let mut reservations = BTreeMap::new();
    let mut actual_indexes = BTreeSet::new();
    let mut retired = BTreeMap::new();
    for (key, value) in state.plugins.domain_entries(OWNER) {
        if key == KEY {
            continue;
        }
        if let Some(endpoint) = key.strip_prefix("image/") {
            if endpoint.is_empty() {
                return Err(malformed());
            }
            let version = ImageVersion::deserialize(value).map_err(|_| malformed())?;
            ScriptInventoryFence::try_new(version.revision, version.hash)
                .map_err(|_| malformed())?;
            if version.revision == 0 {
                return Err(malformed());
            }
        } else if let Some(reference) = key.strip_prefix("reservation/") {
            reservations.insert(
                reference,
                Reservation::deserialize(value).map_err(|_| malformed())?,
            );
        } else if key.starts_with("source/")
            || key.starts_with("custody/")
            || key.starts_with("plan/")
        {
            if value != &serde_json::Value::Bool(true) {
                return Err(malformed());
            }
            actual_indexes.insert(key.to_owned());
        } else if let Some(slot) = key.strip_prefix("retired/") {
            let index = slot.parse::<usize>().map_err(|_| malformed())?;
            if index >= MAX_TERMINAL_RESERVATIONS || index.to_string() != slot {
                return Err(malformed());
            }
            retired.insert(index, value.as_str().ok_or_else(malformed)?);
        } else {
            // In particular, the old monolithic "ledger" schema is not an empty image.
            return Err(malformed());
        }
    }
    let expected_indexes: BTreeSet<_> = reservations.values().flat_map(index_keys).collect();
    let terminal: BTreeSet<_> = reservations
        .iter()
        .filter(|(_, reservation)| !protected(reservation))
        .map(|(reference, _)| *reference)
        .collect();
    let retired_refs: BTreeSet<_> = retired.values().copied().collect();
    if actual_indexes != expected_indexes
        || reservations
            .values()
            .filter(|reservation| protected(reservation))
            .count()
            != metadata.protected_count
        || retired.len() != metadata.retired_count
        || retired_refs.len() != retired.len()
        || terminal != retired_refs
        || (metadata.retired_count < MAX_TERMINAL_RESERVATIONS
            && retired.keys().copied().ne(0..metadata.retired_count))
    {
        return Err(malformed());
    }
    for (reference, reservation) in &reservations {
        if *reference != reservation.snapshot.reservation_ref
            || reservation.owner.is_empty()
            || reservation.revision == 0
            || reservation.revision > MAX_SCRIPT_WORLD_TIME
            || (reservation.snapshot.released && reservation.snapshot.bound_to.is_some())
        {
            return Err(malformed());
        }
        ScriptOperationOutcome::committed(
            reservation.revision,
            ScriptOperationPayload::OwnedInventory {
                result: Box::new(ScriptOwnedInventoryResult::Reservation {
                    reservation: reservation.snapshot.clone(),
                }),
            },
        )
        .map_err(|_| malformed())?;
        match &reservation.snapshot.endpoint {
            Endpoint::PlayerInventory { .. } => {
                if !reservation.source_cells.is_empty() {
                    return Err(malformed());
                }
                let uuid = reservation.player_uuid.ok_or_else(malformed)?;
                let live = std::iter::once(state)
                    .chain(state.other_dimensions.values().map(Box::as_ref))
                    .find_map(|target| {
                        target
                            .connections
                            .all_connections()
                            .find(|connection| {
                                target.connections.uuid_of(*connection) == Some(uuid)
                            })
                            .and_then(|connection| {
                                target
                                    .connections
                                    .entity_of(connection)
                                    .and_then(|entity| target.players.get(&entity))
                            })
                    });
                let offline = std::iter::once(state)
                    .chain(state.other_dimensions.values().map(Box::as_ref))
                    .find_map(|target| target.offline_players.get(&uuid));
                if active(reservation) {
                    let slots = live
                        .map(|player| &player.inventory.slots)
                        .or_else(|| offline.map(|player| &player.inventory));
                    if let Some(slots) = slots {
                        let candidate = &slots[..slots.len().min(36)];
                        let mut quantities = BTreeMap::new();
                        for held in reservations
                            .values()
                            .filter(|r| r.player_uuid == reservation.player_uuid && active(r))
                        {
                            for q in &held.snapshot.quantities {
                                let quantity =
                                    quantities.entry(q.resource_id.clone()).or_insert(0_u64);
                                *quantity = quantity
                                    .checked_add(q.remaining)
                                    .filter(|n| *n <= MAX_SCRIPT_WORLD_TIME)
                                    .ok_or_else(malformed)?;
                            }
                        }
                        for (resource, remaining) in quantities {
                            let id = Identifier::parse(&resource)
                                .ok()
                                .and_then(|id| state.registry.items.id_of(&id))
                                .ok_or_else(malformed)?;
                            let count: u64 = candidate
                                .iter()
                                .filter(|s| s.item_id == id && !s.is_empty())
                                .map(|s| s.count as u64)
                                .sum();
                            if count < remaining {
                                return Err(malformed());
                            }
                        }
                    } else {
                        return Err(malformed());
                    }
                }
            }
            Endpoint::Container { .. } => {
                if reservation.player_uuid.is_some() {
                    return Err(malformed());
                }
                if reservation.source_cells.is_empty() || reservation.source_cells.len() > 2 {
                    return Err(malformed());
                }
                let mut cells = BTreeSet::new();
                for cell in &reservation.source_cells {
                    ScriptBlockPosition::new(cell[0], cell[1], cell[2])
                        .validate()
                        .map_err(|_| malformed())?;
                    if !cells.insert(*cell) {
                        return Err(malformed());
                    }
                }
                match load(state, &reservation.owner, &reservation.snapshot.endpoint) {
                    Ok(image) if active(reservation) => {
                        check_floor(state, &reservation.snapshot.endpoint, &image, None)
                            .map_err(|_| malformed())?
                    }
                    Ok(_) | Err(Failure::Unloaded) => {}
                    Err(_) if !active(reservation) => {}
                    Err(_) => return Err(malformed()),
                }
            }
            _ => return Err(malformed()),
        }
    }
    Ok(())
}

pub(crate) fn can_synchronize_entity_custody(
    state: &TickState,
    slot: crate::entities::EntitySlot,
) -> bool {
    let Some((_, handle)) = crate::plugin_npcs::custody_for_entity(state, slot) else {
        return true;
    };
    let Ok(_) = metadata(state) else {
        return false;
    };
    let equipment = Endpoint::NpcEquipment {
        handle: handle.clone(),
    };
    let carry = Endpoint::NpcCarry { handle };
    [equipment, carry].iter().all(|endpoint| {
        image_version(state, &endpoint_key(state, endpoint))
            .is_ok_and(|image| image.is_none_or(|image| image.revision < u64::MAX))
    })
}

pub(crate) fn synchronize_entity_custody(
    state: &mut TickState,
    slot: crate::entities::EntitySlot,
) -> Result<(), Failure> {
    let Some((_, handle)) = crate::plugin_npcs::custody_for_entity(state, slot) else {
        return Ok(());
    };
    let row = state.entities.row(slot);
    let empty = ItemStack::EMPTY;
    let equipment: BTreeMap<_, _> = [0, 1, 5, 4, 3, 2]
        .iter()
        .enumerate()
        .map(|(script, native)| {
            (
                script as u8,
                row.appearance.equipment[*native].as_ref().unwrap_or(&empty),
            )
        })
        .collect();
    let mut images = vec![(
        Endpoint::NpcEquipment {
            handle: handle.clone(),
        },
        hash(&(row.uuid.to_string(), &equipment)),
    )];
    if let Some(villager) = &row.villager {
        let carry: BTreeMap<_, _> = villager
            .inventory
            .iter()
            .enumerate()
            .map(|(slot, stack)| (slot as u8, stack.as_ref().unwrap_or(&empty)))
            .collect();
        images.push((
            Endpoint::NpcCarry { handle },
            hash(&(row.uuid.to_string(), &carry)),
        ));
    }
    metadata(state)?;
    let mut updates = Vec::with_capacity(images.len());
    for (endpoint, hash) in images {
        let key = endpoint_key(state, &endpoint);
        let revision = match image_version(state, &key)? {
            Some(before) if before.hash == hash => continue,
            Some(before) => next(before.revision)?,
            None => 1,
        };
        updates.push((endpoint, hash, revision));
    }
    for (endpoint, hash, revision) in updates {
        remember(state, &endpoint, &hash, revision)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "plugin_inventories_tests.rs"]
mod tests;
