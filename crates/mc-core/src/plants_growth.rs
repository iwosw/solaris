//! Bounded random-sample vegetation rules. All reads join the native plan.
use super::*;
use crate::farming::{Roll, prop, with_prop};

fn get(read: &Read<'_>, at: BlockPos) -> Option<u32> {
    read.get_cached_block(old(at)).map(|id| id.0)
}

fn path<'a>(read: &'a Read<'_>, at: BlockPos) -> Option<&'a str> {
    Some(
        read.state
            .registry
            .blocks
            .by_id(BlockStateId(get(read, at)?))?
            .block
            .id
            .path(),
    )
}

fn named(read: &Read<'_>, name: &str) -> Option<u32> {
    let name = mc_data::Identifier::parse(format!("minecraft:{name}")).ok()?;
    Some(read.state.registry.blocks.block(&name)?.default.0)
}

fn edit(at: BlockPos, id: u32) -> PlantBlockEdit {
    PlantBlockEdit {
        pos: old(at),
        new_state: BlockStateId(id),
    }
}

fn air(read: &Read<'_>, at: BlockPos) -> bool {
    get(read, at) == Some(read.state.registry.states.air)
}

pub(super) fn random(
    read: &Read<'_>,
    at: BlockPos,
    id: u32,
    seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let mut roll = Roll::new(seed);
    match path(read, at)? {
        "kelp" | "cave_vines" | "weeping_vines" | "twisting_vines" => {
            let chance = if path(read, at)? == "kelp" { 7 } else { 10 };
            if roll.below(chance) != 0 {
                return None;
            }
            extend(read, at, id, false, &mut roll)
        }
        "vine" => vine(read, at, id, &mut roll),
        "brown_mushroom" | "red_mushroom" => mushroom_spread(read, at, id, &mut roll),
        _ => None,
    }
}

fn extend(
    read: &Read<'_>,
    at: BlockPos,
    id: u32,
    forced: bool,
    roll: &mut Roll,
) -> Option<Vec<PlantBlockEdit>> {
    let blocks = &read.state.registry.blocks;
    let name = path(read, at)?;
    let age = prop(blocks, id, "age")?.parse::<u8>().ok()?;
    if age >= 25 && !forced {
        return None;
    }
    let (body, dy) = match name {
        "kelp" => ("kelp_plant", 1),
        "cave_vines" => ("cave_vines_plant", -1),
        "weeping_vines" => ("weeping_vines_plant", -1),
        "twisting_vines" => ("twisting_vines_plant", 1),
        _ => return None,
    };
    let target = BlockPos { y: at.y + dy, ..at };
    if name == "kelp" {
        if path(read, target)? != "water" || prop(blocks, get(read, target)?, "level") != Some("0")
        {
            return None;
        }
    } else if !air(read, target) {
        return None;
    }
    let mut body_id = named(read, body)?;
    let mut tip = with_prop(
        blocks,
        id,
        "age",
        &age.saturating_add(1).min(25).to_string(),
    )?;
    if name == "cave_vines" {
        body_id = with_prop(blocks, body_id, "berries", prop(blocks, id, "berries")?)?;
        tip = with_prop(
            blocks,
            tip,
            "berries",
            if roll.below(9) == 0 { "true" } else { "false" },
        )?;
    }
    Some(vec![edit(at, body_id), edit(target, tip)])
}

fn mushroom_support(read: &Read<'_>, at: BlockPos) -> bool {
    let below = BlockPos { y: at.y - 1, ..at };
    match path(read, below) {
        Some("mycelium" | "podzol" | "crimson_nylium" | "warped_nylium") => true,
        Some(_) => {
            read.state.raw_brightness(at.x, at.y, at.z) < 13
                && get(read, below)
                    .is_some_and(|id| read.state.registry.light.suffocating(id) == Some(true))
        }
        None => false,
    }
}

fn mushroom_spread(
    read: &Read<'_>,
    at: BlockPos,
    id: u32,
    roll: &mut Roll,
) -> Option<Vec<PlantBlockEdit>> {
    if roll.below(25) != 0 {
        return None;
    }
    let mut count = 0;
    for dx in -4..=4 {
        for dz in -4..=4 {
            for dy in -1..=1 {
                if get(
                    read,
                    BlockPos {
                        x: at.x + dx,
                        y: at.y + dy,
                        z: at.z + dz,
                    },
                ) == Some(id)
                {
                    count += 1;
                    if count >= 5 {
                        return None;
                    }
                }
            }
        }
    }
    for _ in 0..4 {
        let target = BlockPos {
            x: at.x + roll.below(3) as i32 - 1,
            y: at.y + roll.below(3) as i32 - 1,
            z: at.z + roll.below(3) as i32 - 1,
        };
        if air(read, target) && mushroom_support(read, target) {
            return Some(vec![edit(target, id)]);
        }
    }
    None
}

fn vine(read: &Read<'_>, at: BlockPos, id: u32, roll: &mut Roll) -> Option<Vec<PlantBlockEdit>> {
    if roll.below(4) != 0 {
        return None;
    }
    let blocks = &read.state.registry.blocks;
    let below = BlockPos { y: at.y - 1, ..at };
    if !air(read, below) {
        return None;
    }
    let mut grown = with_prop(blocks, id, "up", "false")?;
    let mut any = false;
    for face in ["north", "south", "west", "east"] {
        let keep = prop(blocks, id, face) == Some("true") && roll.below(2) == 0;
        any |= keep;
        grown = with_prop(blocks, grown, face, if keep { "true" } else { "false" })?;
    }
    any.then(|| vec![edit(below, grown)])
}

pub(super) fn bonemeal(
    read: &Read<'_>,
    at: BlockPos,
    id: u32,
    seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let blocks = &read.state.registry.blocks;
    let name = path(read, at)?;
    match name {
        "cave_vines" | "cave_vines_plant" => {
            if prop(blocks, id, "berries") == Some("true") {
                return None;
            }
            Some(vec![edit(at, with_prop(blocks, id, "berries", "true")?)])
        }
        "kelp"
        | "kelp_plant"
        | "weeping_vines"
        | "weeping_vines_plant"
        | "twisting_vines"
        | "twisting_vines_plant" => {
            let dy = if name.starts_with("weeping") { -1 } else { 1 };
            let family = name.trim_end_matches("_plant");
            let mut tip = at;
            // Natural vines have at most 26 age stages. Cap the walk even for
            // command-built columns; missing/unreachable tips refuse whole.
            for _ in 0..26 {
                if path(read, tip)? == family {
                    return extend(read, tip, get(read, tip)?, true, &mut Roll::new(seed));
                }
                tip.y += dy;
            }
            None
        }
        "short_grass" | "fern" | "seagrass" => {
            let above = BlockPos { y: at.y + 1, ..at };
            if name == "seagrass" {
                if path(read, above)? != "water" {
                    return None;
                }
            } else if !air(read, above) {
                return None;
            }
            let tall = named(
                read,
                match name {
                    "short_grass" => "tall_grass",
                    "fern" => "large_fern",
                    _ => "tall_seagrass",
                },
            )?;
            Some(vec![
                edit(at, with_prop(blocks, tall, "half", "lower")?),
                edit(above, with_prop(blocks, tall, "half", "upper")?),
            ])
        }
        "rooted_dirt" => {
            let below = BlockPos { y: at.y - 1, ..at };
            air(read, below)
                .then(|| named(read, "hanging_roots").map(|id| vec![edit(below, id)]))?
        }
        "brown_mushroom" | "red_mushroom" => huge_mushroom(read, at, name, seed),
        _ => None,
    }
}

fn huge_mushroom(
    read: &Read<'_>,
    at: BlockPos,
    name: &str,
    seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let mut roll = Roll::new(seed);
    if roll.below(5) >= 2 {
        return None;
    }
    let below = BlockPos { y: at.y - 1, ..at };
    if !path(read, below).is_some_and(super::soil) {
        return None;
    }
    let height = 4 + roll.below(3) as i32;
    let stem = named(read, "mushroom_stem")?;
    let cap = named(
        read,
        if name == "brown_mushroom" {
            "brown_mushroom_block"
        } else {
            "red_mushroom_block"
        },
    )?;
    let mut edits = Vec::new();
    for dy in 0..height {
        edits.push(edit(BlockPos { y: at.y + dy, ..at }, stem));
    }
    let layers = if name == "brown_mushroom" { 1 } else { 4 };
    for layer in 0..layers {
        let radius: i32 = if name == "brown_mushroom" {
            3
        } else if layer == 0 {
            1
        } else {
            2
        };
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx.abs() == radius && dz.abs() == radius {
                    continue;
                }
                if layer > 0 && dx.abs() < radius && dz.abs() < radius {
                    continue;
                }
                edits.push(edit(
                    BlockPos {
                        x: at.x + dx,
                        y: at.y + height - layer,
                        z: at.z + dz,
                    },
                    cap,
                ));
            }
        }
    }
    for cell in &edits {
        let pos = native(cell.pos);
        if pos != at && !air(read, pos) {
            return None;
        }
    }
    Some(edits)
}
