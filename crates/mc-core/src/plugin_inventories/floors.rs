//! Reservation floor checks for blocks, players and intents.
//! Moved verbatim from `plugin_inventories.rs`.

use super::*;

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
