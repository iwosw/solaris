//! End frame, podium and gateway block authority. Gateway links use vanilla NBT.
use crate::boundary::{BlockChange, BlockPos, ChangeSet, EntityId, Hand};
use crate::dimensions::Dimension;
use crate::portals::{named, state_id, write};
use crate::tick::TickState;
use mc_nbt::Tag;
use mc_physics::mth;

pub(crate) const PLATFORM: BlockPos = BlockPos {
    x: 100,
    y: 48,
    z: 0,
};
pub(crate) const EXIT: BlockPos = BlockPos { x: 0, y: 64, z: 0 };

pub(crate) fn exit_portal_position(_state: &TickState) -> [i32; 3] {
    [EXIT.x, EXIT.y, EXIT.z]
}

fn frame_eye(state: &TickState, pos: BlockPos, facing: &str) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| {
            block.block.id.path() == "end_portal_frame"
                && block
                    .properties
                    .iter()
                    .any(|(k, v)| k == "eye" && v == "true")
                && block
                    .properties
                    .iter()
                    .any(|(k, v)| k == "facing" && v == facing)
        })
}

pub(crate) fn activate(
    state: &mut TickState,
    eye: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
) -> bool {
    for dx in -2..=2 {
        for dz in -2..=2 {
            let center = BlockPos {
                x: eye.x + dx,
                y: eye.y,
                z: eye.z + dz,
            };
            if !(-1..=1).all(|n| {
                frame_eye(
                    state,
                    BlockPos {
                        x: center.x + n,
                        z: center.z - 2,
                        ..center
                    },
                    "south",
                ) && frame_eye(
                    state,
                    BlockPos {
                        x: center.x + n,
                        z: center.z + 2,
                        ..center
                    },
                    "north",
                ) && frame_eye(
                    state,
                    BlockPos {
                        x: center.x - 2,
                        z: center.z + n,
                        ..center
                    },
                    "east",
                ) && frame_eye(
                    state,
                    BlockPos {
                        x: center.x + 2,
                        z: center.z + n,
                        ..center
                    },
                    "west",
                )
            }) {
                continue;
            }
            let portal = state_id(state, "minecraft:end_portal", &[]);
            for x in -1..=1 {
                for z in -1..=1 {
                    write(
                        state,
                        writes,
                        BlockPos {
                            x: center.x + x,
                            z: center.z + z,
                            ..center
                        },
                        portal,
                    );
                }
            }
            return true;
        }
    }
    false
}

pub(crate) fn use_eye(state: &mut TickState, actor: EntityId, hand: Hand, pos: BlockPos) -> bool {
    if !named(state, pos, "end_portal_frame") {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let index = crate::tick::held_slot_index(player, hand);
    let stack = &player.inventory.slots[index];
    if state
        .registry
        .items
        .name_of(stack.item_id)
        .is_none_or(|item| item.path() != "ender_eye")
    {
        return false;
    }
    if player.spectator() || player.vitals.is_dead() {
        return true;
    }
    let current = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .expect("frame loaded");
    if state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))
        .is_some_and(|block| {
            block
                .properties
                .iter()
                .any(|(k, v)| k == "eye" && v == "true")
        })
    {
        return true;
    }
    let facing = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))
        .unwrap()
        .properties
        .iter()
        .find(|(k, _)| k == "facing")
        .map(|(_, v)| v.clone())
        .expect("frame facing");
    let next = state_id(
        state,
        "minecraft:end_portal_frame",
        &[("eye", "true"), ("facing", &facing)],
    );
    let mut writes = Vec::new();
    let before = state.players[&actor].inventory.slots[index].clone();
    write(state, &mut writes, pos, next);
    activate(state, pos, &mut writes);
    state.merge_block_writes.extend(writes);
    let player = state.players.get_mut(&actor).expect("player");
    if !player.creative() {
        mc_data::inventory_semantics_26_1_2::decrement_stack(&mut player.inventory.slots[index]);
    }
    let connection = player.connection;
    let update = player.inventory_contents();
    state.push_player(connection, update);
    crate::advancements::accepted_block(
        state,
        actor,
        "minecraft:item_used_on_block",
        pos,
        Some(next),
        Some(before),
    );
    true
}

pub(crate) fn platform(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    changes: &mut ChangeSet,
) {
    let obsidian = state_id(state, "minecraft:obsidian", &[]);
    for x in -2..=2 {
        for z in -2..=2 {
            for y in 0..=3 {
                let pos = BlockPos {
                    x: PLATFORM.x + x,
                    y: PLATFORM.y + y,
                    z: PLATFORM.z + z,
                };
                let next = if y == 0 {
                    obsidian
                } else {
                    state.registry.states.air
                };
                if let Some(previous) = state.world.block_at(pos.x, pos.y, pos.z)
                    && previous != next
                    && !named(state, pos, "air")
                    && !state.stage_command_break(pos, previous)
                {
                    continue;
                }
                write(state, writes, pos, next);
            }
        }
    }
    crate::portals::commit_removals(state, changes);
}

fn committed(state: &mut TickState, changes: &mut ChangeSet, writes: Vec<(BlockPos, u32)>) {
    state.world.fold_overlay();
    crate::portals::commit_removals(state, changes);
    after_blocks(state, &writes);
    changes.blocks.extend(
        writes
            .into_iter()
            .map(|(position, state)| BlockChange { position, state }),
    );
}

/// Fight owner calls at death/respawn boundaries; repeated calls do not rewrite.
pub(crate) fn set_exit_portal(state: &mut TickState, changes: &mut ChangeSet, active: bool) {
    if state.dimension != Dimension::End {
        return;
    }
    let bedrock = state_id(state, "minecraft:bedrock", &[]);
    let portal = state_id(state, "minecraft:end_portal", &[]);
    let stone = state_id(state, "minecraft:end_stone", &[]);
    let air = state.registry.states.air;
    let mut writes = Vec::new();
    for x in -3_i32..=3 {
        for z in -3_i32..=3 {
            if x * x + z * z > 12 {
                continue;
            }
            write(
                state,
                &mut writes,
                BlockPos {
                    x,
                    y: EXIT.y - 1,
                    z,
                },
                if x * x + z * z <= 6 { bedrock } else { stone },
            );
            for y in 0..=3 {
                let id = if (y == 0 && x * x + z * z > 6) || (x == 0 && z == 0) {
                    bedrock
                } else if y == 0 && active {
                    portal
                } else {
                    air
                };
                write(
                    state,
                    &mut writes,
                    BlockPos {
                        x,
                        y: EXIT.y + y,
                        z,
                    },
                    id,
                );
            }
        }
    }
    for (dx, dz, facing) in [
        (1, 0, "east"),
        (-1, 0, "west"),
        (0, 1, "south"),
        (0, -1, "north"),
    ] {
        let torch = state_id(state, "minecraft:wall_torch", &[("facing", facing)]);
        write(
            state,
            &mut writes,
            BlockPos {
                x: dx,
                y: EXIT.y + 2,
                z: dz,
            },
            torch,
        );
    }
    committed(state, changes, writes);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gateway {
    pub exit: Option<BlockPos>,
    pub exact: bool,
    pub age: i64,
    pub cooldown_until: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct GatewayAge {
    pub age: i64,
    pub stored_age: i64,
}

pub(crate) fn validate_gateways(state: &TickState) -> Result<(), String> {
    for chunk in state.world.chunks() {
        for pos in chunk.block_entities.keys() {
            if named(state, *pos, "end_gateway") {
                gateway(state, *pos)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn gateway(state: &TickState, pos: BlockPos) -> Result<Gateway, String> {
    let mut image = stored_gateway(state, pos)?;
    if let Some(clock) = state.gateway_ages.get(&pos) {
        image.age = clock.age;
    }
    Ok(image)
}

fn stored_gateway(state: &TickState, pos: BlockPos) -> Result<Gateway, String> {
    let fail = |message: &str| {
        format!(
            "{} end_gateway ({},{},{}): {message}",
            state.dimension.name(),
            pos.x,
            pos.y,
            pos.z
        )
    };
    let bytes = state
        .world
        .chunk(mc_world::ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos))
        .ok_or_else(|| fail("missing block entity"))?;
    let mut input = bytes.as_slice();
    let Tag::Compound(fields) =
        mc_nbt::read_network(&mut input).map_err(|error| fail(&format!("NBT: {error}")))?
    else {
        return Err(fail("NBT root must be TAG_Compound"));
    };
    if !input.is_empty() {
        return Err(fail("trailing NBT bytes"));
    }
    let find = |key: &str| -> Result<Option<&Tag>, String> {
        let mut entries = fields.iter().filter(|(name, _)| name == key);
        let found = entries.next().map(|(_, tag)| tag);
        if entries.next().is_some() {
            return Err(fail(&format!("duplicate {key}")));
        }
        Ok(found)
    };
    if !matches!(find("id")?,Some(Tag::String(id)) if id == "minecraft:end_gateway") {
        return Err(fail("id must be minecraft:end_gateway"));
    }
    for (key, value) in [("x", pos.x), ("y", pos.y), ("z", pos.z)] {
        if !matches!(find(key)?,Some(Tag::Int(v)) if *v == value) {
            return Err(fail(&format!("{key} coordinate mismatch/type")));
        }
    }
    let age = match find("Age")? {
        None => 0,
        Some(Tag::Long(age)) if *age >= 0 => *age,
        _ => return Err(fail("Age must be nonnegative TAG_Long")),
    };
    let exact = match find("ExactTeleport")? {
        None => false,
        Some(Tag::Byte(0)) => false,
        Some(Tag::Byte(1)) => true,
        _ => return Err(fail("ExactTeleport must be TAG_Byte 0/1")),
    };
    let cooldown_until = match find("solaris:CooldownUntil")? {
        None => 0,
        Some(Tag::Long(value)) if *value >= 0 => *value as u64,
        _ => return Err(fail("solaris:CooldownUntil must be nonnegative TAG_Long")),
    };
    let exit = match find("exit_portal")? {
        None => None,
        Some(Tag::IntArray(coords)) if coords.len() == 3 => {
            let pos = BlockPos {
                x: coords[0],
                y: coords[1],
                z: coords[2],
            };
            if pos.x.unsigned_abs() >= 30_000_000
                || pos.z.unsigned_abs() >= 30_000_000
                || !(0..256).contains(&pos.y)
            {
                return Err(fail("exit_portal outside End bounds"));
            }
            Some(pos)
        }
        _ => return Err(fail("exit_portal must be TAG_IntArray of length 3")),
    };
    Ok(Gateway {
        exit,
        exact,
        age,
        cooldown_until,
    })
}

fn store_gateway(state: &mut TickState, pos: BlockPos, image: Gateway, journal: bool) -> Tag {
    if !state
        .world
        .has_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16))
    {
        state.world.set_opaque_block_entity(pos, None);
        state.world.fold_overlay();
    }
    let mut fields = vec![
        ("id".into(), Tag::String("minecraft:end_gateway".into())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        ("Age".into(), Tag::Long(image.age)),
        ("ExactTeleport".into(), Tag::Byte(i8::from(image.exact))),
        (
            "solaris:CooldownUntil".into(),
            Tag::Long(image.cooldown_until as i64),
        ),
    ];
    if let Some(exit) = image.exit {
        fields.push((
            "exit_portal".into(),
            Tag::IntArray(vec![exit.x, exit.y, exit.z]),
        ));
    }
    let root = Tag::Compound(fields);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &root).expect("gateway NBT");
    if journal {
        state
            .world
            .set_opaque_block_entity(pos, Some(bytes.clone()));
        state.opaque_changes.push((pos, Some(bytes)));
    } else {
        state.world.set_opaque_block_entity(pos, Some(bytes));
    }
    root
}

pub(crate) fn save_gateway(state: &mut TickState, pos: BlockPos, image: Gateway) {
    let previous = stored_gateway(state, pos).ok();
    if previous == Some(image) {
        return;
    }
    let notify = previous.is_none_or(|previous| {
        previous.exit != image.exit
            || previous.exact != image.exact
            || previous.age < 200 && image.age >= 200
    });
    let cooldown_event = image.cooldown_until == state.tick.saturating_add(40)
        && previous.is_none_or(|previous| previous.cooldown_until != image.cooldown_until);
    let root = store_gateway(state, pos, image, true);
    state.gateway_ages.insert(
        pos,
        GatewayAge {
            age: image.age,
            stored_age: image.age,
        },
    );
    if !notify && !cooldown_event {
        return;
    }
    if notify {
        state.world.mark_block_entity_visible(pos);
    }
    if let Some(block) = state.world.block_at(pos.x, pos.y, pos.z) {
        let block_id = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(block))
            .expect("gateway state")
            .block
            .raw_id as u32;
        let viewers: Vec<_> = state
            .players
            .iter()
            .filter_map(|(id, player)| {
                let p = state.entities.pos(state.entities.slot_of(*id)?);
                ((p[0] - f64::from(pos.x)).abs() <= 64.0 && (p[2] - f64::from(pos.z)).abs() <= 64.0)
                    .then_some(player.connection)
            })
            .collect();
        for connection in viewers {
            if notify {
                state.push_player(
                    connection,
                    crate::boundary::PlayerChange::BlockEntityData {
                        position: pos,
                        block_entity_type: mc_data::block_entity_kinds::BlockEntityKind::EndGateway
                            .protocol_id(),
                        nbt: root.clone(),
                    },
                );
            }
            if cooldown_event {
                state.push_player(
                    connection,
                    crate::boundary::PlayerChange::BlockEvent {
                        position: pos,
                        block: block_id,
                        state: block,
                        action: 1,
                        parameter: 0,
                    },
                );
            }
        }
    }
}

fn gateway_blocks(state: &mut TickState, pos: BlockPos, writes: &mut Vec<(BlockPos, u32)>) {
    let bedrock = state_id(state, "minecraft:bedrock", &[]);
    let gateway = state_id(state, "minecraft:end_gateway", &[]);
    for x in -1_i32..=1 {
        for z in -1_i32..=1 {
            for y in -2_i32..=2 {
                let cell = BlockPos {
                    x: pos.x + x,
                    y: pos.y + y,
                    z: pos.z + z,
                };
                let id = if x == 0 && y == 0 && z == 0 {
                    gateway
                } else if y.abs() == 2 && x == 0 && z == 0 || y.abs() == 1 && (x == 0 || z == 0) {
                    bedrock
                } else {
                    state.registry.states.air
                };
                write(state, writes, cell, id);
            }
        }
    }
}

pub(crate) fn outer_exit(
    state: &mut TickState,
    origin: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
) -> BlockPos {
    let angle = mth::atan2(f64::from(origin.z), f64::from(origin.x));
    let mut x = (f64::from(mth::cos(angle)) * 1024.0).floor() as i32;
    let mut z = (f64::from(mth::sin(angle)) * 1024.0).floor() as i32;
    let stone = state_id(state, "minecraft:end_stone", &[]);
    let air = state.registry.states.air;
    // Search the already admitted destination neighbourhood before creating
    // an emergency island. This bounded search runs only on first gateway use.
    let mut landing = None;
    'search: for radius in 0_i32..=12 {
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx.abs().max(dz.abs()) != radius {
                    continue;
                }
                for y in (1..=237).rev() {
                    let cell = BlockPos {
                        x: x + dx,
                        y,
                        z: z + dz,
                    };
                    if state.world.block_at(cell.x, y, cell.z) == Some(stone)
                        && state.world.block_at(cell.x, y + 1, cell.z) == Some(air)
                        && state.world.block_at(cell.x, y + 2, cell.z) == Some(air)
                    {
                        x = cell.x;
                        z = cell.z;
                        landing = Some(y + 1);
                        break 'search;
                    }
                }
            }
        }
    }
    let y = landing.unwrap_or(75);
    if landing.is_none() {
        for dx in -7_i32..=7 {
            for dz in -7_i32..=7 {
                if dx * dx + dz * dz > 49 {
                    continue;
                }
                for dy in -3..=0 {
                    write(
                        state,
                        writes,
                        BlockPos {
                            x: x + dx,
                            y: y - 1 + dy,
                            z: z + dz,
                        },
                        stone,
                    );
                }
            }
        }
    }
    BlockPos { x, y, z }
}

/// Vanilla `EndDragonFight` gateway ring: `floor(96·cos/sin(2·(−π + π/20·i)))`
/// in `java.lang.Math`. Pinned so generated gateways match vanilla Ends.
const GATEWAY_RING: [(i8, i8); 20] = [
    (96, 0),
    (91, 29),
    (77, 56),
    (56, 77),
    (29, 91),
    (-1, 96),
    (-30, 91),
    (-57, 77),
    (-78, 56),
    (-92, 29),
    (-96, -1),
    (-92, -30),
    (-78, -57),
    (-57, -78),
    (-30, -92),
    (0, -96),
    (29, -92),
    (56, -78),
    (77, -57),
    (91, -30),
];

/// Inner gateway on the vanilla ring; its pair is generated atomically on entry.
pub(crate) fn create_gateway(state: &mut TickState, changes: &mut ChangeSet, index: u8) {
    if state.dimension != Dimension::End || index >= 20 {
        return;
    }
    let (x, z) = GATEWAY_RING[usize::from(index)];
    let inner = BlockPos {
        x: i32::from(x),
        y: 75,
        z: i32::from(z),
    };
    if named(state, inner, "end_gateway") {
        if let Err(message) = gateway(state, inner) {
            eprintln!("{message}");
        }
        return;
    }
    let mut writes = Vec::new();
    gateway_blocks(state, inner, &mut writes);
    committed(state, changes, writes);
    save_gateway(
        state,
        inner,
        Gateway {
            exit: None,
            exact: false,
            age: 0,
            cooldown_until: 0,
        },
    );
}

pub(crate) fn initialize_gateway(
    state: &mut TickState,
    pos: BlockPos,
    changes: &mut ChangeSet,
    mut image: Gateway,
) -> Gateway {
    let mut writes = Vec::new();
    let exit = outer_exit(state, pos, &mut writes);
    let pair = BlockPos {
        y: exit.y + 10,
        ..exit
    };
    gateway_blocks(state, pair, &mut writes);
    committed(state, changes, writes);
    save_gateway(
        state,
        pair,
        Gateway {
            exit: Some(pos),
            exact: false,
            age: 0,
            cooldown_until: 0,
        },
    );
    image.exit = Some(pair);
    save_gateway(state, pos, image);
    image
}

/// Install block entities after folding blocks, never overwrite corrupt NBT.
pub(crate) fn after_blocks(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    for &(pos, _) in writes {
        if !named(state, pos, "end_gateway") {
            state.gateway_ages.remove(&pos);
        }
        let chunk_pos = mc_world::ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        if state
            .world
            .chunk(chunk_pos)
            .is_none_or(|chunk| chunk.block_entities.contains_key(&pos))
        {
            continue;
        }
        if named(state, pos, "end_gateway") {
            save_gateway(
                state,
                pos,
                Gateway {
                    exit: None,
                    exact: false,
                    age: 0,
                    cooldown_until: 0,
                },
            );
        } else if named(state, pos, "end_portal") {
            let root = Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:end_portal".into())),
                ("x".into(), Tag::Int(pos.x)),
                ("y".into(), Tag::Int(pos.y)),
                ("z".into(), Tag::Int(pos.z)),
            ]);
            let mut bytes = Vec::new();
            mc_nbt::write_network(&mut bytes, &root).expect("End portal NBT");
            state
                .world
                .set_opaque_block_entity(pos, Some(bytes.clone()));
            state.opaque_changes.push((pos, Some(bytes)));
            state.world.mark_block_entity_visible(pos);
            let viewers: Vec<_> = state
                .players
                .values()
                .map(|player| player.connection)
                .collect();
            for connection in viewers {
                state.push_player(
                    connection,
                    crate::boundary::PlayerChange::BlockEntityData {
                        position: pos,
                        block_entity_type: mc_data::block_entity_kinds::BlockEntityKind::EndPortal
                            .protocol_id(),
                        nbt: root.clone(),
                    },
                );
            }
        }
    }
}

/// Age advances in memory; only gateway transitions enter the WAL.
pub(crate) fn tick(state: &mut TickState) {
    let gateways: Vec<_> = state
        .world
        .active_positions()
        .filter_map(|pos| state.world.chunk(pos))
        .flat_map(|chunk| chunk.block_entities.keys().copied())
        .filter(|pos| named(state, *pos, "end_gateway"))
        .collect();
    for pos in gateways {
        let Ok(mut image) = gateway(state, pos) else {
            continue;
        };
        let previous_age = image.age;
        image.age = image.age.saturating_add(1);
        let clock = state.gateway_ages.entry(pos).or_insert(GatewayAge {
            age: previous_age,
            stored_age: previous_age,
        });
        let changed_since_store = previous_age == clock.stored_age && image.age != previous_age;
        clock.age = image.age;
        if changed_since_store {
            state.world.mark_block_entity_dirty(pos);
        }
        if image.cooldown_until <= state.tick && image.age % 2400 == 0 {
            image.cooldown_until = state.tick.saturating_add(40);
            save_gateway(state, pos, image);
        } else if previous_age < 200 && image.age >= 200 {
            save_gateway(state, pos, image);
        }
    }
}

pub(crate) fn checkpoint(state: &mut TickState) {
    let positions: Vec<_> = state.gateway_ages.keys().copied().collect();
    for pos in positions {
        let Ok(image) = gateway(state, pos) else {
            continue;
        };
        store_gateway(state, pos, image, false);
        state
            .gateway_ages
            .get_mut(&pos)
            .expect("resident gateway clock")
            .stored_age = image.age;
    }
}
