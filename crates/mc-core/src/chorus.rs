//! Chorus random ticks and local support. Uses the farming read/preimage plan;
//! no additional world scan or periodic worker is needed.
use mc_data::Identifier;
use mc_world::plant_rules_26_1_2::{PlantBlockEdit, PlantBlockRead};
use mc_world::{BlockPos, BlockRegistry, BlockStateId};

fn path<'a>(
    blocks: &'a BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
) -> Option<&'a str> {
    Some(blocks.by_id(world.get_cached_block(pos)?)?.block.id.path())
}
fn sides(pos: BlockPos) -> [BlockPos; 4] {
    [
        BlockPos {
            x: pos.x - 1,
            ..pos
        },
        BlockPos {
            x: pos.x + 1,
            ..pos
        },
        BlockPos {
            z: pos.z - 1,
            ..pos
        },
        BlockPos {
            z: pos.z + 1,
            ..pos
        },
    ]
}
fn resolve(blocks: &BlockRegistry, name: &str, props: &[(&str, &str)]) -> Option<BlockStateId> {
    blocks.by_name_and_props(
        &Identifier::parse(format!("minecraft:{name}")).ok()?,
        &props
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn growth(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    current: BlockStateId,
    seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let state = blocks.by_id(current)?;
    let age: u8 = state
        .properties
        .iter()
        .find(|(k, _)| k == "age")?
        .1
        .parse()
        .ok()?;
    if age >= 5 {
        return None;
    }
    let above = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if path(blocks, world, above)? != "air" {
        return None;
    }
    let mut stem = 0;
    for dy in 1..=5 {
        if path(
            blocks,
            world,
            BlockPos {
                y: pos.y - dy,
                ..pos
            },
        )? != "chorus_plant"
        {
            break;
        }
        stem += 1;
    }
    let clear = |at| {
        sides(at)
            .into_iter()
            .all(|side| path(blocks, world, side) == Some("air"))
    };
    let plant = |up: bool, side: Option<usize>| {
        let mut props = [
            ("down", "true"),
            ("up", if up { "true" } else { "false" }),
            ("west", "false"),
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
        ];
        props[0].1 = if matches!(
            path(
                blocks,
                world,
                BlockPos {
                    y: pos.y - 1,
                    ..pos
                }
            )?,
            "chorus_plant" | "end_stone"
        ) {
            "true"
        } else {
            "false"
        };
        for (index, neighbour) in sides(pos).into_iter().enumerate() {
            if matches!(
                path(blocks, world, neighbour)?,
                "chorus_plant" | "chorus_flower"
            ) {
                props[index + 2].1 = "true";
            }
        }
        if let Some(side) = side {
            props[side + 2].1 = "true";
        }
        resolve(blocks, "chorus_plant", &props)
    };
    if stem < 4
        && clear(above)
        && path(
            blocks,
            world,
            BlockPos {
                y: pos.y + 2,
                ..pos
            },
        )? == "air"
    {
        return Some(vec![
            PlantBlockEdit {
                pos,
                new_state: plant(true, None)?,
            },
            PlantBlockEdit {
                pos: above,
                new_state: current,
            },
        ]);
    }
    // One branch per random tick; at most four probes, bounded by flower age.
    if age < 4 {
        for offset in 0..4 {
            let direction = (seed as usize + offset) % 4;
            let target = sides(pos)[direction];
            if path(blocks, world, target) != Some("air")
                || path(
                    blocks,
                    world,
                    BlockPos {
                        y: target.y - 1,
                        ..target
                    },
                ) != Some("air")
            {
                continue;
            }
            if !sides(target)
                .into_iter()
                .filter(|p| *p != pos)
                .all(|p| path(blocks, world, p) == Some("air"))
            {
                continue;
            }
            let next_age = (age + 1).to_string();
            return Some(vec![
                PlantBlockEdit {
                    pos,
                    new_state: plant(false, Some(direction))?,
                },
                PlantBlockEdit {
                    pos: target,
                    new_state: resolve(blocks, "chorus_flower", &[("age", &next_age)])?,
                },
            ]);
        }
    }
    Some(vec![PlantBlockEdit {
        pos,
        new_state: resolve(blocks, "chorus_flower", &[("age", "5")])?,
    }])
}

fn native_path(state: &crate::tick::TickState, at: crate::boundary::BlockPos) -> Option<&str> {
    Some(
        state
            .registry
            .blocks
            .by_id(BlockStateId(state.world.block_at(at.x, at.y, at.z)?))?
            .block
            .id
            .path(),
    )
}

pub(crate) fn supported(
    state: &crate::tick::TickState,
    at: crate::boundary::BlockPos,
    flower: bool,
) -> bool {
    let below = crate::boundary::BlockPos { y: at.y - 1, ..at };
    let Some(base) = native_path(state, below) else {
        return true;
    };
    if matches!(base, "end_stone" | "chorus_plant") {
        return true;
    }
    if base != "air" {
        return false;
    }
    let mut supports = 0;
    for side in crate::plants::horizontal(at) {
        let Some(path) = native_path(state, side) else {
            return true;
        };
        if path == "chorus_plant" {
            if flower
                || matches!(
                    native_path(
                        state,
                        crate::boundary::BlockPos {
                            y: side.y - 1,
                            ..side
                        }
                    ),
                    Some("chorus_plant" | "end_stone")
                )
            {
                supports += 1;
            }
        } else if flower && path != "air" {
            return false;
        }
    }
    if flower { supports == 1 } else { supports > 0 }
}

pub(crate) fn connections(
    state: &crate::tick::TickState,
    at: crate::boundary::BlockPos,
    id: u32,
) -> Option<u32> {
    if native_path(state, at)? != "chorus_plant" {
        return None;
    }
    let positions = crate::plants::neighbours(at);
    let names = ["west", "east", "north", "south", "down", "up"];
    let mut props = [("", "false"); 6];
    for i in 0..6 {
        let path = native_path(state, positions[i])?;
        props[i] = (
            names[i],
            if matches!(path, "chorus_plant" | "chorus_flower") || i == 4 && path == "end_stone" {
                "true"
            } else {
                "false"
            },
        );
    }
    let new = resolve(&state.registry.blocks, "chorus_plant", &props)?.0;
    (new != id).then_some(new)
}
