//! Enderman stare anger and safe, dry-ground teleportation. Anger is retained
//! by the existing special-mob NBT/checkpoint image; every mutation is journaled.
use crate::boundary::EntityId;
use crate::entities::{ColdRow, EntityKind, EntitySlot};
use crate::intent::DamageSource;
use crate::natural_spawn::Rng;
use crate::special_mobs::Retained;
use crate::survival::DamageKind;
use crate::tick::{DamagePiece, TickState};
use mc_physics::BlockMaterial;
use mc_physics::mth::{self, floor_i32};

static ENDERMAN_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| crate::piglin::type_id_of("minecraft:enderman"));

pub(crate) fn is_enderman(body: &crate::entities::Body) -> bool {
    matches!(body.kind, EntityKind::Mob { type_id } if Some(type_id) == *ENDERMAN_TYPE)
}

/// Cheap row gate shared by [`target`] and the tick's row preparation: only
/// rows retaining enderman anger carry a target.
pub(crate) fn needs_state(row: &ColdRow) -> bool {
    matches!(row.special_mob.as_deref(), Some(Retained::Enderman { .. }))
}

pub(crate) fn target(state: &TickState, slot: EntitySlot) -> Option<EntityId> {
    let row = state.entities.row(slot);
    if !needs_state(row) {
        return None;
    }
    let Some(&Retained::Enderman { target, until }) = row.special_mob.as_deref() else {
        return None;
    };
    if until <= state.tick {
        return None;
    }
    let target = state.entities.slot_of_uuid(target)?;
    let body = state.entities.body(target);
    (body.alive
        && state
            .players
            .get(&body.id)
            .is_some_and(|p| !p.creative() && !p.spectator()))
    .then_some(body.id)
}

fn anger(state: &mut TickState, slot: EntitySlot, player: EntityId) {
    let Some(player) = state.entities.slot_of(player) else {
        return;
    };
    let target = state.entities.row(player).uuid;
    let (body, row) = state.entities.parts_mut(slot);
    row.set_special_mob(Retained::Enderman {
        target,
        until: state.tick + 600,
    });
    state.appearance_changed.push(body.id);
}

pub(crate) fn hurt(state: &mut TickState, slot: EntitySlot, piece: &DamagePiece) {
    if is_enderman(state.entities.body(slot))
        && let Some(
            DamageSource::Melee { attacker, .. }
            | DamageSource::Mace { attacker, .. }
            | DamageSource::Sweep { attacker, .. },
        ) = piece.source
        && state.players.contains_key(&attacker)
    {
        anger(state, slot, attacker);
    }
}

pub(crate) fn damage_allowed(state: &mut TickState, slot: EntitySlot, piece: &DamagePiece) -> bool {
    if is_enderman(state.entities.body(slot)) && piece.kind == DamageKind::Projectile {
        teleport(state, slot);
        return false;
    }
    true
}

pub(crate) fn projectile_hit(state: &mut TickState, id: EntityId) -> bool {
    let Some(slot) = state.entities.slot_of(id) else {
        return false;
    };
    if !is_enderman(state.entities.body(slot)) {
        return false;
    }
    teleport(state, slot);
    true
}

pub(crate) fn teleport(state: &mut TickState, slot: EntitySlot) -> bool {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let (width, height, id) = (body.half_width, body.height, body.id);
    let mut rng = Rng(state.tick ^ id.0 as u64 ^ state.world.seed() as u64);
    for _ in 0..64 {
        let x = floor_i32(pos[0]) + (rng.next() % 63) as i32 - 31;
        let z = floor_i32(pos[2]) + (rng.next() % 63) as i32 - 31;
        let top = floor_i32(pos[1]) + (rng.next() % 32) as i32;
        for y in (floor_i32(pos[1]) - 32..=top).rev() {
            let at = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
            if at
                .iter()
                .zip(pos)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                > 32.0 * 32.0
            {
                continue;
            }
            let solid = state.world.block_at(x, y - 1, z).is_some_and(|block| {
                state.registry.materials.classify(block) == BlockMaterial::Solid
                    && state.registry.collision_boxes[block as usize]
                        .iter()
                        .any(|b| b.as_blocks() == [0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
            });
            if !solid {
                continue;
            }
            let clear = (floor_i32(at[0] - width)..=floor_i32(at[0] + width)).all(|bx| {
                (floor_i32(at[2] - width)..=floor_i32(at[2] + width)).all(|bz| {
                    (y..=floor_i32(at[1] + height - 1.0e-6)).all(|by| {
                        state.world.block_at(bx, by, bz).is_some_and(|block| {
                            state.registry.collision_boxes[block as usize].is_empty()
                                && !crate::conduit::water(
                                    state,
                                    [f64::from(bx), f64::from(by), f64::from(bz)],
                                )
                        })
                    })
                })
            });
            if clear {
                state.entities.set_pos(slot, at);
                state.entities.set_vel(slot, [0.0; 3]);
                state.entities.set_ground(slot, true);
                state.appearance_changed.push(id);
                return true;
            }
        }
    }
    false
}

fn looking(state: &TickState, slot: EntitySlot, player: EntityId) -> bool {
    let Some(player_slot) = state.entities.slot_of(player) else {
        return false;
    };
    let Some(player_state) = state.players.get(&player) else {
        return false;
    };
    if player_state.creative() || player_state.spectator() {
        return false;
    }
    let helmet = &player_state.inventory.slots[39];
    if !helmet.is_empty()
        && state
            .registry
            .items
            .name_of(helmet.item_id)
            .is_some_and(|name| name.path() == "carved_pumpkin")
    {
        return false;
    }
    let pos = state.entities.pos(slot);
    let head = [
        pos[0],
        pos[1] + state.entities.body(slot).height * 0.9,
        pos[2],
    ];
    let pos = state.entities.pos(player_slot);
    let eye = [pos[0], pos[1] + 1.62, pos[2]];
    let delta: [f64; 3] = std::array::from_fn(|axis| head[axis] - eye[axis]);
    let distance = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !(0.01..=64.0).contains(&distance) {
        return false;
    }
    let [yaw, pitch, _] = state.entities.rot(player_slot);
    let (yaw, pitch) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    let look = [
        -f64::from(mth::sin(yaw)) * f64::from(mth::cos(pitch)),
        -f64::from(mth::sin(pitch)),
        f64::from(mth::cos(yaw)) * f64::from(mth::cos(pitch)),
    ];
    delta.iter().zip(look).map(|(a, b)| a * b).sum::<f64>() / distance > 1.0 - 0.025 / distance
        && crate::hostile::sight_clear(state, eye, head)
}

pub(crate) fn tick(state: &mut TickState) {
    let endermen: Vec<_> = ENDERMAN_TYPE
        .iter()
        .flat_map(|&type_id| state.entities.mob_slots(type_id))
        .collect();
    for slot in endermen {
        let body = state.entities.body(slot);
        if !body.alive {
            continue;
        }
        let pos = state.entities.pos(slot);
        if !state.world.is_active(
            floor_i32(pos[0]).div_euclid(16),
            floor_i32(pos[2]).div_euclid(16),
        ) {
            continue;
        }
        if crate::conduit::wet(state, pos)
            || crate::conduit::water(state, [pos[0], pos[1] + body.height * 0.5, pos[2]])
        {
            state.merge_damage.push((
                slot,
                DamagePiece {
                    physical: None,
                    custom_armor: false,
                    amount: 1.0,
                    kind: DamageKind::Drowning,
                    source: None,
                },
            ));
            teleport(state, slot);
        }
        if state.entities.row(slot).controls.no_ai {
            continue;
        }
        if target(state, slot).is_some() {
            continue;
        }
        if matches!(
            state.entities.row(slot).special_mob.as_deref(),
            Some(Retained::Enderman { .. })
        ) {
            state.entities.row_mut(slot).special_mob = None;
            state.appearance_changed.push(state.entities.body(slot).id);
        }
        let player = state
            .players
            .keys()
            .copied()
            .filter(|id| looking(state, slot, *id))
            .min_by_key(|id| id.0);
        if let Some(player) = player {
            anger(state, slot, player)
        }
    }
}
