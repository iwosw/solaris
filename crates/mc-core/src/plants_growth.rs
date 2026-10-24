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
        // Vanilla kelp grows into sources and downward-flowing water (a
        // falling column with water above), turning the flow into a source
        // as it extends. Sideways spread with air above is not a column.
        if path(read, target)? != "water" {
            return None;
        }
        if prop(blocks, get(read, target)?, "level") != Some("0") {
            let roof = BlockPos { y: target.y + 1, ..target };
            if path(read, roof)? != "water" {
                return None;
            }
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
            // Walk the whole cultivated column: bone meal acts on the tip
            // however far it stands above the clicked segment.
            for _ in 0..512 {
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
        "azalea" | "flowering_azalea" => azalea_tree(read, at, name, seed),
        "mangrove_propagule" => mangrove_tree(read, at, seed),
        "crimson_fungus" | "warped_fungus" => huge_fungus(read, at, name),
        "moss_block" => moss_spread(read, at, seed),
        "crimson_nylium" | "warped_nylium" => nylium_foliage(read, at, name, seed),
        "netherrack" => nylium_conversion(read, at, seed),
        "sea_pickle" => sea_pickle_growth(read, at, id, seed),
        // Vanilla duplicates double flowers (except pitcher plants): the
        // plant is retained and one flower item drops. The drop spawns at
        // merge (`farming::accepted_use`); the plan itself edits nothing.
        "sunflower" | "lilac" | "rose_bush" | "peony" => {
            let lower = BlockPos { y: at.y - 1, ..at };
            let upper = BlockPos { y: at.y + 1, ..at };
            // Accept a click on either half of an intact plant.
            let half = prop(blocks, id, "half")?;
            let (root, other) = if half == "lower" {
                (at, upper)
            } else if half == "upper" {
                (lower, at)
            } else {
                return None;
            };
            if path(read, root)? != name || path(read, other)? != name {
                return None;
            }
            Some(Vec::new())
        }
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

/// Vanilla azalea fertilizer: an oak-like tree with azalea leaves. Needs a
/// small soil patch and open air above; schematic, not bit-exact.
fn azalea_tree(
    read: &Read<'_>,
    at: BlockPos,
    name: &str,
    seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let blocks = &read.state.registry.blocks;
    let mut roll = Roll::new(seed);
    let height = 4 + roll.below(3) as i32;
    for dy in 1..=height + 2 {
        if !air(read, BlockPos { y: at.y + dy, ..at }) {
            return None;
        }
    }
    let log = with_prop(blocks, named(read, "oak_log")?, "axis", "y")?;
    let leaves = if name == "flowering_azalea" {
        named(read, "flowering_azalea_leaves")?
    } else {
        named(read, "azalea_leaves")?
    };
    let mut edits = Vec::new();
    edits.push(edit(
        at,
        with_prop(blocks, named(read, "oak_log")?, "axis", "y")?,
    ));
    for dy in 1..=height {
        edits.push(edit(BlockPos { y: at.y + dy, ..at }, log));
    }
    for dy in (height - 2)..=(height + 1) {
        let radius: i32 = if dy >= height { 1 } else { 2 };
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx == 0 && dz == 0 && dy <= height {
                    continue;
                }
                if dx.abs() == radius && dz.abs() == radius && roll.below(2) == 0 {
                    continue;
                }
                let cell = BlockPos {
                    x: at.x + dx,
                    y: at.y + dy,
                    z: at.z + dz,
                };
                if air(read, cell) {
                    edits.push(edit(cell, leaves));
                }
            }
        }
    }
    Some(edits)
}

/// Vanilla mangrove propagule fertilizer: a small mangrove tree. Requires a
/// planted (non-hanging) propagule on soil with open air above.
fn mangrove_tree(read: &Read<'_>, at: BlockPos, seed: u64) -> Option<Vec<PlantBlockEdit>> {
    let blocks = &read.state.registry.blocks;
    if prop(blocks, get(read, at)?, "hanging") == Some("true") {
        return None;
    }
    let below = BlockPos { y: at.y - 1, ..at };
    if !matches!(
        path(read, below)?,
        "dirt"
            | "coarse_dirt"
            | "rooted_dirt"
            | "mud"
            | "muddy_mangrove_roots"
            | "moss_block"
            | "grass_block"
            | "podzol"
            | "mycelium"
            | "sand"
            | "clay"
    ) {
        return None;
    }
    let mut roll = Roll::new(seed);
    let height = 4 + roll.below(3) as i32;
    for dy in 1..=height + 1 {
        if !air(read, BlockPos { y: at.y + dy, ..at }) {
            return None;
        }
    }
    let log = with_prop(blocks, named(read, "mangrove_log")?, "axis", "y")?;
    let leaves = named(read, "mangrove_leaves")?;
    let mut edits = Vec::new();
    for dy in 0..=height {
        edits.push(edit(BlockPos { y: at.y + dy, ..at }, log));
    }
    for dy in (height - 2)..=height {
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                if dx.abs() == 2 && dz.abs() == 2 {
                    continue;
                }
                let cell = BlockPos {
                    x: at.x + dx,
                    y: at.y + dy,
                    z: at.z + dz,
                };
                if air(read, cell) {
                    edits.push(edit(cell, leaves));
                }
            }
        }
    }
    Some(edits)
}

/// Vanilla huge fungi: crimson/warped fungus on its own nylium grows a stem
/// column with a wart cap. Schematic heights, not bit-exact.
fn huge_fungus(read: &Read<'_>, at: BlockPos, name: &str) -> Option<Vec<PlantBlockEdit>> {
    let blocks = &read.state.registry.blocks;
    let (stem, cap, soil) = if name == "crimson_fungus" {
        ("crimson_stem", "nether_wart_block", "crimson_nylium")
    } else {
        ("warped_stem", "warped_wart_block", "warped_nylium")
    };
    let below = BlockPos { y: at.y - 1, ..at };
    if path(read, below)? != soil {
        return None;
    }
    let height = 5;
    for dy in 1..=height + 1 {
        if !air(read, BlockPos { y: at.y + dy, ..at }) {
            return None;
        }
    }
    let stem_id = with_prop(blocks, named(read, stem)?, "axis", "y")?;
    let cap_id = named(read, cap)?;
    let mut edits = Vec::new();
    for dy in 0..height {
        edits.push(edit(BlockPos { y: at.y + dy, ..at }, stem_id));
    }
    for dx in -1..=1 {
        for dz in -1..=1 {
            let cell = BlockPos {
                x: at.x + dx,
                y: at.y + height,
                z: at.z + dz,
            };
            if air(read, cell) {
                edits.push(edit(cell, cap_id));
            }
        }
    }
    Some(edits)
}

/// Vanilla moss fertilizer: neighboring stone/dirt-family converts to moss,
/// with moss carpet above where air allows.
fn moss_spread(read: &Read<'_>, at: BlockPos, seed: u64) -> Option<Vec<PlantBlockEdit>> {
    let mut roll = Roll::new(seed);
    let moss = named(read, "moss_block")?;
    let carpet = named(read, "moss_carpet")?;
    let mut edits = Vec::new();
    for dx in -2..=2 {
        for dz in -2..=2 {
            for dy in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                if roll.below(3) == 0 {
                    continue;
                }
                let cell = BlockPos {
                    x: at.x + dx,
                    y: at.y + dy,
                    z: at.z + dz,
                };
                if !matches!(
                    path(read, cell)?,
                    "stone"
                        | "dirt"
                        | "coarse_dirt"
                        | "rooted_dirt"
                        | "grass_block"
                        | "podzol"
                        | "gravel"
                        | "sand"
                        | "andesite"
                        | "diorite"
                        | "granite"
                        | "tuff"
                ) {
                    continue;
                }
                edits.push(edit(cell, moss));
                let above = BlockPos {
                    y: cell.y + 1,
                    ..cell
                };
                if air(read, above) && roll.below(4) == 0 {
                    edits.push(edit(above, carpet));
                }
                if edits.len() >= 12 {
                    return Some(edits);
                }
            }
        }
    }
    (!edits.is_empty()).then_some(edits)
}

/// Vanilla nylium fertilizer: roots, sprouts and fungi sprout on nearby
/// nylium with free air above.
fn nylium_foliage(
    read: &Read<'_>,
    at: BlockPos,
    name: &str,
    seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let mut roll = Roll::new(seed);
    let (roots, fungus) = if name == "crimson_nylium" {
        ("crimson_roots", "crimson_fungus")
    } else {
        ("warped_roots", "warped_fungus")
    };
    let mut edits = Vec::new();
    for _ in 0..8 {
        let cell = BlockPos {
            x: at.x + roll.below(7) as i32 - 3,
            y: at.y + 1,
            z: at.z + roll.below(7) as i32 - 3,
        };
        let soil = BlockPos {
            y: cell.y - 1,
            ..cell
        };
        if path(read, soil)? != name || !air(read, cell) {
            continue;
        }
        let pick = roll.below(10);
        let plant = if pick == 0 {
            fungus
        } else if pick <= 3 {
            "nether_sprouts"
        } else {
            roots
        };
        if let Some(id) = named(read, plant) {
            edits.push(edit(cell, id));
        }
        if edits.len() >= 4 {
            break;
        }
    }
    // Netherrack directly adjacent to nylium converts to that nylium.
    for dx in -1..=1 {
        for dz in -1..=1 {
            for dy in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let cell = BlockPos {
                    x: at.x + dx,
                    y: at.y + dy,
                    z: at.z + dz,
                };
                if path(read, cell)? == "netherrack"
                    && let Some(id) = named(read, name)
                {
                    edits.push(edit(cell, id));
                    return Some(edits);
                }
            }
        }
    }
    (!edits.is_empty()).then_some(edits)
}

/// Bone meal on netherrack beside nylium converts it to that nylium.
fn nylium_conversion(read: &Read<'_>, at: BlockPos, seed: u64) -> Option<Vec<PlantBlockEdit>> {
    let _ = seed;
    for dx in -1..=1 {
        for dz in -1..=1 {
            for dy in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let cell = BlockPos {
                    x: at.x + dx,
                    y: at.y + dy,
                    z: at.z + dz,
                };
                let found = path(read, cell)?;
                if (found == "crimson_nylium" || found == "warped_nylium")
                    && let Some(id) = named(read, found)
                {
                    return Some(vec![edit(at, id)]);
                }
            }
        }
    }
    None
}

/// Vanilla sea-pickle fertilizer: a waterlogged colony gains pickles up to
/// four, and spreads new pickles onto adjacent live coral.
fn sea_pickle_growth(
    read: &Read<'_>,
    at: BlockPos,
    id: u32,
    seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let blocks = &read.state.registry.blocks;
    if prop(blocks, id, "waterlogged") != Some("true") {
        return None;
    }
    let mut roll = Roll::new(seed);
    let pickles = prop(blocks, id, "pickles")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(1);
    if pickles < 4 && roll.below(3) != 0 {
        let next = with_prop(blocks, id, "pickles", &(pickles + 1).to_string())?;
        return Some(vec![edit(at, next)]);
    }
    // Spread onto adjacent live coral blocks with water above.
    let mut offsets = Vec::new();
    for dx in -2..=2 {
        for dz in -2..=2 {
            if dx == 0 && dz == 0 {
                continue;
            }
            offsets.push((dx, dz));
        }
    }
    // Deterministic rotation of the search order by seed.
    let len = offsets.len() as u64;
    offsets.rotate_left((seed % len) as usize);
    for (dx, dz) in offsets {
        let soil = BlockPos {
            x: at.x + dx,
            y: at.y - 1,
            z: at.z + dz,
        };
        let found = path(read, soil)?;
        if !matches!(
            found,
            "tube_coral_block"
                | "brain_coral_block"
                | "bubble_coral_block"
                | "fire_coral_block"
                | "horn_coral_block"
        ) {
            continue;
        }
        let cell = BlockPos { y: at.y, ..soil };
        let water = path(read, cell)?;
        if water != "water" {
            continue;
        }
        let mut single = with_prop(blocks, id, "pickles", "1")?;
        single = with_prop(blocks, single, "waterlogged", "true")?;
        return Some(vec![edit(cell, single)]);
    }
    // Colony already full with nowhere to spread: still a valid target.
    (pickles < 4).then(|| {
        with_prop(blocks, id, "pickles", &(pickles + 1).to_string())
            .map(|next| vec![edit(at, next)])
    })?
}

/// Pointed dripstone renewal: a down-tipped stalactite with a fluid source
/// above fills the cauldron below, and water-fed columns keep growing.
pub(super) fn dripstone(
    read: &Read<'_>,
    at: BlockPos,
    id: u32,
    seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let blocks = &read.state.registry.blocks;
    if prop(blocks, id, "vertical_direction") != Some("down")
        || !matches!(
            prop(blocks, id, "thickness"),
            Some("tip") | Some("tip_merge")
        )
    {
        return None;
    }
    let mut roll = Roll::new(seed);
    // Climb the stalactite column to the fluid held above it.
    let mut top = at.y;
    for _ in 0..16 {
        let above = BlockPos { y: top + 1, ..at };
        if path(read, above)? != "pointed_dripstone" {
            break;
        }
        top += 1;
    }
    let cap = BlockPos { y: top + 1, ..at };
    let fluid = match path(read, cap)? {
        "dripstone_block" => {
            let source = BlockPos { y: cap.y + 1, ..at };
            match path(read, source)? {
                "water" => Some(false),
                "lava" => Some(true),
                _ => None,
            }
        }
        "water" => Some(false),
        "lava" => Some(true),
        _ => None,
    }?;
    // The cauldron below the tip, across a clear gap.
    let mut cell = BlockPos { y: at.y - 1, ..at };
    loop {
        match path(read, cell)? {
            "air" | "water" => {
                cell.y -= 1;
                if at.y - cell.y > 8 {
                    return None;
                }
            }
            "cauldron" | "water_cauldron" | "lava_cauldron" => break,
            _ => return None,
        }
    }
    if fluid {
        if roll.below(6) != 0 {
            return grow_dripstone(read, at, id, false, &mut roll);
        }
        return fill_cauldron(read, cell, "lava", &mut roll);
    }
    if roll.below(16) < 6 {
        return fill_cauldron(read, cell, "water", &mut roll);
    }
    grow_dripstone(read, at, id, true, &mut roll)
}

/// Raise the cauldron at `cell` one level toward `fluid`.
fn fill_cauldron(
    read: &Read<'_>,
    cell: BlockPos,
    fluid: &str,
    roll: &mut Roll,
) -> Option<Vec<PlantBlockEdit>> {
    let _ = roll;
    let blocks = &read.state.registry.blocks;
    let current = get(read, cell)?;
    let entry = blocks.by_id(BlockStateId(current))?;
    let name = entry.block.id.path().to_owned();
    let level = entry
        .properties
        .iter()
        .find(|(key, _)| key == "level")
        .and_then(|(_, value)| value.parse::<u8>().ok())
        .unwrap_or(0);
    let target = if fluid == "lava" {
        "lava_cauldron"
    } else {
        "water_cauldron"
    };
    if name == "cauldron" {
        let id = mc_data::Identifier::parse(format!("minecraft:{target}")).ok()?;
        let one = blocks.by_name_and_props(&id, &[("level".into(), "1".into())])?;
        return Some(vec![edit(cell, one.0)]);
    }
    if name != target || level >= 3 {
        return None;
    }
    let id = mc_data::Identifier::parse(format!("minecraft:{target}")).ok()?;
    let next = blocks.by_name_and_props(&id, &[("level".into(), (level + 1).to_string())])?;
    Some(vec![edit(cell, next.0)])
}

/// Water-fed stalactite/stalagmite growth: extend the down tip into air, or
/// raise a tip-up stalagmite standing a few cells below.
fn grow_dripstone(
    read: &Read<'_>,
    at: BlockPos,
    id: u32,
    water: bool,
    roll: &mut Roll,
) -> Option<Vec<PlantBlockEdit>> {
    if !water || roll.below(8) != 0 {
        return None;
    }
    let blocks = &read.state.registry.blocks;
    let below = BlockPos { y: at.y - 1, ..at };
    if air(read, below) {
        let middle = with_prop(blocks, id, "thickness", "middle")?;
        let tip = with_prop(blocks, id, "thickness", "tip")?;
        return Some(vec![edit(at, middle), edit(below, tip)]);
    }
    // A stalagmite tip-up below a short gap grows upward toward the tip.
    for gap in 1..=4 {
        let cell = BlockPos {
            y: at.y - gap,
            ..at
        };
        let found = path(read, cell)?;
        if found != "pointed_dripstone" {
            return None;
        }
        let state = get(read, cell)?;
        if prop(blocks, state, "vertical_direction") == Some("up")
            && matches!(
                prop(blocks, state, "thickness"),
                Some("tip") | Some("tip_merge")
            )
        {
            let middle = with_prop(blocks, state, "thickness", "middle")?;
            let tip = with_prop(blocks, state, "thickness", "tip")?;
            let above = BlockPos {
                y: cell.y + 1,
                ..cell
            };
            if !air(read, above) {
                return None;
            }
            return Some(vec![edit(cell, middle), edit(above, tip)]);
        }
    }
    None
}

/// Budding amethyst renewal: exposed air/source-water faces grow small buds,
/// and existing buds advance toward harvestable clusters.
pub(super) fn amethyst(read: &Read<'_>, at: BlockPos, seed: u64) -> Option<Vec<PlantBlockEdit>> {
    let blocks = &read.state.registry.blocks;
    let mut roll = Roll::new(seed);
    let faces: [(&str, BlockPos); 6] = [
        ("down", BlockPos { y: at.y - 1, ..at }),
        ("up", BlockPos { y: at.y + 1, ..at }),
        ("north", BlockPos { z: at.z - 1, ..at }),
        ("south", BlockPos { z: at.z + 1, ..at }),
        ("west", BlockPos { x: at.x - 1, ..at }),
        ("east", BlockPos { x: at.x + 1, ..at }),
    ];
    // Advance one existing bud first (vanilla picks a random direction).
    let start = roll.below(6) as usize;
    for step in 0..6 {
        let (facing, cell) = faces[(start + step) % 6];
        let found = path(read, cell)?;
        let next = match found {
            "small_amethyst_bud" => Some("medium_amethyst_bud"),
            "medium_amethyst_bud" => Some("large_amethyst_bud"),
            "large_amethyst_bud" => Some("amethyst_cluster"),
            _ => None,
        };
        if let Some(stage) = next {
            if roll.below(5) != 0 {
                continue;
            }
            let current = get(read, cell)?;
            if prop(blocks, current, "facing") != Some(facing) {
                continue;
            }
            let mut grown = named(read, stage)?;
            grown = with_prop(blocks, grown, "facing", facing)?;
            let wet = prop(blocks, current, "waterlogged").unwrap_or("false");
            grown = with_prop(blocks, grown, "waterlogged", wet)?;
            return Some(vec![edit(cell, grown)]);
        }
        // An exposed air or source-water face can sprout a new small bud.
        if found != "air" && found != "water" {
            continue;
        }
        if found == "water" && prop(blocks, get(read, cell)?, "level") != Some("0") {
            continue;
        }
        if roll.below(5) != 0 {
            continue;
        }
        let mut bud = named(read, "small_amethyst_bud")?;
        bud = with_prop(blocks, bud, "facing", facing)?;
        bud = with_prop(
            blocks,
            bud,
            "waterlogged",
            if found == "water" { "true" } else { "false" },
        )?;
        return Some(vec![edit(cell, bud)]);
    }
    None
}
