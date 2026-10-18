//! Native copper golems: construction, bounded chest sorting and weathering.
//! Vanilla CopperGolem saves weather_state (string), next_weather_age (long);
//! carried items/antenna use ordinary equipment (mainhand/saddle).
use crate::boundary::{BlockPos, EntityDataValue, EntityId, Hand};
use crate::entities::EntitySlot;
use crate::special_mobs::Retained;
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use mc_nbt::Tag;
use mc_physics::mth;
use mc_world::BlockStateId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

static COPPER_GOLEM_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| crate::piglin::type_id_of("minecraft:copper_golem"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopperState {
    pub weather: u8,
    /// -2 is waxed, -1 requests a fresh deadline; otherwise absolute game time.
    pub next_weather: i64,
}
impl Default for CopperState {
    fn default() -> Self {
        Self {
            weather: 0,
            next_weather: -1,
        }
    }
}
pub(crate) fn validate(value: &CopperState) -> Result<(), &'static str> {
    if value.weather > 3 {
        return Err("Copper.weather: expected 0..=3");
    }
    if value.next_weather < -2 {
        return Err("Copper.next_weather: expected -2, -1 or nonnegative game time");
    }
    Ok(())
}
pub(crate) fn from_fields(fields: &[(String, Tag)]) -> Result<CopperState, String> {
    let get = |name: &str| -> Result<Option<&Tag>, String> {
        let mut entries = fields.iter().filter(|(key, _)| key == name);
        let first = entries.next().map(|(_, tag)| tag);
        if entries.next().is_some() {
            return Err(format!("Copper.{name}: duplicate field"));
        }
        Ok(first)
    };
    let weather = match get("weather_state")? {
        None => 0,
        Some(Tag::String(name)) => crate::copper::AGES
            .iter()
            .position(|age| age == name)
            .ok_or("Copper.weather_state: expected unaffected, exposed, weathered or oxidized")?
            as u8,
        Some(_) => return Err("Copper.weather_state: expected string".into()),
    };
    let next_weather = match get("next_weather_age")? {
        None => -1,
        Some(Tag::Long(age)) => *age,
        Some(_) => return Err("Copper.next_weather_age: expected long".into()),
    };
    let value = CopperState {
        weather,
        next_weather,
    };
    validate(&value).map_err(str::to_owned)?;
    Ok(value)
}
pub(crate) fn to_fields(value: &CopperState) -> Vec<(String, Tag)> {
    vec![
        (
            "weather_state".into(),
            Tag::String(crate::copper::AGES[value.weather as usize].into()),
        ),
        ("next_weather_age".into(), Tag::Long(value.next_weather)),
    ]
}

/// Retain the loaded entity contract's geometry, including exact float widening.
pub(crate) fn extend_blueprints(
    types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<(u32, crate::blueprint::MobBlueprint)>,
) {
    use crate::blueprint::*;
    let name = Identifier::parse("minecraft:copper_golem").expect("id");
    let Some(facts) = types.facts_of(&name) else {
        return;
    };
    let id = facts.protocol_id;
    out.push((
        id,
        MobBlueprint {
            name: "minecraft:copper_golem",
            type_id: id,
            movement: MovementClass::Stationary,
            goals: &[],
            sensors: Sensors {
                sight_range: 32.0,
                attack_reach: 0.0,
            },
            params: MobParams {
                max_health: 12.0,
                speed: 3.0,
                attack_damage: 0.0,
                pickup_delay: 0,
                breed_cooldown: 0,
            },
            half_width: facts.dimensions.half_width(),
            height: facts.dimensions.height,
            offspring: None,
            breed_food: Vec::new(),
            loot_table: Some(Identifier::parse("minecraft:entities/copper_golem").expect("loot")),
            category: MobCategory::Misc,
            wool: false,
            spawn_weight: 0,
            burns_in_daylight: false,
            night_aggro_only: false,
        },
    ));
}

pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot, type_id: u32) {
    if mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
        .is_none_or(|entry| entry.name != "minecraft:copper_golem")
    {
        return;
    }
    debug_assert!(
        matches!(
            state.entities.body(slot).kind,
            crate::entities::EntityKind::Mob { type_id: kind }
            if kind == type_id
        ),
        "copper component implies the copper golem mob lane"
    );
    state
        .entities
        .row_mut(slot)
        .set_special_mob(Retained::Copper(CopperState::default()));
    state.entities.row_mut(slot).persistent = true;
    restore(state, slot);
}
pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    let Some(&Retained::Copper(copper)) = state.entities.row(slot).special_mob.as_deref() else {
        return;
    };
    if state.entities.row(slot).appearance.equipment[0]
        .as_ref()
        .is_some_and(|stack| !stack.is_empty())
    {
        state.entities.row_mut(slot).loot.drop_chances[0] = 2.0;
    }
    metadata(
        state,
        slot,
        16,
        EntityDataValue::WeatheringCopper(copper.weather),
    );
    metadata(state, slot, 17, EntityDataValue::CopperGolemState(0));
}
fn metadata(state: &mut TickState, slot: EntitySlot, index: u8, value: EntityDataValue) {
    let (body, row) = state.entities.parts_mut(slot);
    let extra = &mut Arc::make_mut(&mut row.appearance).extra;
    if let Some(entry) = extra.iter_mut().find(|(key, _)| *key == index) {
        if entry.1 == value {
            return;
        }
        entry.1 = value;
    } else {
        extra.push((index, value));
    }
    state.appearance_changed.push(body.id);
}
fn block(state: &TickState, pos: BlockPos) -> Option<&mc_world::BlockState> {
    state
        .registry
        .blocks
        .by_id(BlockStateId(state.world.block_at(pos.x, pos.y, pos.z)?))
}
fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}
fn centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    mth::length2(a[0] - b[0], a[2] - b[2]) + (a[1] - b[1]).abs()
}

/// Pumpkin placement consumes the pumpkin, transforms the body into a copper
/// chest of its oxidation/wax state, and creates one golem above that chest.
pub(crate) fn after_blocks(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>) {
    let end = writes.len();
    for index in 0..end {
        let (pos, id) = writes[index];
        if state.world.block_at(pos.x, pos.y, pos.z) != Some(id) {
            continue;
        }
        let Some(pumpkin) = state.registry.blocks.by_id(BlockStateId(id)) else {
            continue;
        };
        if !matches!(pumpkin.block.id.path(), "carved_pumpkin" | "jack_o_lantern") {
            continue;
        }
        let facing = pumpkin
            .properties
            .iter()
            .find(|(name, _)| name == "facing")
            .map(|(_, value)| value.as_str())
            .unwrap_or("north")
            .to_owned();
        let body = BlockPos {
            y: pos.y - 1,
            ..pos
        };
        let Some(body_block) = block(state, body) else {
            continue;
        };
        let path = body_block.block.id.path();
        let path = path.strip_prefix("waxed_").unwrap_or(path);
        let weather = match path {
            "copper_block" => 0,
            "exposed_copper" => 1,
            "weathered_copper" => 2,
            "oxidized_copper" => 3,
            _ => continue,
        };
        let waxed = body_block.block.id.path().starts_with("waxed_");
        let chest_id =
            Identifier::parse(crate::copper::name("copper_chest", weather, waxed)).expect("id");
        let Some(chest) = state.registry.blocks.by_name_and_props(
            &chest_id,
            &[
                ("facing".into(), facing),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ],
        ) else {
            continue;
        };
        let Some(type_id) = state
            .registry
            .entity_types
            .id_of(&Identifier::parse("minecraft:copper_golem").expect("id"))
        else {
            continue;
        };
        crate::redstone::set(state, writes, pos, state.registry.states.air);
        crate::redstone::set(state, writes, body, chest.0);
        state.block_entities.insert(
            key(body),
            crate::tick::BlockEntityState {
                slots: vec![ItemStack::EMPTY; 27],
                furnace: None,
            },
        );
        state.world.mark_block_entity_dirty(body);
        let id = state.spawn_mob(type_id, centre(pos), 12.0);
        let slot = state.entities.slot_of(id).expect("spawn");
        state
            .entities
            .row_mut(slot)
            .set_special_mob(Retained::Copper(CopperState {
                weather,
                next_weather: if waxed { -2 } else { -1 },
            }));
        restore(state, slot);
        state.hostile.spawned.push(id);
    }
}

/// Native serial owner, no AI parity promise. Transfers up to sixteen identical
/// items from copper storage to an empty ordinary chest or one containing that
/// item. Equipment is the custody record while travelling; both slot images
/// and the carrier image enter the same tick journal.
pub(crate) fn tick(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>) {
    let coppers: Vec<_> = COPPER_GOLEM_TYPE
        .iter()
        .flat_map(|&type_id| state.entities.mob_slots(type_id))
        .filter(|&slot| {
            matches!(
                state.entities.row(slot).special_mob.as_deref(),
                Some(Retained::Copper(_))
            )
        })
        .collect();
    if coppers.is_empty() {
        return;
    }
    let chests: Vec<BlockPos> = state
        .block_entities
        .keys()
        .filter_map(|&(x, y, z)| {
            let pos = BlockPos { x, y, z };
            let name = block(state, pos)?.block.id.path();
            (crate::copper::chest(name) || matches!(name, "chest" | "trapped_chest")).then_some(pos)
        })
        .collect();
    let mut removed = Vec::new();
    for slot in coppers {
        if !state.entities.body(slot).alive {
            continue;
        }
        let Some(&Retained::Copper(mut copper)) = state.entities.row(slot).special_mob.as_deref()
        else {
            continue;
        };
        let pos = state.entities.pos(slot);
        if !state.world.is_active(
            (pos[0].floor() as i32).div_euclid(16),
            (pos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        if copper.next_weather == -1 {
            copper.next_weather = state.tick.saturating_add(528000).min(i64::MAX as u64) as i64;
        } else if copper.next_weather >= 0
            && state.tick >= copper.next_weather as u64
            && copper.weather < 3
        {
            copper.weather += 1;
            copper.next_weather = if copper.weather == 3 {
                0
            } else {
                state.tick.saturating_add(528000).min(i64::MAX as u64) as i64
            };
        }
        state
            .entities
            .row_mut(slot)
            .set_special_mob(Retained::Copper(copper));
        metadata(
            state,
            slot,
            16,
            EntityDataValue::WeatheringCopper(copper.weather),
        );
        if copper.weather == 3 && copper.next_weather != -2 {
            let cell = BlockPos {
                x: pos[0].floor() as i32,
                y: pos[1].floor() as i32,
                z: pos[2].floor() as i32,
            };
            if block(state, cell).is_some_and(|block| {
                matches!(
                    block.block.id.path(),
                    "air" | "cave_air" | "void_air" | "water"
                )
            }) {
                statue(state, slot, cell, writes);
                removed.push(state.entities.body(slot).id);
                continue;
            }
        }
        if state.entities.row(slot).controls.no_ai {
            continue;
        }
        let carried = state.entities.row(slot).appearance.equipment[0]
            .clone()
            .unwrap_or(ItemStack::EMPTY);
        let source = carried.is_empty();
        let target = chests
            .iter()
            .copied()
            .filter(|cell| distance(pos, centre(*cell)) <= 32.0)
            .filter(|cell| {
                let Some(name) = block(state, *cell).map(|b| b.block.id.path()) else {
                    return false;
                };
                if source {
                    crate::copper::chest(name)
                        && state
                            .block_entities
                            .get(&key(*cell))
                            .is_some_and(|r| r.slots.iter().any(|s| !s.is_empty()))
                } else {
                    matches!(name, "chest" | "trapped_chest")
                        && destination(state, *cell, &carried).is_some()
                }
            })
            .min_by(|a, b| distance(pos, centre(*a)).total_cmp(&distance(pos, centre(*b))));
        let Some(target) = target else {
            move_to(state, slot, None);
            metadata(state, slot, 17, EntityDataValue::CopperGolemState(0));
            continue;
        };
        if distance(pos, centre(target)) > 1.8 {
            move_to(state, slot, Some(centre(target)));
            continue;
        }
        move_to(state, slot, None);
        if !state.tick.is_multiple_of(20) {
            continue;
        }
        if source {
            let record = state.block_entities.get_mut(&key(target)).expect("source");
            let Some(stack) = record.slots.iter_mut().find(|s| !s.is_empty()) else {
                continue;
            };
            let mut carried = stack.clone();
            carried.count = stack.count.min(16);
            stack.count -= carried.count;
            if stack.count == 0 {
                *stack = ItemStack::EMPTY;
            }
            Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] =
                Some(carried);
            state.entities.row_mut(slot).loot.drop_chances[0] = 2.0;
            metadata(state, slot, 17, EntityDataValue::CopperGolemState(1));
        } else if let Some((cell, index)) = destination(state, target, &carried) {
            let max = state.registry.item_rules().max_stack(&carried).min(99);
            let stack = &mut state
                .block_entities
                .get_mut(&key(cell))
                .expect("target")
                .slots[index];
            let moved = carried.count.min(if stack.is_empty() {
                max
            } else {
                max - stack.count
            });
            if stack.is_empty() {
                *stack = carried.clone();
                stack.count = moved;
            } else {
                stack.count += moved;
            }
            let mut remaining = carried;
            remaining.count -= moved;
            let has_cargo = !remaining.is_empty();
            let row = state.entities.row_mut(slot);
            Arc::make_mut(&mut row.appearance).equipment[0] = has_cargo.then_some(remaining);
            row.loot.drop_chances[0] = if has_cargo {
                2.0
            } else {
                crate::entity_loot::EntityLootState::default().drop_chances[0]
            };
            state.world.mark_block_entity_dirty(cell);
            metadata(state, slot, 17, EntityDataValue::CopperGolemState(3));
        }
        state.world.mark_block_entity_dirty(target);
        state.appearance_changed.push(state.entities.body(slot).id);
    }
    for id in removed {
        if let Some(slot) = state.entities.slot_of(id) {
            state.remove_entity(slot);
            state.hostile.removed.push(id);
        }
    }
}
fn destination(state: &TickState, pos: BlockPos, carried: &ItemStack) -> Option<(BlockPos, usize)> {
    let cells = crate::chests::storage_cells(&state.registry.blocks, pos, &|p| {
        state.world.block_at(p.x, p.y, p.z)
    });
    let empty = cells.iter().all(|p| {
        state
            .block_entities
            .get(&key(*p))
            .is_none_or(|r| r.slots.iter().all(ItemStack::is_empty))
    });
    let matches = cells.iter().any(|p| {
        state.block_entities.get(&key(*p)).is_some_and(|r| {
            r.slots
                .iter()
                .any(|s| mc_data::inventory_semantics_26_1_2::can_stack(s, carried))
        })
    });
    if !empty && !matches {
        return None;
    }
    let max = state.registry.item_rules().max_stack(carried).min(99);
    for cell in cells {
        if let Some(record) = state.block_entities.get(&key(cell))
            && let Some(index) = record.slots.iter().position(|s| {
                s.is_empty()
                    || (mc_data::inventory_semantics_26_1_2::can_stack(s, carried) && s.count < max)
            })
        {
            return Some((cell, index));
        }
    }
    None
}
fn move_to(state: &mut TickState, slot: EntitySlot, target: Option<[f64; 3]>) {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let horizontal = target
        .map(|p| crate::entity_motion::steer(crate::blueprint::MovementClass::Walker, pos, p, 3.0))
        .unwrap_or([0.0; 3]);
    let dt = mc_physics::TICK_SECONDS;
    let velocity = [
        horizontal[0],
        (state.entities.vel(slot)[1] - mc_physics::GRAVITY_BLOCKS_PER_SECOND_SQUARED * dt)
            .max(mc_physics::TERMINAL_VELOCITY_BLOCKS_PER_SECOND),
        horizontal[2],
    ];
    let sampler = crate::tick::TickSampler {
        world: &state.world,
        registry: &state.registry,
    };
    if let Some(fixed) = mc_physics::resolve_collision_displacement(
        &sampler,
        mc_physics::Vec3::new(pos[0], pos[1], pos[2]),
        mc_physics::Aabb {
            half_width: body.half_width,
            height: body.height,
        },
        mc_physics::Vec3::new(velocity[0] * dt, velocity[1] * dt, velocity[2] * dt),
        state.entities.ground(slot),
        mc_physics::STEP_HEIGHT,
    ) {
        state.entities.set_pos(
            slot,
            [
                pos[0] + fixed.delta.x,
                pos[1] + fixed.delta.y,
                pos[2] + fixed.delta.z,
            ],
        );
        state.entities.set_vel(
            slot,
            [
                fixed.delta.x / dt,
                if fixed.grounded {
                    0.0
                } else {
                    fixed.delta.y / dt
                },
                fixed.delta.z / dt,
            ],
        );
        state.entities.set_ground(slot, fixed.grounded);
        if horizontal[0] != 0.0 || horizontal[2] != 0.0 {
            state.entities.set_rot(
                slot,
                mth::atan2(-horizontal[0], horizontal[2]) as f32 * mth::RAD_TO_DEG,
                0.0,
            );
        }
    }
}
fn statue(
    state: &mut TickState,
    slot: EntitySlot,
    pos: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
) {
    let yaw = state.entities.rot(slot)[0];
    let facing = match ((yaw / 90.0).round() as i32).rem_euclid(4) {
        0 => "south",
        1 => "west",
        2 => "north",
        _ => "east",
    };
    let pose = crate::copper::POSES[(state.entities.body(slot).id.0.unsigned_abs() % 4) as usize];
    let id = Identifier::parse("minecraft:oxidized_copper_golem_statue").expect("id");
    let wet = block(state, pos).is_some_and(|block| {
        block.block.id.path() == "water"
            || block
                .properties
                .iter()
                .any(|(key, value)| key == "waterlogged" && value == "true")
    });
    let next = state
        .registry
        .blocks
        .by_name_and_props(
            &id,
            &[
                ("facing".into(), facing.into()),
                ("copper_golem_pose".into(), pose.into()),
                (
                    "waterlogged".into(),
                    if wet { "true" } else { "false" }.into(),
                ),
            ],
        )
        .expect("statue")
        .0;
    crate::redstone::set(state, writes, pos, next);
    let appearance = Arc::clone(&state.entities.row(slot).appearance);
    for item in appearance
        .equipment
        .iter()
        .flatten()
        .chain(appearance.saddle.iter())
    {
        state
            .merge_life_drops
            .push((state.entities.pos(slot), item.clone()));
    }
    let mut fields = vec![
        (
            "id".into(),
            Tag::String("minecraft:copper_golem_statue".into()),
        ),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ];
    if let Some(name) = &appearance.custom_name {
        fields.push((
            "components".into(),
            Tag::Compound(vec![(
                "minecraft:custom_name".into(),
                Tag::String(name.clone()),
            )]),
        ));
    }
    state.world.fold_overlay();
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("statue NBT");
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
}

/// WeatheringCopperGolemStatueBlock revives an unwaxed, unaffected statue
/// on the final axe use, retaining its native name and horizontal facing.
pub(crate) fn revive(state: &mut TickState, pos: BlockPos) -> bool {
    let Some(statue) = block(state, pos) else {
        return false;
    };
    let yaw = match statue
        .properties
        .iter()
        .find(|(key, _)| key == "facing")
        .map(|(_, value)| value.as_str())
    {
        Some("west") => 90.0,
        Some("north") => 180.0,
        Some("east") => 270.0,
        _ => 0.0,
    };
    let wet = statue
        .properties
        .iter()
        .any(|(key, value)| key == "waterlogged" && value == "true");
    let mut name = None;
    if let Some(bytes) = state
        .world
        .chunk(crate::boundary::ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos))
    {
        let Ok(Tag::Compound(fields)) = mc_nbt::read_network(&mut std::io::Cursor::new(bytes))
        else {
            return false;
        };
        if let Some((_, Tag::Compound(components))) =
            fields.iter().find(|(key, _)| key == "components")
            && let Some((_, Tag::String(value))) = components
                .iter()
                .find(|(key, _)| key == "minecraft:custom_name")
        {
            name = Some(value.clone());
        }
    }
    let Some(type_id) = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:copper_golem").expect("id"))
    else {
        return false;
    };
    let replacement = if wet {
        state.registry.states.water
    } else {
        state.registry.states.air
    };
    state.world.set_block(pos.x, pos.y, pos.z, replacement);
    state.merge_block_writes.push((pos, replacement));
    state.world.set_opaque_block_entity(pos, None);
    state.opaque_changes.push((pos, None));
    let id = state.spawn_mob(type_id, centre(pos), 12.0);
    let slot = state.entities.slot_of(id).expect("revived golem");
    state.entities.set_rot(slot, yaw, 0.0);
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name = name;
    state.hostile.spawned.push(id);
    true
}

pub(crate) fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
) -> bool {
    let Some(slot) = state.entities.slot_of(target) else {
        return false;
    };
    let Some(&Retained::Copper(mut copper)) = state.entities.row(slot).special_mob.as_deref()
    else {
        return false;
    };
    if !state.entities.body(slot).alive {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.spectator() {
        return false;
    }
    let index = if hand == Hand::Main {
        usize::from(player.inventory.selected)
    } else {
        crate::inventory::OFF_HAND
    };
    let held = player.inventory.slots[index].clone();
    let name = state
        .registry
        .items
        .name_of(held.item_id)
        .map(|id| id.path())
        .unwrap_or("");
    let mut consume = false;
    let mut tool_wear = None;
    if held.is_empty() && state.entities.row(slot).appearance.equipment[0].is_some() {
        if !crate::plugin_inventories::can_synchronize_entity_custody(state, slot) {
            crate::plugin::fail_native(state);
            return false;
        }
        let stack = Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0]
            .take()
            .expect("item");
        state.entities.row_mut(slot).loot.drop_chances[0] =
            crate::entity_loot::EntityLootState::default().drop_chances[0];
        state
            .merge_life_drops
            .push((state.entities.pos(slot), stack));
        crate::plugin_inventories::synchronize_entity_custody(state, slot)
            .expect("preflighted native copper equipment custody");
    } else if name == "shears" {
        let antenna = state.entities.row(slot).appearance.saddle.as_ref();
        if !antenna.is_some_and(|s| {
            state
                .registry
                .items
                .name_of(s.item_id)
                .is_some_and(|id| id.as_str() == "minecraft:poppy")
        }) {
            return false;
        }
        let Ok(plan) = crate::copper::prepare_tool_wear(state, actor, hand) else {
            return false;
        };
        tool_wear = plan;
        let item = Arc::make_mut(&mut state.entities.row_mut(slot).appearance)
            .saddle
            .take()
            .expect("antenna");
        state.entities.row_mut(slot).loot.saddle = None;
        state
            .merge_life_drops
            .push((state.entities.pos(slot), item));
    } else if name == "poppy" && state.entities.row(slot).appearance.saddle.is_none() {
        if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
            return false;
        }
        let mut antenna = held.clone();
        antenna.count = 1;
        state.entities.row_mut(slot).loot.saddle = Some(Box::new(antenna.clone()));
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance).saddle = Some(antenna);
        consume = true;
    } else if name == "honeycomb" && copper.next_weather != -2 {
        if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
            return false;
        }
        copper.next_weather = -2;
        consume = true;
    } else if name.ends_with("_axe") && (copper.next_weather == -2 || copper.weather > 0) {
        let Ok(plan) = crate::copper::prepare_tool_wear(state, actor, hand) else {
            return false;
        };
        tool_wear = plan;
        if copper.next_weather == -2 {
            copper.next_weather = -1;
        } else {
            copper.weather -= 1;
            copper.next_weather = -1;
        }
    } else {
        return false;
    }
    state
        .entities
        .row_mut(slot)
        .set_special_mob(Retained::Copper(copper));
    restore(state, slot);
    if consume {
        let player = state.players.get_mut(&actor).expect("player");
        let connection = player.connection;
        let effects = player.consume_held(hand, 1);
        for change in effects.changes {
            state.push_player(connection, change);
        }
    } else {
        crate::copper::apply_tool_wear(state, actor, tool_wear);
    }
    state.appearance_changed.push(target);
    true
}
