//! Creeper-style explosions (vanilla `ServerExplosion`, ADR 0013).
//!
//! [`apply`] runs at merge for an [`IntentKind::Explode`](crate::intent::IntentKind):
//! damage is planned first (distance falloff times the share of the victim's
//! body in sight of the centre), then the blast rays decide which blocks
//! break. Blocks break through the ordinary block pipeline (merge writes,
//! unsupported-block pops, fluid scheduling); their loot uses the vanilla
//! block tables with explosion decay (each drop survives with `1 / power`).

use std::collections::BTreeSet;

use mc_physics::BlockMaterial;
use mc_world::BlockStateId;

use crate::boundary::{BlockPos, EntityId, ExplosionEvent};
use crate::entities::EntityKind;
use crate::hostile::{Special, sight_clear, special_of};
use crate::natural_spawn::Rng;
use crate::registry::CoreRegistry;
use crate::survival::DamageKind;
use crate::tick::TickState;

/// Blast resistance of unbreakable blocks (bedrock).
const IMMUNE: f32 = 3_600_000.0;
/// Without the vanilla table: obsidian-class blocks.
const OBSIDIAN: f32 = 1200.0;
/// Without the vanilla table: water and lava.
const FLUID: f32 = 100.0;
/// Without the vanilla table: any other solid block (stone-ish).
const SOLID: f32 = 3.0;
/// Eye height of a player (knockback direction).
const PLAYER_EYE: f64 = 1.62;

/// Blast resistance of a block state; `None` for air (rays pass freely).
/// Vanilla values from `reports/block_explosion.json` when the registry
/// loaded them; otherwise bedrock-class blocks are immune, obsidian-class
/// ones 1200, fluids 100 and every other solid block 3.
#[must_use]
pub fn blast_resistance(registry: &CoreRegistry, state: u32) -> Option<f32> {
    let block = &registry.blocks.by_id(BlockStateId(state))?.block;
    let path = block.id.path();
    if matches!(path, "air" | "cave_air" | "void_air") {
        return None;
    }
    if let Some(table) = &registry.explosion {
        return table.resistance(state);
    }
    Some(match registry.materials.classify(state) {
        BlockMaterial::Air => 0.0,
        BlockMaterial::Water | BlockMaterial::Lava => FLUID,
        BlockMaterial::Solid => match path {
            "bedrock"
            | "barrier"
            | "end_portal"
            | "end_portal_frame"
            | "end_gateway"
            | "command_block"
            | "chain_command_block"
            | "repeating_command_block"
            | "structure_block"
            | "jigsaw" => IMMUNE,
            "obsidian" | "crying_obsidian" | "respawn_anchor" => OBSIDIAN,
            _ => SOLID,
        },
    })
}

/// Apply one blast: stage entity damage, block removal, drops and the
/// client event.
pub(crate) fn apply(
    state: &mut TickState,
    source: Option<EntityId>,
    center: [f64; 3],
    power: f32,
    mob_source: bool,
    kill_credit: Option<(EntityId, u32)>,
) {
    let mut rng = Rng((state.world.seed() as u64)
        ^ state.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (source.map_or(0, |id| id.0) as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
    // Vanilla hurts entities before the blocks go (cover still counts).
    let knockback = hurt_entities(state, source, center, power, kill_credit);
    let blocks = if !mob_source || crate::command_rules::boolean(state, "mob_griefing") {
        destroyed_blocks(state, center, power, &mut rng)
    } else {
        BTreeSet::new()
    };
    let air = state.registry.states.air;
    let mut spawned: Vec<EntityId> = Vec::new();
    let decay = crate::command_rules::boolean(
        state,
        if mob_source {
            "mob_explosion_drop_decay"
        } else {
            "tnt_explosion_drop_decay"
        },
    );
    for &(x, y, z) in &blocks {
        let pos = BlockPos { x, y, z };
        if state
            .merge_block_writes
            .iter()
            .rev()
            .find(|(cell, _)| *cell == pos)
            .is_some_and(|(_, pending)| *pending == air)
        {
            continue;
        }
        let Some(old) = state.world.block_at(x, y, z) else {
            continue;
        };
        // TNT is a chain fuse, never explosion loot. A disabled TNT rule
        // suppresses priming but the blast still destroys it without a drop.
        if crate::tnt::is_tnt(&state.registry, old) {
            let fuse = crate::tnt::DEFAULT_FUSE / 8
                + (rng.next() % u64::from(crate::tnt::DEFAULT_FUSE / 4)) as u32;
            if crate::tnt::prime_block(state, pos, fuse).is_none() {
                crate::plugin_inventories::invalidate_block(state, pos);
                state.world.set_block(x, y, z, air);
                state.merge_block_writes.push((pos, air));
            }
            continue;
        }
        let rolled = if crate::command_rules::boolean(state, "block_drops") {
            crate::block_drop_context::explosion_drops(
                state,
                pos,
                old,
                decay.then_some(power),
                rng.next(),
            )
        } else {
            Ok(Vec::new())
        };
        let Some(drops) = crate::block_drop_context::admit(state, pos, rolled) else {
            continue;
        };
        state.stage_container_removal(pos, !crate::block_drop_context::retains_contents(&drops));
        state.merge_block_writes.push((pos, air));
        state.spawn_block_loot(pos, drops, rng.next(), &mut spawned);
    }
    state.hostile.spawned.append(&mut spawned);
    state.hostile.explosions.push(ExplosionEvent {
        center,
        radius: power,
        blocks: blocks.len() as u32,
        knockback,
    });
}

/// Vanilla ExplosionInteraction.NONE: entity damage and packets, no block loot.
pub(crate) fn apply_no_blocks(
    state: &mut TickState,
    center: [f64; 3],
    power: f32,
    excluded: &[EntityId],
) {
    let pending = state.merge_damage.len();
    let mut knockback = hurt_entities(state, None, center, power, None);
    let mut index = 0;
    state.merge_damage.retain(|(slot, _)| {
        index += 1;
        index <= pending || !excluded.contains(&state.entities.body(*slot).id)
    });
    knockback.retain(|(id, _)| !excluded.contains(id));
    state.hostile.explosions.push(ExplosionEvent {
        center,
        radius: power,
        blocks: 0,
        knockback,
    });
}

/// Vanilla `ServerExplosion.calculateExplodedPositions`: rays from the
/// centre through the faces of a 16x16x16 grid lose strength to each block's
/// resistance; every non-air, non-fluid block they still hit with strength
/// left breaks.
fn destroyed_blocks(
    state: &TickState,
    center: [f64; 3],
    power: f32,
    rng: &mut Rng,
) -> BTreeSet<(i32, i32, i32)> {
    let mut out = BTreeSet::new();
    for gx in 0..16 {
        for gy in 0..16 {
            for gz in 0..16 {
                if gx != 0 && gx != 15 && gy != 0 && gy != 15 && gz != 0 && gz != 15 {
                    continue;
                }
                let axis = |g: i32| f64::from(g as f32 / 15.0 * 2.0 - 1.0);
                let mut direction = [axis(gx), axis(gy), axis(gz)];
                let length = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
                direction = direction.map(|v| v / length);
                let mut strength = power * (0.7 + rng.unit() as f32 * 0.6);
                let mut at = center;
                while strength > 0.0 {
                    let cell = at.map(|v| v.floor() as i32);
                    let Some(block) = state.world.block_at(cell[0], cell[1], cell[2]) else {
                        break;
                    };
                    let resistance = blast_resistance(&state.registry, block);
                    if let Some(resistance) = resistance {
                        strength -= (resistance + 0.3) * 0.3;
                    }
                    if strength > 0.0
                        && resistance.is_some()
                        && !state.registry.materials.classify(block).is_fluid()
                    {
                        out.insert((cell[0], cell[1], cell[2]));
                    }
                    for (v, d) in at.iter_mut().zip(direction) {
                        *v += d * 0.3;
                    }
                    strength -= 0.225;
                }
            }
        }
    }
    out
}

/// Vanilla `hurtEntities`: damage `((i^2 + i) / 2 * 7 * 2p + 1)` with impact
/// `i = (1 - distance / 2p) * exposure`, scaled by difficulty. Returns the
/// knockback of the hit players (blocks/tick), which the client applies.
fn hurt_entities(
    state: &mut TickState,
    source: Option<EntityId>,
    center: [f64; 3],
    power: f32,
    kill_credit: Option<(EntityId, u32)>,
) -> Vec<(EntityId, [f64; 3])> {
    let reach = f64::from(power) * 2.0;
    let mut hits = Vec::new();
    let mut knockback = Vec::new();
    for slot in state.entities.slots() {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if Some(body.id) == source || !body.alive || row.tnt.is_some() || row.falling.is_some() {
            continue;
        }
        let is_player = match body.kind {
            EntityKind::Player { .. } => {
                if state
                    .players
                    .get(&body.id)
                    .is_none_or(|player| player.creative() || player.spectator())
                    || state.entity_session_locked(body.id)
                {
                    continue;
                }
                true
            }
            EntityKind::Mob { type_id } => {
                if state
                    .registry
                    .blueprint(type_id)
                    .is_none_or(|blueprint| special_of(blueprint) == Some(Special::Projectile))
                {
                    continue;
                }
                false
            }
            EntityKind::Item { .. } | EntityKind::ExperienceOrb { .. } => continue,
        };
        let pos = state.entities.pos(slot);
        let distance = pos
            .iter()
            .zip(center)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt()
            / reach;
        if distance > 1.0 {
            continue;
        }
        let min = [pos[0] - body.half_width, pos[1], pos[2] - body.half_width];
        let max = [
            pos[0] + body.half_width,
            pos[1] + body.height,
            pos[2] + body.half_width,
        ];
        let impact = (1.0 - distance) * f64::from(seen_percent(state, min, max, center));
        let damage = ((impact * impact + impact) / 2.0 * 7.0 * reach + 1.0).floor() as f32;
        hits.push((slot, state.difficulty.scale_mob_damage(damage)));
        if is_player {
            let eye = [pos[0], pos[1] + PLAYER_EYE, pos[2]];
            let away: [f64; 3] = std::array::from_fn(|axis| eye[axis] - center[axis]);
            let length = away.iter().map(|v| v * v).sum::<f64>().sqrt();
            if length > 0.0 {
                knockback.push((body.id, away.map(|v| v / length * impact)));
            }
        }
    }
    for (slot, amount) in hits {
        state.merge_damage.push((
            slot,
            crate::tick::DamagePiece {
                physical: None,
                custom_armor: false,
                amount,
                kind: DamageKind::Explosion,
                source: source.map(|source| crate::intent::DamageSource::Explosion {
                    source,
                    kill_credit,
                }),
            },
        ));
    }
    knockback
}

/// Vanilla `getSeenPercent`: the share of a grid of points over the body
/// box with a clear line to the centre.
pub(crate) fn seen_percent(
    state: &TickState,
    min: [f64; 3],
    max: [f64; 3],
    center: [f64; 3],
) -> f32 {
    let step: [f64; 3] = std::array::from_fn(|axis| 1.0 / ((max[axis] - min[axis]) * 2.0 + 1.0));
    let offset = |axis: usize| (1.0 - (1.0 / step[axis]).floor() * step[axis]) / 2.0;
    let (x_offset, z_offset) = (offset(0), offset(2));
    let (mut clear, mut total) = (0_u32, 0_u32);
    let mut fx = 0.0;
    while fx <= 1.0 {
        let mut fy = 0.0;
        while fy <= 1.0 {
            let mut fz = 0.0;
            while fz <= 1.0 {
                let from = [
                    min[0] + (max[0] - min[0]) * fx + x_offset,
                    min[1] + (max[1] - min[1]) * fy,
                    min[2] + (max[2] - min[2]) * fz + z_offset,
                ];
                if sight_clear(state, from, center) {
                    clear += 1;
                }
                total += 1;
                fz += step[2];
            }
            fy += step[1];
        }
        fx += step[0];
    }
    clear as f32 / total as f32
}
