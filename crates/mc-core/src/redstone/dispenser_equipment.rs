//! Equipment dispensing uses the maintained chunk membership, not a population scan.
use crate::boundary::{BlockPos, ChunkPos};
use crate::entities::{EntityKind, EntitySlot};
use crate::tick::TickState;
use mc_data::ItemStack;
use std::sync::Arc;

pub(super) fn equip(
    state: &mut TickState,
    target: BlockPos,
    stack: &ItemStack,
) -> Result<bool, ()> {
    let Some(facts) = state
        .registry
        .item_facts
        .facts_for_stack(stack, &state.registry.items)
    else {
        return Ok(false);
    };
    if !facts.equippable_dispensable.unwrap_or(true) {
        return Ok(false);
    }
    let index = match facts.equippable_slot.as_deref() {
        Some("feet") => 2,
        Some("legs") => 3,
        Some("chest") => 4,
        Some("head") => 5,
        _ => return Ok(false),
    };
    let min = [
        f64::from(target.x),
        f64::from(target.y),
        f64::from(target.z),
    ];
    let max = min.map(|v| v + 1.0);
    let extent = state.entities.max_half_width();
    let chunk = |coordinate: f64| (coordinate.floor() as i32).div_euclid(16);
    let mut selected: Option<EntitySlot> = None;
    for z in chunk(min[2] - extent)..=chunk(max[2] + extent) {
        for x in chunk(min[0] - extent)..=chunk(max[0] + extent) {
            state
                .entities
                .for_each_slot_in_chunk(ChunkPos { x, z }, |slot| {
                    if selected.is_some_and(|previous| previous <= slot) {
                        return;
                    }
                    let body = state.entities.body(slot);
                    let p = state.entities.pos(slot);
                    if !body.alive
                        || p[0] + body.half_width <= min[0]
                        || p[0] - body.half_width >= max[0]
                        || p[1] + body.height <= min[1]
                        || p[1] >= max[1]
                        || p[2] + body.half_width <= min[2]
                        || p[2] - body.half_width >= max[2]
                    {
                        return;
                    }
                    let (name, type_id) = match body.kind {
                        EntityKind::Player { .. } => {
                            let Some(player) = state.players.get(&body.id) else {
                                return;
                            };
                            if player.spectator()
                                || player.vitals.is_dead()
                                || !player.inventory.slots[index + 34].is_empty()
                            {
                                return;
                            }
                            let Some(contract) =
                                mc_data::entity_types::entity_type_contract_26_1_2_by_name(
                                    "minecraft:player",
                                )
                            else {
                                return;
                            };
                            (contract.name, contract.protocol_id)
                        }
                        EntityKind::Mob { type_id } => {
                            let row = state.entities.row(slot);
                            if row.health <= 0.0
                                || row.appearance.equipment[index]
                                    .as_ref()
                                    .is_some_and(|s| !s.is_empty())
                            {
                                return;
                            }
                            if crate::armor_stand::is_stand(state, slot) {
                                if !crate::armor_stand::can_dispense_equipment(state, slot, index) {
                                    return;
                                }
                            } else if !row.loot.can_pick_up_loot {
                                return;
                            }
                            let wire_type = crate::blueprint::wire_type_id(type_id);
                            let Some(contract) =
                                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(
                                    wire_type,
                                )
                            else {
                                return;
                            };
                            if contract.behavior.archetype
                                != mc_data::entity_types::EntityArchetype::Living
                            {
                                return;
                            }
                            (contract.name, wire_type)
                        }
                        _ => return,
                    };
                    if facts
                        .equippable_allowed_entities
                        .as_ref()
                        .is_some_and(|allowed| {
                            !allowed.iter().any(|entry| {
                                entry == name
                                    || entry.strip_prefix('#').is_some_and(|tag| {
                                        state.registry.tags.contains_raw_id(
                                            "minecraft:entity_type",
                                            tag,
                                            type_id as i32,
                                        )
                                    })
                            })
                        })
                    {
                        return;
                    }
                    selected = Some(slot);
                });
        }
    }
    let Some(slot) = selected else {
        return Ok(false);
    };
    let id = state.entities.body(slot).id;
    let mut equipped = stack.clone();
    equipped.count = 1;
    if let Some(player) = state.players.get_mut(&id) {
        player.inventory.slots[index + 34] = equipped;
        player.inventory_menu.state_id = (player.inventory_menu.state_id + 1) & 0x7fff;
        let change = player.inventory_contents();
        let connection = player.connection;
        state.push_player(connection, change);
    } else {
        if !crate::plugin_inventories::can_synchronize_entity_custody(state, slot) {
            crate::plugin::fail_native(state);
            return Err(());
        }
        let stand = crate::armor_stand::is_stand(state, slot);
        let row = state.entities.row_mut(slot);
        Arc::make_mut(&mut row.appearance).equipment[index] = Some(equipped);
        if !stand {
            row.loot.drop_chances[index] = 2.0;
            row.persistent = true;
        }
        crate::plugin_inventories::synchronize_entity_custody(state, slot)
            .expect("preflighted dispenser equipment custody");
        state.appearance_changed.push(id);
        crate::entity_loot::restore(state, slot);
    }
    Ok(true)
}
