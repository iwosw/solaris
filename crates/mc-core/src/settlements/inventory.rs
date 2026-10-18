//! Settlement inventory plans use the canonical native inventory fence ledger.
//! This is a child of `plugin_inventories` so containers/players share its
//! physical-cell identity, reservations and version authority, not a second KV
//! inventory. Resident carry/equipment resolve exact native UUIDs, never handles.
use std::collections::{BTreeMap, BTreeSet};

use mc_data::ItemStack;
use mc_script::{ScriptInventoryFence, ScriptOperationFailure as Failure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tick::TickState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Endpoint {
    VillagerCarry {
        uuid: Uuid,
    },
    VillagerEquipment {
        uuid: Uuid,
    },
    Container {
        dimension: String,
        position: [i32; 3],
    },
    Player {
        uuid: Uuid,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Image {
    pub endpoint: Endpoint,
    pub slots: Vec<ItemStack>,
    pub fence: ScriptInventoryFence,
}

pub(crate) struct Change {
    pub before: Image,
    pub after: Vec<ItemStack>,
}

fn canonical(state: &TickState, endpoint: &Endpoint) -> Result<Option<super::Endpoint>, Failure> {
    Ok(match endpoint {
        Endpoint::Container {
            dimension,
            position,
        } => Some(super::Endpoint::Container {
            dimension: dimension.clone(),
            position: mc_script::ScriptBlockPosition::new(position[0], position[1], position[2]),
        }),
        Endpoint::Player { uuid } => {
            let connection = std::iter::once(state)
                .chain(state.other_dimensions.values().map(Box::as_ref))
                .find_map(|world| {
                    let slot = world.entities.slot_of_uuid(*uuid)?;
                    world
                        .connections
                        .entity_connection(world.entities.body(slot).id)
                })
                .ok_or(Failure::NotFound)?;
            Some(super::Endpoint::PlayerInventory {
                player_id: connection.0,
            })
        }
        Endpoint::VillagerCarry { .. } | Endpoint::VillagerEquipment { .. } => None,
    })
}

fn resident_key(endpoint: &Endpoint) -> String {
    match endpoint {
        Endpoint::VillagerCarry { uuid } => format!("villager-carry:{uuid}"),
        Endpoint::VillagerEquipment { uuid } => format!("villager-equipment:{uuid}"),
        _ => unreachable!("native resident endpoint"),
    }
}

fn resident(
    state: &TickState,
    endpoint: &Endpoint,
) -> Result<crate::entities::EntitySlot, Failure> {
    let uuid = match endpoint {
        Endpoint::VillagerCarry { uuid } | Endpoint::VillagerEquipment { uuid } => *uuid,
        _ => return Err(Failure::InvalidRequest),
    };
    let slot = state.entities.slot_of_uuid(uuid).ok_or(Failure::NotFound)?;
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    if !body.alive
        || row
            .villager
            .as_ref()
            .is_none_or(|v| v.wandering || v.conversion_time.is_some())
    {
        return Err(Failure::NotFound);
    }
    Ok(slot)
}

fn resident_slots(state: &TickState, endpoint: &Endpoint) -> Result<Vec<ItemStack>, Failure> {
    let row = state.entities.row(resident(state, endpoint)?);
    Ok(match endpoint {
        Endpoint::VillagerCarry { .. } => row
            .villager
            .as_ref()
            .expect("resident")
            .inventory
            .iter()
            .map(|stack| stack.clone().unwrap_or(ItemStack::EMPTY))
            .collect(),
        Endpoint::VillagerEquipment { .. } => [0, 1, 5, 4, 3, 2]
            .into_iter()
            .map(|index| {
                row.appearance.equipment[index]
                    .clone()
                    .unwrap_or(ItemStack::EMPTY)
            })
            .collect(),
        _ => unreachable!("native resident endpoint"),
    })
}

fn resident_hash(endpoint: &Endpoint, slots: &[ItemStack]) -> String {
    let uuid = match endpoint {
        Endpoint::VillagerCarry { uuid } | Endpoint::VillagerEquipment { uuid } => uuid,
        _ => unreachable!("native resident endpoint"),
    };
    super::hash(&(uuid, slots))
}

pub(crate) fn load(state: &TickState, endpoint: &Endpoint) -> Result<Image, Failure> {
    if let Some(canonical) = canonical(state, endpoint)? {
        let image = super::load(state, "", &canonical)?;
        return Ok(Image {
            endpoint: endpoint.clone(),
            slots: image.slots.into_values().collect(),
            fence: image.fence,
        });
    }
    let slots = resident_slots(state, endpoint)?;
    let snapshot_hash = resident_hash(endpoint, &slots);
    super::metadata(state)?;
    let revision = match super::image_version(state, &resident_key(endpoint))? {
        Some(version) if version.hash == snapshot_hash => version.revision,
        Some(version) => super::next(version.revision)?,
        None => 1,
    };
    Ok(Image {
        endpoint: endpoint.clone(),
        slots,
        fence: ScriptInventoryFence::try_new(revision, snapshot_hash)
            .map_err(|_| Failure::InvalidRequest)?,
    })
}

fn check_stack(
    state: &TickState,
    endpoint: &Endpoint,
    canonical: Option<&super::Endpoint>,
    index: u8,
    before: &ItemStack,
    after: &ItemStack,
) -> Result<(), Failure> {
    if after == &ItemStack::EMPTY
        || (mc_data::inventory_semantics_26_1_2::can_stack(before, after)
            && after.count > 0
            && after.count <= before.count)
    {
        return Ok(());
    }
    if after.is_empty() || state.registry.items.name_of(after.item_id).is_none() {
        return Err(Failure::InvalidRequest);
    }
    let maximum = if let Some(canonical) = canonical {
        super::room_for_slot(state, canonical, index, &ItemStack::EMPTY, after) as i32
    } else if matches!(endpoint, Endpoint::VillagerEquipment { .. }) && index >= 2 {
        let required = ["head", "chest", "legs", "feet"][usize::from(index - 2)];
        if state
            .registry
            .item_facts
            .facts_for_stack(after, &state.registry.items)
            .and_then(|facts| facts.equippable_slot.as_deref())
            != Some(required)
        {
            return Err(Failure::InvalidRequest);
        }
        1
    } else {
        state.registry.item_rules().max_stack(after).min(99)
    };
    if after.count > maximum {
        return Err(Failure::Capacity);
    }
    Ok(())
}

/// Every source, fence, slot constraint and reservation is checked before any
/// native mutation. The main-thread caller journals policy + stock in one tick.
pub(crate) fn commit(
    state: &mut TickState,
    changes: Vec<Change>,
) -> Result<Vec<ScriptInventoryFence>, Failure> {
    let metadata = super::metadata(state)?;
    let mut physical = BTreeSet::new();
    let mut canonical_images = BTreeMap::new();
    let mut residents = Vec::new();
    let mut fences = Vec::with_capacity(changes.len());
    for change in changes {
        let current = load(state, &change.before.endpoint)?;
        if current.fence != change.before.fence || current.slots != change.before.slots {
            return Err(Failure::StaleRevision);
        }
        if current.slots.len() != change.after.len() {
            return Err(Failure::InvalidRequest);
        }
        let revision = super::next(current.fence.revision)?;
        let endpoint = change.before.endpoint;
        if let Some(canonical) = canonical(state, &endpoint)? {
            for key in super::image_keys(state, &canonical) {
                if !physical.insert(key) {
                    return Err(Failure::InvalidRequest);
                }
            }
            let mut image = super::load(state, "", &canonical)?;
            for ((index, before), after) in image.slots.iter_mut().zip(change.after) {
                check_stack(state, &endpoint, Some(&canonical), *index, before, &after)?;
                *before = after;
            }
            super::check_floor(state, &canonical, &image, None)?;
            // Canonical apply computes the final physical identity/hash after
            // writing. Return fences in the caller's original change order.
            fences.push((endpoint, revision, None));
            canonical_images.insert(canonical, image);
        } else {
            let key = resident_key(&endpoint);
            if !physical.insert(key.clone()) {
                return Err(Failure::InvalidRequest);
            }
            let slot = resident(state, &endpoint)?;
            for (index, (before, after)) in current.slots.iter().zip(&change.after).enumerate() {
                check_stack(state, &endpoint, None, index as u8, before, after)?;
            }
            let hash = resident_hash(&endpoint, &change.after);
            let fence = ScriptInventoryFence::try_new(revision, hash.clone())
                .map_err(|_| Failure::InvalidRequest)?;
            fences.push((endpoint.clone(), revision, Some(fence)));
            residents.push((endpoint, slot, key, hash, revision, change.after));
        }
    }
    // Unlike guest operations, native settlement authority is checked by the
    // settlement controller, not a made-up plugin manifest/ownership handle.
    super::apply(state, "", canonical_images)?;
    if state.plugins.get_domain(super::OWNER, super::KEY).is_none() && !residents.is_empty() {
        super::save_metadata(state, &metadata);
    }
    for (endpoint, slot, key, hash, revision, slots) in residents {
        let row = state.entities.row_mut(slot);
        match endpoint {
            Endpoint::VillagerCarry { .. } => {
                for (destination, stack) in row
                    .villager
                    .as_mut()
                    .expect("preflight resident")
                    .inventory
                    .iter_mut()
                    .zip(slots)
                {
                    *destination = (!stack.is_empty()).then_some(stack);
                }
            }
            Endpoint::VillagerEquipment { .. } => {
                let equipment = &mut std::sync::Arc::make_mut(&mut row.appearance).equipment;
                for (index, stack) in [0, 1, 5, 4, 3, 2].into_iter().zip(slots) {
                    equipment[index] = (!stack.is_empty()).then_some(stack);
                }
                state.appearance_changed.push(state.entities.body(slot).id);
            }
            _ => unreachable!("preflight resident"),
        }
        state.plugins.set_domain(
            super::OWNER,
            &format!("image/{key}"),
            serde_json::to_value(super::ImageVersion { hash, revision })
                .expect("native inventory version"),
        );
    }
    fences
        .into_iter()
        .map(|(endpoint, revision, fence)| {
            if let Some(fence) = fence {
                return Ok(fence);
            }
            let image = load(state, &endpoint)?;
            debug_assert_eq!(image.fence.revision, revision);
            Ok(image.fence)
        })
        .collect()
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
