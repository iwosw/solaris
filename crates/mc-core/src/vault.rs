//! Per-player vault key admission and persisted reward ejection.
use crate::boundary::{BlockPos, ChangeSet, EntityId, Hand};
use crate::tick::{TickState, spawn_item_entity};
use crate::trial_spawner::{block, key, nearby, position, property, set_properties};
use mc_data::ItemStack;
use mc_data::item_nbt::{field, stack_to_fields};
use mc_data::trial_nbt::{self as nbt, Fields, VaultData};
use mc_nbt::Tag;
use std::collections::BTreeMap;

pub(crate) type Vaults = BTreeMap<(i32, i32, i32), VaultData>;
fn persist(state: &mut TickState, pos: BlockPos, vault: &VaultData) {
    let fields = vault.to_fields(&state.registry.items);
    let wire = Tag::Compound(vec![(
        "shared_data".into(),
        field(&fields, "shared_data")
            .cloned()
            .unwrap_or_else(|| Tag::Compound(Vec::new())),
    )]);
    let kind = mc_data::block_entity_kinds::BlockEntityKind::Vault;
    crate::trial_spawner::persist(
        state,
        pos,
        kind.vanilla_id(),
        fields,
        wire,
        kind.protocol_id(),
    );
}
pub(crate) fn decode_chunk(
    vaults: &Vaults,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) -> Result<crate::tick::CellRecords<VaultData>, String> {
    let mut decoded = Vec::new();
    for (pos, bytes) in &chunk.block_entities {
        if vaults.contains_key(&key(*pos)) {
            continue;
        }
        if chunk
            .get_block(
                pos.x.rem_euclid(16) as u8,
                pos.y,
                pos.z.rem_euclid(16) as u8,
            )
            .and_then(|id| registry.blocks.by_id(id))
            .is_some_and(|block| block.block.id.path() == "vault")
        {
            let vault = nbt::decode(bytes)
                .and_then(|fields| VaultData::from_fields(fields, &registry.items))
                .map_err(|error| format!("vault at ({},{},{}): {error}", pos.x, pos.y, pos.z))?;
            decoded.push((key(*pos), vault));
        }
    }
    Ok(decoded)
}
pub(crate) fn after_blocks(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    for &(pos, _) in writes {
        if block(state, pos, "vault").is_some() && !state.vaults.contains_key(&key(pos)) {
            let vault = VaultData::default();
            persist(state, pos, &vault);
            state.vaults.insert(key(pos), vault);
        }
    }
}
fn config(vault: &VaultData) -> Option<&Fields> {
    nbt::compound(&vault.fields, "config").expect("validated config")
}
fn range(vault: &VaultData, name: &str, default: f64) -> f64 {
    config(vault)
        .and_then(|fields| field(fields, name))
        .and_then(|value| {
            if let Tag::Double(value) = value {
                Some(*value)
            } else {
                None
            }
        })
        .unwrap_or(default)
}
fn table(vault: &VaultData, ominous: bool) -> &str {
    config(vault)
        .and_then(|fields| field(fields, "loot_table"))
        .and_then(|value| {
            if let Tag::String(value) = value {
                Some(value.as_str())
            } else {
                None
            }
        })
        .unwrap_or(if ominous {
            "minecraft:chests/trial_chambers/reward_ominous"
        } else {
            "minecraft:chests/trial_chambers/reward"
        })
}
fn shared(
    vault: &mut VaultData,
    players: std::collections::BTreeSet<uuid::Uuid>,
    items: &mc_data::items::ItemRegistry,
) -> bool {
    let mut fields = nbt::compound(&vault.fields, "shared_data")
        .expect("validated shared data")
        .cloned()
        .unwrap_or_default();
    nbt::set(&mut fields, "connected_players", nbt::uuid_list(&players));
    nbt::set(
        &mut fields,
        "connected_particles_range",
        Tag::Double(range(vault, "deactivation_range", 4.5)),
    );
    fields.retain(|(name, _)| name != "display_item");
    if let Some(item) = vault.items.first() {
        fields.push((
            "display_item".into(),
            Tag::Compound(stack_to_fields(items, item).expect("registered vault item")),
        ));
    }
    let next = Tag::Compound(fields);
    if field(&vault.fields, "shared_data") == Some(&next) {
        return false;
    }
    nbt::set(&mut vault.fields, "shared_data", next);
    true
}
/// Returns true for all vault interactions; refused keys never fall through to placement.
pub(crate) fn use_key(
    state: &mut TickState,
    entity: EntityId,
    hand: Hand,
    pos: BlockPos,
    held: &ItemStack,
    changes: &mut ChangeSet,
) -> bool {
    if block(state, pos, "vault").is_none() {
        return false;
    }
    let phase = property(state, pos, "vault_state").unwrap_or("inactive");
    if matches!(phase, "unlocking" | "ejecting") {
        return true;
    }
    let Some(slot) = state.entities.slot_of(entity) else {
        return true;
    };
    let uuid = state.entities.row(slot).uuid;
    let ominous = property(state, pos, "ominous") == Some("true");
    let mut vault = state.vaults.remove(&key(pos)).unwrap_or_default();
    let key_name = if ominous {
        "ominous_trial_key"
    } else {
        "trial_key"
    };
    let expected = config(&vault)
        .and_then(|fields| nbt::compound(fields, "key_item").ok().flatten())
        .and_then(|fields| field(fields, "id"))
        .and_then(|value| {
            if let Tag::String(name) = value {
                Some(name.as_str())
            } else {
                None
            }
        });
    let valid = state
        .registry
        .items
        .name_of(held.item_id)
        .is_some_and(|name| {
            expected.map_or(name.path() == key_name, |expected| {
                name.as_str() == expected
            })
        });
    if !valid || held.is_empty() || vault.rewarded.contains(&uuid) {
        state.vaults.insert(key(pos), vault);
        return true;
    }
    if crate::tick::held_debit_below_floor(state, entity, hand, 1) {
        state.vaults.insert(key(pos), vault);
        return true;
    }
    let seed = state.tick
        ^ uuid.as_u128() as u64
        ^ (pos.x as u64).rotate_left(19)
        ^ (pos.z as u64).rotate_left(37);
    let rewards = state
        .registry
        .trial_loot
        .roll(
            table(&vault, ominous),
            seed,
            &state.registry.items,
            &state.registry.item_facts,
        )
        .expect("validated vault loot");
    let Some(player) = state.players.get_mut(&entity) else {
        state.vaults.insert(key(pos), vault);
        return true;
    };
    let index = crate::tick::held_slot_index(player, hand);
    if !player.creative() {
        let slot = &mut player.inventory.slots[index];
        slot.count -= 1;
        if slot.count <= 0 {
            *slot = ItemStack::EMPTY;
        }
    }
    let contents = player.inventory_contents();
    let connection = player.connection;
    state.push_player(connection, contents);
    vault.rewarded.insert(uuid);
    vault.items = rewards;
    vault.total_ejections = vault.items.len() as i32;
    vault.resumes = state.tick + 20;
    shared(
        &mut vault,
        std::collections::BTreeSet::new(),
        &state.registry.items,
    );
    let mut writes = Vec::new();
    set_properties(state, pos, &[("vault_state", "unlocking")], &mut writes);
    changes.blocks.extend(
        writes
            .into_iter()
            .map(|(position, state)| crate::boundary::BlockChange { position, state }),
    );
    persist(state, pos, &vault);
    state.vaults.insert(key(pos), vault);
    crate::advancements::accepted_block(
        state,
        entity,
        "minecraft:item_used_on_block",
        pos,
        None,
        Some(held.clone()),
    );
    true
}
pub(crate) fn run(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    changes: &mut ChangeSet,
) {
    let mut vaults = std::mem::take(&mut state.vaults);
    vaults.retain(|key, vault| {
        let pos = position(*key);
        if !state
            .world
            .is_block_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
        {
            return true;
        }
        if block(state, pos, "vault").is_none() {
            state.world.set_opaque_block_entity(pos, None);
            state.opaque_changes.push((pos, None));
            return false;
        }
        if state.tick < vault.resumes {
            return true;
        }
        let previous = (vault.items.len(), vault.resumes, vault.total_ejections);
        let phase = match property(state, pos, "vault_state") {
            Some("active") => "active",
            Some("unlocking") => "unlocking",
            Some("ejecting") => "ejecting",
            _ => "inactive",
        };
        if matches!(phase, "inactive" | "active") && !state.tick.is_multiple_of(20) {
            return true;
        }
        let mut shared_changed = false;
        let mut next = phase;
        if state.tick >= vault.resumes {
            if phase == "unlocking" {
                next = "ejecting";
                vault.resumes = state.tick + 20;
            } else if phase == "ejecting" {
                if !vault.items.is_empty() {
                    let item = vault.items.remove(0);
                    changes.entities_spawned.push(spawn_item_entity(
                        state,
                        [pos.x as f64 + 0.5, pos.y as f64 + 1.5, pos.z as f64 + 0.5],
                        item,
                    ));
                    vault.resumes = state.tick + 20;
                } else {
                    next = "inactive";
                    vault.total_ejections = 0;
                }
            }
            if !matches!(next, "unlocking" | "ejecting") {
                let range = range(
                    vault,
                    if phase == "active" {
                        "deactivation_range"
                    } else {
                        "activation_range"
                    },
                    if phase == "active" { 4.5 } else { 4.0 },
                );
                let players = nearby(state, pos, range, true)
                    .into_iter()
                    .filter_map(|id| {
                        state
                            .entities
                            .slot_of(id)
                            .map(|slot| state.entities.row(slot).uuid)
                    })
                    .filter(|uuid| !vault.rewarded.contains(uuid))
                    .collect::<std::collections::BTreeSet<_>>();
                next = if players.is_empty() {
                    "inactive"
                } else {
                    "active"
                };
                shared_changed = shared(vault, players, &state.registry.items);
            } else {
                shared_changed = shared(
                    vault,
                    std::collections::BTreeSet::new(),
                    &state.registry.items,
                );
            }
        }
        set_properties(state, pos, &[("vault_state", next)], writes);
        if (vault.items.len(), vault.resumes, vault.total_ejections) != previous
            || shared_changed
            || phase != next
        {
            persist(state, pos, vault);
        }
        true
    });
    state.vaults = vaults;
}

#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;
