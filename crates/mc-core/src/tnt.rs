//! Native primed TNT: retained fuse/state, collision-only motion and merge blasts.
//! 26.1.2 PrimedTnt javap fixes gravity, drag, launch, bounce and save keys.

use mc_physics::mth;
use std::sync::Arc;

use mc_nbt::Tag;
use mc_physics::{Aabb, Vec3};
use mc_world::BlockStateId;
use serde::{Deserialize, Serialize};

use crate::boundary::{BlockPos, EntityDataValue, EntityId};
use crate::config::CoreError;
use crate::entities::EntitySlot;
use crate::intent::{Footprint, Intent, IntentKind};
use crate::registry::CoreRegistry;
use crate::tick::{TickSampler, TickState};

pub const DEFAULT_FUSE: u32 = 80;
// PrimedTnt extends Entity (eight inherited entries), not LivingEntity.
pub const FUSE_INDEX: u8 = 8;
pub const BLOCK_STATE_INDEX: u8 = 9;
const HEIGHT: f64 = 0.98_f32 as f64;
const HALF_WIDTH: f64 = HEIGHT / 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PrimedTntState {
    pub fuse: u32,
    pub block_state: u32,
    pub explosion_power: f32,
    #[serde(default)]
    pub on_ground: bool,
}

fn default_block(registry: &CoreRegistry) -> Option<u32> {
    let name = mc_data::Identifier::parse("minecraft:tnt").expect("static TNT identifier");
    registry.blocks.block(&name).map(|block| block.default.0)
}

/// Generic spawn/summon initializes vanilla TNT without a launch impulse or
/// publication side effect; the caller owns that ordinary spawn transaction.
pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot, type_id: u32) {
    if mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
        .is_none_or(|contract| contract.name != "minecraft:tnt")
    {
        return;
    }
    let block_state = default_block(&state.registry).expect("vanilla TNT block registered");
    state.entities.row_mut(slot).tnt = Some(PrimedTntState {
        fuse: DEFAULT_FUSE,
        block_state,
        explosion_power: 4.0,
        on_ground: state.entities.ground(slot),
    });
    restore(state, slot);
}

pub(crate) fn is_tnt(registry: &CoreRegistry, block_state: u32) -> bool {
    registry
        .blocks
        .by_id(BlockStateId(block_state))
        .is_some_and(|block| block.block.id.as_str() == "minecraft:tnt")
}

/// Immediate powered-block ignition; TNT is not a redstone dust connector.
pub(crate) fn changed(state: &mut TickState, pos: BlockPos) {
    if state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .is_some_and(|block| is_tnt(&state.registry, block))
        && crate::redstone::power_at(state, pos) != 0
    {
        let _ = prime_block(state, pos, DEFAULT_FUSE);
    }
}

/// Prime a dispenser item without touching any world block. Callers debit only
/// on success; spawn publication and retained state belong to this helper.
pub fn prime_at(state: &mut TickState, position: [f64; 3], fuse: u32) -> Option<EntityId> {
    let block_state = default_block(&state.registry)?;
    prime(state, position, fuse, block_state)
}

/// Replace exactly one resident TNT block on successful priming. The overlay
/// changes immediately, so repeated ignition in this tick cannot prime twice.
pub fn prime_block(state: &mut TickState, pos: BlockPos, fuse: u32) -> Option<EntityId> {
    let block_state = state.world.block_at(pos.x, pos.y, pos.z)?;
    if !is_tnt(&state.registry, block_state) {
        return None;
    }
    let id = prime(
        state,
        [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ],
        fuse,
        block_state,
    )?;
    let air = state.registry.states.air;
    crate::plugin_inventories::invalidate_block(state, pos);
    state.world.set_block(pos.x, pos.y, pos.z, air);
    state.merge_block_writes.push((pos, air));
    Some(id)
}

fn prime(
    state: &mut TickState,
    position: [f64; 3],
    fuse: u32,
    block_state: u32,
) -> Option<EntityId> {
    if !crate::command_rules::boolean(state, "tnt_explodes")
        || fuse > i16::MAX as u32
        || position.iter().any(|value| !value.is_finite())
        || state
            .world
            .block_at(
                position[0].floor() as i32,
                position[1].floor() as i32,
                position[2].floor() as i32,
            )
            .is_none()
    {
        return None;
    }
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:tnt").expect("static TNT identifier"))?;
    let id = state.spawn_mob(type_id, position, 1.0);
    let slot = state.entities.slot_of(id).expect("new TNT slot");
    state.entities.row_mut(slot).tnt = Some(PrimedTntState {
        fuse,
        block_state,
        explosion_power: 4.0,
        on_ground: false,
    });
    restore(state, slot);
    let mut random = crate::natural_spawn::Rng(
        (state.world.seed() as u64)
            ^ state.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (id.0 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9),
    );
    let angle = random.unit() * 6.283_185_482_025_146_5;
    // Store velocities in blocks/second, as the native entity boundary does.
    state.entities.set_vel(
        slot,
        [
            -f64::from(mth::sin(angle)) * 0.4,
            0.200_000_002_980_232_24 * 20.0,
            -f64::from(mth::cos(angle)) * 0.4,
        ],
    );
    state.hostile.spawned.push(id);
    Some(id)
}

pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    let id = state.entities.body(slot).id;
    if state.entities.row(slot).tnt.is_none() {
        return;
    }
    state.entities.track_tnt(id);
    state.entities.set_half_width(slot, HALF_WIDTH);
    let (body, row) = state.entities.parts_mut(slot);
    let Some(record) = row.tnt else {
        return;
    };
    body.movement = crate::blueprint::MovementClass::Stationary;
    body.height = HEIGHT;
    row.speed = 0.0;
    row.attack_damage = 0.0;
    row.sight_range = 0.0;
    row.attack_reach = 0.0;
    update_appearance(row, record);
}

fn update_appearance(row: &mut crate::entities::ColdRow, record: PrimedTntState) {
    let appearance = Arc::make_mut(&mut row.appearance);
    for (index, value) in [
        (FUSE_INDEX, EntityDataValue::VarInt(record.fuse as i32)),
        (
            BLOCK_STATE_INDEX,
            EntityDataValue::BlockState(record.block_state),
        ),
    ] {
        if let Some((_, old)) = appearance.extra.iter_mut().find(|(at, _)| *at == index) {
            *old = value;
        } else {
            appearance.extra.push((index, value));
        }
    }
}

/// Run before generic entity physics. Expiration produces an ordinary explosion
/// intent and one removal; explosions never recurse directly into chain TNT.
pub(crate) fn run(state: &mut TickState, intents: &mut Vec<Intent>) {
    let Some(last) = state.entities.last_tnt() else {
        return;
    };
    let mut previous = None;
    while let Some(id) = state.entities.next_tnt(previous) {
        if id > last {
            break;
        }
        previous = Some(id);
        let Some(slot) = state.entities.slot_of(id) else {
            continue;
        };
        let Some(mut record) = state.entities.row(slot).tnt else {
            continue;
        };
        let position = state.entities.pos(slot);
        if !state.world.is_active(
            (position[0].floor() as i32).div_euclid(16),
            (position[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        if state
            .world
            .block_at(
                position[0].floor() as i32,
                position[1].floor() as i32,
                position[2].floor() as i32,
            )
            .is_none()
        {
            continue;
        }
        let mut velocity = state.entities.vel(slot).map(|value| value / 20.0);
        velocity[1] -= 0.04;
        let desired = Vec3::new(velocity[0], velocity[1], velocity[2]);
        let resolved = mc_physics::resolve_collision_displacement(
            &TickSampler {
                world: &state.world,
                registry: &state.registry,
            },
            Vec3::new(position[0], position[1], position[2]),
            Aabb {
                half_width: HALF_WIDTH,
                height: HEIGHT,
            },
            desired,
            state.entities.ground(slot),
            0.0,
        );
        if let Some(resolved) = resolved {
            let delta = [resolved.delta.x, resolved.delta.y, resolved.delta.z];
            state.entities.set_pos(
                slot,
                std::array::from_fn(|axis| position[axis] + delta[axis]),
            );
            // Entity.move clips momentum on collision before TNT applies drag.
            for axis in 0..3 {
                if (delta[axis] - velocity[axis]).abs() > 1.0e-7 {
                    velocity[axis] = 0.0;
                }
                velocity[axis] *= 0.98;
            }
            if resolved.grounded {
                velocity[0] *= 0.7;
                velocity[1] *= -0.5;
                velocity[2] *= 0.7;
            }
            state
                .entities
                .set_vel(slot, velocity.map(|value| value * 20.0));
            state.entities.set_ground(slot, resolved.grounded);
        }
        record.on_ground = state.entities.ground(slot);
        record.fuse = record.fuse.saturating_sub(1);
        if record.fuse == 0 {
            if crate::command_rules::boolean(state, "tnt_explodes") {
                let mut center = state.entities.pos(slot);
                center[1] += HEIGHT * 0.0625;
                intents.push(Intent {
                    actor: id,
                    seq: state.next_seq(id),
                    kind: IntentKind::Explode {
                        center,
                        power: record.explosion_power,
                        mob_grief: None,
                        kill_credit: None,
                    },
                    footprint: Footprint::default(),
                });
            }
            state.remove_entity(slot);
            state.hostile.removed.push(id);
            continue;
        }
        state.entities.row_mut(slot).tnt = Some(record);
        update_appearance(state.entities.row_mut(slot), record);
        state.appearance_changed.push(id);
    }
}

pub(crate) fn from_record(
    registry: &CoreRegistry,
    record: &mc_data::entity_nbt::VanillaEntity,
) -> Result<Option<PrimedTntState>, CoreError> {
    if record.id != "minecraft:tnt" {
        return Ok(None);
    }
    let mut result = PrimedTntState {
        fuse: DEFAULT_FUSE,
        block_state: default_block(registry)
            .ok_or_else(|| CoreError::Storage("missing TNT block".into()))?,
        explosion_power: 4.0,
        on_ground: record.on_ground,
    };
    for (key, value) in &record.extra_fields {
        match (key.as_str(), value) {
            ("fuse", Tag::Short(value)) => result.fuse = (*value).max(0) as u32,
            ("explosion_power", Tag::Float(value)) if value.is_finite() => {
                result.explosion_power = value.clamp(0.0, 128.0)
            }
            ("block_state", Tag::Compound(fields)) => {
                let Some(Tag::String(name)) = mc_data::item_nbt::field(fields, "Name") else {
                    return Err(CoreError::Storage("invalid TNT block state name".into()));
                };
                let name = mc_data::Identifier::parse(name)
                    .map_err(|err| CoreError::Storage(err.to_string()))?;
                let block = registry
                    .blocks
                    .block(&name)
                    .ok_or_else(|| CoreError::Storage("unknown TNT retained block".into()))?;
                let mut properties = registry
                    .blocks
                    .by_id(block.default)
                    .expect("registered block default")
                    .properties
                    .clone();
                if let Some(Tag::Compound(fields)) = mc_data::item_nbt::field(fields, "Properties")
                {
                    for (key, value) in fields {
                        let Tag::String(value) = value else {
                            return Err(CoreError::Storage("invalid TNT block property".into()));
                        };
                        let Some((_, old)) = properties.iter_mut().find(|(name, _)| name == key)
                        else {
                            return Err(CoreError::Storage(
                                "unknown TNT retained block property".into(),
                            ));
                        };
                        old.clone_from(value);
                    }
                }
                result.block_state = registry
                    .blocks
                    .by_name_and_props(&name, &properties)
                    .ok_or_else(|| CoreError::Storage("unknown TNT retained block state".into()))?
                    .0;
            }
            _ => {}
        }
    }
    Ok(Some(result))
}

pub(crate) fn to_fields(
    record: &PrimedTntState,
    blocks: &mc_world::BlockRegistry,
) -> Vec<(String, Tag)> {
    let block = blocks
        .by_id(BlockStateId(record.block_state))
        .expect("retained TNT state registered");
    let mut block_fields = vec![("Name".into(), Tag::String(block.block.id.as_str().into()))];
    if !block.properties.is_empty() {
        block_fields.push((
            "Properties".into(),
            Tag::Compound(
                block
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), Tag::String(value.clone())))
                    .collect(),
            ),
        ));
    }
    let mut fields = vec![
        (
            "fuse".into(),
            Tag::Short(record.fuse.min(i16::MAX as u32) as i16),
        ),
        ("block_state".into(), Tag::Compound(block_fields)),
    ];
    if record.explosion_power != 4.0 {
        fields.push(("explosion_power".into(), Tag::Float(record.explosion_power)));
    }
    fields
}

#[cfg(test)]
#[path = "tnt_tests.rs"]
mod tests;
