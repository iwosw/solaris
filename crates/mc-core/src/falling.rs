//! Native falling-block lifecycle. Source removal, entity image and landing
//! are committed on the tick owner; no instantaneous column teleportation.
use crate::boundary::{BlockPos, EntityId};
use crate::registry::CoreRegistry;
use crate::tick::TickState;
use mc_data::Identifier;
use mc_nbt::Tag;
use mc_world::{BlockRegistry, BlockStateId};
use std::sync::Arc;

static FALLING_BLOCK_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| crate::piglin::type_id_of("minecraft:falling_block"));

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FallingState {
    pub block_state: u32,
    pub age: u32,
    pub touched_water: bool,
}

/// Generic spawn paths receive the vanilla default state too; an untyped
/// falling body must never checkpoint as an unrecoverable nonliving Walker.
pub(crate) fn initialize(state: &mut TickState, slot: crate::entities::EntitySlot, type_id: u32) {
    if mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
        .is_none_or(|contract| contract.name != "minecraft:falling_block")
    {
        return;
    }
    debug_assert!(
        matches!(
            state.entities.body(slot).kind,
            crate::entities::EntityKind::Mob { type_id: kind }
            if kind == type_id
        ),
        "falling component implies the falling-block mob lane"
    );
    let sand = state.registry.states.sand;
    state.entities.row_mut(slot).falling = Some(FallingState {
        block_state: sand,
        age: 0,
        touched_water: false,
    });
    let id = state.entities.body(slot).id;
    configure(state, id);
}

pub(crate) fn configure(state: &mut TickState, id: EntityId) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    state.entities.set_half_width(slot, 0.49);
    let (body, row) = state.entities.parts_mut(slot);
    body.movement = crate::blueprint::MovementClass::Stationary;
    body.height = 0.98;
    row.speed = 0.0;
    row.persistent = true;
    if let Some(falling) = row.falling {
        Arc::make_mut(&mut row.appearance).spawn_data = falling.block_state as i32;
    }
}

fn prop<'a>(state: &'a mc_world::BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}
fn hanging(blocks: &BlockRegistry, id: u32) -> bool {
    blocks.by_id(BlockStateId(id)).is_some_and(|state| {
        state.block.id.path() == "pointed_dripstone"
            && prop(state, "vertical_direction") == Some("down")
    })
}
pub(crate) fn gravity(blocks: &BlockRegistry, id: u32) -> bool {
    blocks.by_id(BlockStateId(id)).is_some_and(|state| {
        let path = state.block.id.path();
        path.ends_with("_concrete_powder")
            || matches!(
                path,
                "sand"
                    | "red_sand"
                    | "gravel"
                    | "anvil"
                    | "chipped_anvil"
                    | "damaged_anvil"
                    | "dragon_egg"
                    | "suspicious_sand"
                    | "suspicious_gravel"
            )
            || hanging(blocks, id)
    })
}
fn enter(state: &TickState, block: u32) -> bool {
    crate::fluids::fluid_at(&state.registry.states, block).is_some()
        || state
            .registry
            .blocks
            .by_id(BlockStateId(block))
            .is_some_and(|entry| {
                mc_data::collision_shapes::vanilla_collision_shapes()
                    .get_for_state(block, &entry.block.id, &entry.properties)
                    .is_some_and(|shape| shape.is_empty())
            })
}
fn set(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>, pos: BlockPos, block: u32) {
    state.world.set_block(pos.x, pos.y, pos.z, block);
    writes.retain(|(cell, _)| *cell != pos);
    writes.push((pos, block));
}
pub(crate) fn concrete(blocks: &BlockRegistry, block: u32) -> Option<u32> {
    let entry = blocks.by_id(BlockStateId(block))?;
    let color = entry.block.id.path().strip_suffix("_concrete_powder")?;
    blocks
        .block(&Identifier::parse(format!("minecraft:{color}_concrete")).ok()?)
        .map(|block| block.default.0)
}

/// Sources are scanned bottom-up so a stack becomes distinct retained entities.
pub(crate) fn start(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let Some(type_id) = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:falling_block").expect("static id"))
    else {
        return;
    };
    let mut cells: Vec<_> = writes
        .iter()
        .flat_map(|(pos, _)| {
            [
                *pos,
                BlockPos {
                    y: pos.y + 1,
                    ..*pos
                },
                BlockPos {
                    y: pos.y - 1,
                    ..*pos
                },
            ]
        })
        .collect();
    cells.sort_by_key(|pos| (pos.x, pos.z, pos.y));
    cells.dedup();
    let mut index = 0;
    while index < cells.len() {
        let pos = cells[index];
        index += 1;
        let Some(block) = state.world.block_at(pos.x, pos.y, pos.z) else {
            continue;
        };
        if !gravity(&state.registry.blocks, block) {
            continue;
        }
        let suspended = hanging(&state.registry.blocks, block);
        let support_y = pos.y + if suspended { 1 } else { -1 };
        let Some(support) = state.world.block_at(pos.x, support_y, pos.z) else {
            continue;
        };
        if !enter(state, support) {
            continue;
        }
        let air = state.registry.states.air;
        set(state, writes, pos, air);
        let id = state.spawn_mob(
            type_id,
            [pos.x as f64 + 0.5, pos.y as f64, pos.z as f64 + 0.5],
            1.0,
        );
        let slot = state.entities.slot_of(id).expect("spawned falling block");
        state.entities.row_mut(slot).falling = Some(FallingState {
            block_state: block,
            age: 0,
            touched_water: false,
        });
        configure(state, id);
        spawned.push(id);
        let next = BlockPos {
            y: pos.y + if suspended { -1 } else { 1 },
            ..pos
        };
        if !cells.contains(&next) {
            cells.push(next);
        }
    }
}

pub(crate) fn run(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
    removed: &mut Vec<EntityId>,
) {
    let ids: Vec<_> = FALLING_BLOCK_TYPE
        .iter()
        .flat_map(|&type_id| state.entities.mob_slots(type_id))
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            row.falling.is_some().then_some(body.id)
        })
        .collect();
    for id in ids {
        let Some(slot) = state.entities.slot_of(id) else {
            continue;
        };
        let mut falling = state.entities.row(slot).falling.expect("falling payload");
        let old = state.entities.pos(slot);
        let vel = state.entities.vel(slot);
        let desired = mc_physics::Vec3::new(vel[0] / 20.0, vel[1] / 20.0 - 0.04, vel[2] / 20.0);
        let next_y = old[1] + desired.y;
        let x = old[0].floor() as i32;
        let z = old[2].floor() as i32;
        if !state.world.is_active(x.div_euclid(16), z.div_euclid(16)) {
            continue;
        }
        if falling.age >= 600 || next_y < f64::from(state.dimension.geometry().min_y()) {
            drop(
                state,
                BlockPos {
                    x,
                    y: next_y.floor() as i32,
                    z,
                },
                falling.block_state,
                spawned,
            );
            state.remove_entity(slot);
            removed.push(id);
            continue;
        }
        let min_x = (old[0].min(old[0] + desired.x) - 0.49).floor() as i32;
        let max_x = (old[0].max(old[0] + desired.x) + 0.49).floor() as i32;
        let min_z = (old[2].min(old[2] + desired.z) - 0.49).floor() as i32;
        let max_z = (old[2].max(old[2] + desired.z) + 0.49).floor() as i32;
        // Missing columns pause the entire retained step, never form a floor.
        if (min_x.div_euclid(16)..=max_x.div_euclid(16)).any(|cx| {
            (min_z.div_euclid(16)..=max_z.div_euclid(16)).any(|cz| !state.world.has_chunk(cx, cz))
        }) {
            continue;
        }
        let sampler = crate::tick::TickSampler {
            world: &state.world,
            registry: &state.registry,
        };
        let Some(resolved) = mc_physics::resolve_collision_displacement(
            &sampler,
            mc_physics::Vec3::new(old[0], old[1], old[2]),
            mc_physics::Aabb {
                half_width: 0.49,
                height: 0.98,
            },
            desired,
            false,
            0.0,
        ) else {
            continue;
        };
        let next = [
            old[0] + resolved.delta.x,
            old[1] + resolved.delta.y,
            old[2] + resolved.delta.z,
        ];
        state.entities.row_mut(slot).controls.fall_distance += (old[1] - next[1]).max(0.0);
        let powder = state
            .registry
            .blocks
            .by_id(BlockStateId(falling.block_state))
            .is_some_and(|entry| entry.block.id.path().ends_with("_concrete_powder"));
        let mut water_landing = None;
        let Some(samples) = mc_physics::sweep_sample_count(resolved.delta, 0.25) else {
            continue;
        };
        'sweep: for sample in 0..=if powder { samples } else { 0 } {
            if !powder {
                break;
            }
            let t = sample as f64 / samples.max(1) as f64;
            let center = [
                old[0] + resolved.delta.x * t,
                old[1] + resolved.delta.y * t,
                old[2] + resolved.delta.z * t,
            ];
            for x in
                ((center[0] - 0.49).floor() as i32)..=((center[0] + 0.49 - 1e-7).floor() as i32)
            {
                for y in (center[1].floor() as i32)..=((center[1] + 0.98 - 1e-7).floor() as i32) {
                    for z in ((center[2] - 0.49).floor() as i32)
                        ..=((center[2] + 0.49 - 1e-7).floor() as i32)
                    {
                        let block = state.world.block_at(x, y, z);
                        let wet = block
                            .and_then(|block| crate::fluids::contained(state, block))
                            .is_some_and(|(fluid, _)| fluid == crate::fluids::Fluid::Water);
                        falling.touched_water |= wet;
                        if wet
                            && block
                                .and_then(|id| crate::fluids::fluid_at(&state.registry.states, id))
                                .is_some()
                        {
                            water_landing = Some(BlockPos { x, y, z });
                            break 'sweep;
                        }
                    }
                }
            }
        }
        falling.age = falling.age.saturating_add(1);
        let landing = water_landing.or_else(|| {
            resolved.grounded.then_some(BlockPos {
                x: next[0].floor() as i32,
                y: next[1].floor() as i32,
                z: next[2].floor() as i32,
            })
        });
        if let Some(pos) = landing {
            if resolved.grounded {
                impact(state, slot, falling.block_state, next);
            }
            let landed_block = if resolved.grounded {
                anvil_landing_state(state, slot, falling.block_state)
            } else {
                Some(falling.block_state)
            };
            if let Some(block) = landed_block {
                falling.block_state = block;
            }
            let current = state
                .world
                .block_at(pos.x, pos.y, pos.z)
                .expect("resident landing");
            let path = state
                .registry
                .blocks
                .by_id(BlockStateId(falling.block_state))
                .expect("retained registered state")
                .block
                .id
                .path();
            let shatter = landed_block.is_none()
                || matches!(
                    path,
                    "suspicious_sand" | "suspicious_gravel" | "pointed_dripstone"
                );
            let replaceable = crate::tick::replaceable(state, current);
            if !replaceable && !shatter {
                drop(state, pos, falling.block_state, spawned);
            } else if !shatter {
                let block = if falling.touched_water {
                    concrete(&state.registry.blocks, falling.block_state)
                        .unwrap_or(falling.block_state)
                } else {
                    falling.block_state
                };
                set(state, writes, pos, block);
            }
            state.remove_entity(slot);
            removed.push(id);
        } else {
            state.entities.set_pos(slot, next);
            state.entities.set_vel(
                slot,
                [
                    if (resolved.delta.x - desired.x).abs() < 1e-7 {
                        vel[0] * 0.98
                    } else {
                        0.0
                    },
                    desired.y * 0.98 * 20.0,
                    if (resolved.delta.z - desired.z).abs() < 1e-7 {
                        vel[2] * 0.98
                    } else {
                        0.0
                    },
                ],
            );
            state.entities.row_mut(slot).falling = Some(falling);
        }
    }
}

fn anvil_landing_state(
    state: &TickState,
    slot: crate::entities::EntitySlot,
    block: u32,
) -> Option<u32> {
    let entry = state.registry.blocks.by_id(BlockStateId(block))?;
    let next = match entry.block.id.path() {
        "anvil" => Some("minecraft:chipped_anvil"),
        "chipped_anvil" => Some("minecraft:damaged_anvil"),
        "damaged_anvil" => None,
        _ => return Some(block),
    };
    let distance = (state.entities.row(slot).controls.fall_distance - 1.0).ceil();
    if distance <= 0.0 {
        return Some(block);
    }
    let roll = crate::farming::hash(&[
        state.tick,
        state.entities.body(slot).id.0 as u64,
        u64::from(block),
    ]);
    if (roll as f64 / u64::MAX as f64) >= (0.05 + distance * 0.05).min(1.0) {
        return Some(block);
    }
    next.and_then(|name| {
        state.registry.blocks.by_name_and_props(
            &Identifier::parse(name).expect("static anvil id"),
            &entry.properties,
        )
    })
    .map(|id| id.0)
}

/// Only a landing hazard queries neighboring entity buckets; ordinary falling
/// sand and airborne hazards do not scan other entities.
fn impact(
    state: &mut TickState,
    falling: crate::entities::EntitySlot,
    block: u32,
    position: [f64; 3],
) {
    let Some(entry) = state.registry.blocks.by_id(BlockStateId(block)) else {
        return;
    };
    let (kind, multiplier) = match entry.block.id.path() {
        "anvil" | "chipped_anvil" | "damaged_anvil" => {
            (crate::survival::DamageKind::FallingAnvil, 2.0)
        }
        "pointed_dripstone" => (crate::survival::DamageKind::FallingStalactite, 6.0),
        _ => return,
    };
    let distance = state.entities.row(falling).controls.fall_distance;
    let amount = ((distance - 1.0).ceil().max(0.0) as f32 * multiplier).min(40.0);
    if amount == 0.0 {
        return;
    }
    let reach = state.entities.max_half_width() + 0.49;
    let mut targets = Vec::new();
    for x in ((position[0] - reach).floor() as i32).div_euclid(16)
        ..=((position[0] + reach).floor() as i32).div_euclid(16)
    {
        for z in ((position[2] - reach).floor() as i32).div_euclid(16)
            ..=((position[2] + reach).floor() as i32).div_euclid(16)
        {
            state
                .entities
                .for_each_slot_in_chunk(crate::boundary::ChunkPos { x, z }, |slot| {
                    let body = state.entities.body(slot);
                    let p = state.entities.pos(slot);
                    if slot != falling
                        && body.alive
                        && (p[0] - position[0]).abs() < body.half_width + 0.49
                        && (p[2] - position[2]).abs() < body.half_width + 0.49
                        && p[1] < position[1] + 0.98
                        && p[1] + body.height > position[1]
                    {
                        targets.push(slot);
                    }
                });
        }
    }
    targets.sort_unstable();
    targets.dedup();
    for slot in targets {
        state.merge_damage.push((
            slot,
            crate::tick::DamagePiece {
                amount,
                kind,
                source: None,
                physical: None,
                custom_armor: false,
            },
        ));
    }
}
fn drop(state: &mut TickState, pos: BlockPos, block: u32, spawned: &mut Vec<EntityId>) {
    // Falling entities use entity_drops, independently of block breaking.
    if !crate::command_rules::boolean(state, "entity_drops") {
        return;
    }
    let stacks = match crate::block_drops::block_drops(
        &crate::block_drops::BlockDropRules {
            blocks: &state.registry.blocks,
            loot: &state.registry.loot,
            mining: &state.registry.mining,
        },
        &state.registry.item_rules(),
        block,
        &mc_data::ItemStack::EMPTY,
        state.tick,
        None,
    ) {
        Ok(stacks) => stacks,
        Err(error) => {
            crate::block_drop_context::reject(state, pos, error);
            return;
        }
    };
    let center = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ];
    for stack in stacks {
        spawned.push(crate::tick::spawn_item_entity(state, center, stack));
    }
}

pub fn to_fields(falling: &FallingState, blocks: &BlockRegistry) -> Vec<(String, Tag)> {
    let Some(block) = blocks.by_id(BlockStateId(falling.block_state)) else {
        return Vec::new();
    };
    vec![
        (
            "BlockState".into(),
            Tag::Compound(vec![
                ("Name".into(), Tag::String(block.block.id.as_str().into())),
                (
                    "Properties".into(),
                    Tag::Compound(
                        block
                            .properties
                            .iter()
                            .map(|(key, value)| (key.clone(), Tag::String(value.clone())))
                            .collect(),
                    ),
                ),
            ]),
        ),
        ("Time".into(), Tag::Int(falling.age as i32)),
        ("DropItem".into(), Tag::Byte(1)),
        (
            "SolarisTouchedWater".into(),
            Tag::Byte(i8::from(falling.touched_water)),
        ),
    ]
}
fn saved_field<'a>(fields: &'a [(String, Tag)], name: &str) -> Result<Option<&'a Tag>, String> {
    let mut matches = fields.iter().filter(|(key, _)| key == name);
    let value = matches.next().map(|(_, tag)| tag);
    if matches.next().is_some() {
        return Err(format!("{name}: duplicate field"));
    }
    Ok(value)
}

pub fn from_record(
    record: &mc_data::entity_nbt::VanillaEntity,
    registry: &CoreRegistry,
) -> Result<Option<FallingState>, String> {
    if record.id.as_str() != "minecraft:falling_block" {
        return Ok(None);
    }
    for (name, values) in [("Pos", &record.pos), ("Motion", &record.motion)] {
        if let Some(index) = values.iter().position(|value| !value.is_finite()) {
            return Err(format!("{name}[{index}]: non-finite number"));
        }
    }
    if !record.yaw.is_finite() {
        return Err("Rotation[0]: non-finite yaw".into());
    }
    if !record.pitch.is_finite() {
        return Err("Rotation[1]: non-finite pitch".into());
    }
    let Some(Tag::Compound(fields)) = saved_field(&record.extra_fields, "BlockState")? else {
        return Err("BlockState: missing or not a compound".into());
    };
    let Some(Tag::String(name)) =
        saved_field(fields, "Name").map_err(|err| format!("BlockState.{err}"))?
    else {
        return Err("BlockState.Name: missing or not a string".into());
    };
    let name = Identifier::parse(name).map_err(|err| format!("BlockState.Name: {err}"))?;
    let block = registry
        .blocks
        .block(&name)
        .ok_or_else(|| format!("BlockState.Name: unknown block {name}"))?;
    let mut props = registry
        .blocks
        .by_id(block.default)
        .ok_or_else(|| "BlockState.Name: missing default state".to_owned())?
        .properties
        .clone();
    if let Some(properties) =
        saved_field(fields, "Properties").map_err(|err| format!("BlockState.{err}"))?
    {
        let Tag::Compound(properties) = properties else {
            return Err("BlockState.Properties: expected compound".into());
        };
        for (index, (key, tag)) in properties.iter().enumerate() {
            if properties[..index].iter().any(|(other, _)| other == key) {
                return Err(format!("BlockState.Properties.{key}: duplicate property"));
            }
            let Tag::String(value) = tag else {
                return Err(format!("BlockState.Properties.{key}: expected string"));
            };
            let property = props
                .iter_mut()
                .find(|(name, _)| name == key)
                .ok_or_else(|| format!("BlockState.Properties.{key}: unknown property"))?;
            let valid = block
                .properties
                .iter()
                .find(|(name, _)| name == key)
                .is_some_and(|(_, values)| values.contains(value));
            if !valid {
                return Err(format!(
                    "BlockState.Properties.{key}: invalid value {value}"
                ));
            }
            property.1 = value.clone();
        }
    }
    let block_state = registry
        .blocks
        .by_name_and_props(&name, &props)
        .ok_or_else(|| "BlockState.Properties: unresolved state".to_owned())?
        .0;
    if !gravity(&registry.blocks, block_state) {
        return Err("BlockState.Name: not a gravity block".into());
    }
    let age = match saved_field(&record.extra_fields, "Time")? {
        Some(Tag::Int(age)) if *age >= 0 => *age as u32,
        None => 0,
        _ => return Err("Time: expected nonnegative int".into()),
    };
    let touched_water = match saved_field(&record.extra_fields, "SolarisTouchedWater")? {
        Some(Tag::Byte(value @ (0 | 1))) => *value != 0,
        None => false,
        _ => return Err("SolarisTouchedWater: expected byte 0 or 1".into()),
    };
    Ok(Some(FallingState {
        block_state,
        age,
        touched_water,
    }))
}

#[cfg(test)]
#[path = "falling_tests.rs"]
mod tests;
