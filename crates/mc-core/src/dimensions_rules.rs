//! Dimension-owned bed/anchor interactions and safe charged-anchor respawns.
use crate::boundary::{BlockPos, EntityId, Hand};
use crate::dimensions::Dimension;
use crate::intent::{Footprint, Intent, IntentKind, InvClaim};
use crate::tick::TickState;

pub(crate) fn use_block(
    state: &mut TickState,
    actor: EntityId,
    pos: BlockPos,
    hand: Hand,
    intents: &mut Vec<Intent>,
) -> bool {
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(current)) else {
        return false;
    };
    let bed = block.block.id.path().ends_with("_bed");
    let anchor = block.block.id.path() == "respawn_anchor";
    if (!bed || state.dimension.beds_work()) && !anchor {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    if player.spectator() {
        return true;
    }
    let mut footprint = Footprint {
        entity_reads: vec![actor],
        block_expect: vec![(pos, current)],
        ..Default::default()
    };
    let kind = if bed {
        let Some((other, _)) = crate::placement::other_half(&state.registry.blocks, pos, current)
        else {
            return true;
        };
        let Some(other_state) = state.world.block_at(other.x, other.y, other.z) else {
            return true;
        };
        footprint.block_write.push((pos, state.registry.states.air));
        if state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(other_state))
            .is_some_and(|other| other.block.id == block.block.id)
        {
            footprint.block_expect.push((other, other_state));
            footprint
                .block_write
                .push((other, state.registry.states.air));
        }
        IntentKind::EnvironmentalExplosion {
            center: [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ],
            power: 5.0,
        }
    } else {
        let Some(charges) = anchor_charge(state, current) else {
            return true;
        };
        // Vanilla hand arbitration: a non-fuel main hand defers to an
        // offhand glowstone charge (main-hand PASS), before any SetRespawn
        // or overworld explosion decision.
        let main_slot = crate::tick::held_slot_index(player, hand);
        let main_held = &player.inventory.slots[main_slot];
        let main_glowstone = !main_held.is_empty()
            && state
                .registry
                .items
                .name_of(main_held.item_id)
                .is_some_and(|item| item.path() == "glowstone");
        let other_hand = match hand {
            Hand::Main => Hand::Off,
            Hand::Off => Hand::Main,
        };
        let other_slot = crate::tick::held_slot_index(player, other_hand);
        let other_held = &player.inventory.slots[other_slot];
        let other_glowstone = !other_held.is_empty()
            && state
                .registry
                .items
                .name_of(other_held.item_id)
                .is_some_and(|item| item.path() == "glowstone");
        let (charge_slot, charge_hand) = if main_glowstone {
            (main_slot, hand)
        } else if other_glowstone {
            (other_slot, other_hand)
        } else {
            (usize::MAX, hand)
        };
        let glowstone = charge_slot != usize::MAX;
        if glowstone && charges < 4 {
            let charge_held = &player.inventory.slots[charge_slot];
            let Some(next) = anchor_state(state, current, charges + 1) else {
                return true;
            };
            footprint.block_write.push((pos, next));
            if !player.creative() {
                footprint.inv.push(InvClaim {
                    owner: actor,
                    slot: charge_slot as u16,
                    expect_item: charge_held.item_id,
                    expect_count: charge_held.count,
                    take: 1,
                    put_item: None,
                    put_count: 0,
                });
            }
            IntentKind::UseBlock {
                pos,
                hand: Some(charge_hand),
            }
        } else if charges == 0 {
            return true;
        } else if state.dimension == Dimension::Nether {
            IntentKind::SetRespawn { pos }
        } else {
            footprint.block_write.push((pos, state.registry.states.air));
            IntentKind::EnvironmentalExplosion {
                center: [
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                ],
                power: 5.0,
            }
        }
    };
    intents.push(Intent {
        actor,
        seq: state.next_seq(actor),
        kind,
        footprint,
    });
    true
}

pub(crate) fn anchor_charge(state: &TickState, current: u32) -> Option<u8> {
    let entry = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))?;
    if entry.block.id.path() != "respawn_anchor" {
        return None;
    }
    entry
        .properties
        .iter()
        .find(|(key, _)| key == "charges")?
        .1
        .parse()
        .ok()
}

pub(crate) fn anchor_state(state: &TickState, current: u32, charges: u8) -> Option<u32> {
    let entry = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))?;
    let mut properties = entry.properties.clone();
    properties.iter_mut().find(|(key, _)| key == "charges")?.1 = charges.to_string();
    state
        .registry
        .blocks
        .by_name_and_props(&entry.block.id, &properties)
        .map(|id| id.0)
}

pub(crate) fn anchor_spawn(state: &TickState, pos: BlockPos) -> Option<[f64; 3]> {
    if state.dimension != Dimension::Nether
        || anchor_charge(state, state.world.block_at(pos.x, pos.y, pos.z)?)? == 0
    {
        return None;
    }
    // Vanilla fits the 1.8-high player AABB above a solid floor instead of
    // requiring whole feet/head cells to classify Air: carpet/snow layers
    // do not invalidate the spawn; only an intersecting solid shape does.
    // The center-top column (vanilla RESPAWN_OFFSETS final (0,1,0)) is the
    // last fallback, not omitted.
    let clear = |cell: BlockPos| {
        let Some(id) = state.world.block_at(cell.x, cell.y, cell.z) else {
            return false;
        };
        if state.registry.materials.classify(id) == mc_physics::BlockMaterial::Air {
            return true;
        }
        if state.registry.materials.classify(id) == mc_physics::BlockMaterial::Solid {
            return false;
        }
        // Non-air, non-solid (carpets, snow layers, fluids): blocked only
        // when the actual collision shape intersects the player AABB.
        let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
            return false;
        };
        !mc_data::collision_shapes::vanilla_collision_shapes()
            .get_for_state(id, &block.block.id, &block.properties)
            .is_some_and(|shape| {
                shape.iter().any(|collision_box| {
                    let [min_x, min_y, min_z, max_x, max_y, max_z] =
                        collision_box.coordinates();
                    let units = mc_data::collision_shapes::COLLISION_UNITS_PER_BLOCK as f64;
                    // Player AABB at feet (cell.x+0.1..0.9, cell.y..+1.8).
                    let feet = f64::from(cell.y);
                    let top = f64::from(max_y) / units;
                    let _ = (min_x, min_z, max_x, max_z);
                    top > 0.0625 && f64::from(min_y) / units < feet + 1.8 - f64::from(cell.y)
                })
            })
    };
    for dy in [0, 1, -1] {
        // All eight neighbors first, then the center-top column (vanilla
        // RESPAWN_OFFSETS final (0,1,0) fallback).
        for [dx, dz] in [
            [1, 0],
            [-1, 0],
            [0, 1],
            [0, -1],
            [1, 1],
            [-1, 1],
            [1, -1],
            [-1, -1],
            [0, 0],
        ] {
            let feet = BlockPos {
                x: pos.x + dx,
                y: pos.y + dy,
                z: pos.z + dz,
            };
            if clear(feet)
                && clear(BlockPos {
                    y: feet.y + 1,
                    ..feet
                })
                && state
                    .world
                    .block_at(feet.x, feet.y - 1, feet.z)
                    .is_some_and(|id| {
                        state.registry.materials.classify(id) == mc_physics::BlockMaterial::Solid
                    })
            {
                return Some([
                    f64::from(feet.x) + 0.5,
                    f64::from(feet.y),
                    f64::from(feet.z) + 0.5,
                ]);
            }
        }
    }
    None
}

pub(crate) fn void_intents(state: &mut TickState, intents: &mut Vec<Intent>) {
    if !state.tick.is_multiple_of(10) {
        return;
    }
    let threshold = f64::from(state.dimension.geometry().min_y() - 64);
    for index in 0..state.entities.len() {
        let slot = crate::entities::EntitySlot(index as u32);
        let body = state.entities.body(slot);
        if !body.alive || state.entities.pos(slot)[1] >= threshold {
            continue;
        }
        let actor = body.id;
        intents.push(Intent {
            actor,
            seq: state.next_seq(actor),
            kind: IntentKind::DamageEntity {
                target: actor,
                amount: 4.0,
                kind: crate::survival::DamageKind::Void,
                source: None,
            },
            footprint: Footprint {
                entity_writes: vec![actor],
                ..Default::default()
            },
        });
    }
}
