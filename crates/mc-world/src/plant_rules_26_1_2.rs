use mc_data::Identifier;

use crate::{BlockPos, BlockRegistry, BlockState, BlockStateId};

/// Distance contributed by a neighbouring block when resolving leaf support.
#[must_use]
pub fn leaf_distance_from_state(blocks: &BlockRegistry, state: BlockStateId) -> u8 {
    let Some(state) = blocks.by_id(state) else {
        return 7;
    };
    let path = state.block.id.path();
    if path.ends_with("_log")
        || path.ends_with("_wood")
        || matches!(
            path,
            "crimson_stem"
                | "stripped_crimson_stem"
                | "warped_stem"
                | "stripped_warped_stem"
                | "crimson_hyphae"
                | "stripped_crimson_hyphae"
                | "warped_hyphae"
                | "stripped_warped_hyphae"
        )
    {
        return 0;
    }
    if !path.ends_with("_leaves") {
        return 7;
    }
    block_state_property(state, "distance")
        .and_then(|distance| distance.parse::<u8>().ok())
        .unwrap_or(7)
        .min(7)
}

pub trait PlantBlockRead {
    fn get_cached_block(&self, pos: BlockPos) -> Option<BlockStateId>;
    /// Baked combined light (sky+block, 0..15) at `pos`, or `None` when the
    /// chunk has no baked light. `None` reads as light (fail-open): tests and
    /// unbaked planning worlds have no light layers.
    fn combined_light_at(&self, _pos: BlockPos) -> Option<u8> {
        None
    }
}

/// Ground plants whose vanilla `BushBlock` survival requires soil below: they
/// pop when the block under them is removed. Column plants (sugar cane,
/// cactus, bamboo) use the vertical column rules instead; face-attached
/// vegetation (vine, glow lichen) and leaves survive by their own rules.
#[must_use]
pub fn is_ground_support_plant(path: &str) -> bool {
    if path.ends_with("_sapling") {
        return true;
    }
    matches!(
        path,
        // crops lose their soil when the supporting block is destroyed
        "wheat"
            | "carrots"
            | "potatoes"
            | "beetroots"
            | "nether_wart"
            | "melon_stem"
            | "pumpkin_stem"
            | "attached_melon_stem"
            | "attached_pumpkin_stem"
            | "sweet_berry_bush"
        // small flowers
        | "poppy"
            | "dandelion"
            | "golden_dandelion"
            | "blue_orchid"
            | "allium"
            | "azure_bluet"
            | "oxeye_daisy"
            | "cornflower"
            | "lily_of_the_valley"
            | "torchflower"
            | "wither_rose"
            | "open_eyeblossom"
            | "closed_eyeblossom"
            | "orange_tulip"
            | "red_tulip"
            | "pink_tulip"
            | "white_tulip"
            | "cactus_flower"
            // grass and bush family
            | "short_grass"
            | "short_dry_grass"
            | "tall_dry_grass"
            | "tall_grass"
            | "fern"
            | "large_fern"
            | "bush"
            | "firefly_bush"
            | "leaf_litter"
            | "dead_bush"
            // double flowers
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "pitcher_plant"
            // rooted and submerged soil plants (hanging_roots is ceiling-hung: excluded)
            | "crimson_roots"
            | "warped_roots"
            | "nether_sprouts"
            | "seagrass"
            | "tall_seagrass"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlantBlockEdit {
    pub pos: BlockPos,
    pub new_state: BlockStateId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlantItemDrop {
    pub item: Identifier,
    pub count: i32,
}

impl PlantItemDrop {
    fn new(item: &str, count: i32) -> Self {
        Self {
            item: Identifier::parse(item).expect("static identifier"),
            count,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlantHorizontalDirection {
    North,
    South,
    West,
    East,
}

pub fn vertical_plant_growth_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    state: BlockStateId,
    random_seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let current = blocks.by_id(state)?;
    if !matches!(current.block.id.path(), "sugar_cane" | "cactus" | "bamboo") {
        return None;
    }
    let plant_path = current.block.id.path();
    if plant_path == "bamboo"
        && block_state_property(current, "age").is_some()
        && block_state_property(current, "leaves").is_some()
        && block_state_property(current, "stage").is_some()
    {
        return bamboo_growth_edits(blocks, world, pos, state, random_seed);
    }
    let plant_state = blocks.block(&current.block.id).map(|block| block.default)?;
    let air = named_block_default(blocks, "minecraft:air")?;
    let above = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if world.get_cached_block(above)? != air {
        return None;
    }

    let mut bottom_y = pos.y;
    while same_block_at(
        blocks,
        world,
        plant_path,
        BlockPos {
            y: bottom_y - 1,
            ..pos
        },
    )? {
        bottom_y -= 1;
    }
    let support = BlockPos {
        y: bottom_y - 1,
        ..pos
    };
    if !vertical_plant_supported_base(blocks, world, plant_path, support, air) {
        return None;
    }

    let max_height = if plant_path == "bamboo" { 16 } else { 3 };
    if pos.y - bottom_y + 1 >= max_height {
        return None;
    }
    // Vanilla `SugarCaneBlock`/`CactusBlock.randomTick`: every random tick
    // ages the stalk by one and only age 15 extends it (and resets to 0), so
    // a stalk grows once per ~16 random ticks. Reduced registries without an
    // `age` property keep the single-tick growth.
    let age = block_state_property(current, "age").and_then(|age| age.parse::<u8>().ok());
    if let Some(age) = age
        && age < 15
    {
        let aged = sibling_state_with_property(blocks, current, "age", &(age + 1).to_string())?;
        return Some(vec![PlantBlockEdit {
            pos,
            new_state: aged,
        }]);
    }
    if plant_path == "cactus" && !cactus_growth_target_clear(world, above, air) {
        return None;
    }
    let mut edits = vec![PlantBlockEdit {
        pos: above,
        new_state: plant_state,
    }];
    if age.is_some() {
        edits.push(PlantBlockEdit {
            pos,
            new_state: sibling_state_with_property(blocks, current, "age", "0")?,
        });
    }
    Some(edits)
}

pub fn bamboo_sapling_growth_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    random_seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    if !random_seed.is_multiple_of(3) {
        return None;
    }
    let current = blocks.by_id(world.get_cached_block(pos)?)?;
    if current.block.id.path() != "bamboo_sapling" {
        return None;
    }
    let air = named_block_default(blocks, "minecraft:air")?;
    let above = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if world.get_cached_block(above)? != air {
        return None;
    }
    let bamboo = blocks.block(&Identifier::parse("minecraft:bamboo").ok()?)?;
    let bottom = bamboo_state_with_properties(blocks, bamboo.default, "0", "none", "0")?;
    let top = bamboo_state_with_properties(blocks, bamboo.default, "0", "small", "0")?;
    Some(vec![
        PlantBlockEdit {
            pos,
            new_state: bottom,
        },
        PlantBlockEdit {
            pos: above,
            new_state: top,
        },
    ])
}

fn bamboo_growth_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    state: BlockStateId,
    random_seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let current = blocks.by_id(state)?;
    if block_state_property(current, "stage") != Some("0") || !random_seed.is_multiple_of(3) {
        return None;
    }
    let air = named_block_default(blocks, "minecraft:air")?;
    let above = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if world.get_cached_block(above)? != air {
        return None;
    }

    let mut bottom_y = pos.y;
    while bottom_y > pos.y - 15
        && same_block_at(
            blocks,
            world,
            "bamboo",
            BlockPos {
                y: bottom_y - 1,
                ..pos
            },
        )?
    {
        bottom_y -= 1;
    }
    let height = pos.y - bottom_y + 1;
    if height >= 16 {
        return None;
    }

    let b1_pos = BlockPos {
        y: pos.y - 1,
        ..pos
    };
    let b2_pos = BlockPos {
        y: pos.y - 2,
        ..pos
    };
    let b1 = world
        .get_cached_block(b1_pos)
        .and_then(|state| blocks.by_id(state))
        .filter(|block| block.block.id.path() == "bamboo");
    let b2 = world
        .get_cached_block(b2_pos)
        .and_then(|state| blocks.by_id(state))
        .filter(|block| block.block.id.path() == "bamboo");
    let leaves = if b1
        .and_then(|block| block_state_property(block, "leaves"))
        .is_none_or(|leaves| leaves == "none")
    {
        "small"
    } else {
        "large"
    };
    let age = if block_state_property(current, "age") == Some("1") || b2.is_some() {
        "1"
    } else {
        "0"
    };
    let terminal_roll =
        height >= 11 && splitmix64(random_seed ^ 0x4241_4d42_4f4f_5354).is_multiple_of(4);
    let stage = if terminal_roll || height == 15 {
        "1"
    } else {
        "0"
    };

    let mut edits = Vec::new();
    for y in bottom_y..=pos.y {
        let existing_pos = BlockPos { y, ..pos };
        let existing = world.get_cached_block(existing_pos)?;
        let existing_state = blocks.by_id(existing)?;
        let desired_age = if age == "1" {
            "1"
        } else {
            block_state_property(existing_state, "age")?
        };
        let desired_leaves = if leaves == "large" && y == pos.y - 1 {
            "small"
        } else if leaves == "large" && y == pos.y - 2 {
            "none"
        } else {
            block_state_property(existing_state, "leaves")?
        };
        let desired = bamboo_state_with_properties(
            blocks,
            existing,
            desired_age,
            desired_leaves,
            block_state_property(existing_state, "stage")?,
        )?;
        if desired != existing {
            edits.push(PlantBlockEdit {
                pos: existing_pos,
                new_state: desired,
            });
        }
    }
    let bamboo = blocks.block(&current.block.id)?;
    edits.push(PlantBlockEdit {
        pos: above,
        new_state: bamboo_state_with_properties(blocks, bamboo.default, age, leaves, stage)?,
    });
    Some(edits)
}

fn bamboo_state_with_properties(
    blocks: &BlockRegistry,
    state: BlockStateId,
    age: &str,
    leaves: &str,
    stage: &str,
) -> Option<BlockStateId> {
    let current = blocks.by_id(state)?;
    let mut properties = current.properties.clone();
    for (name, value) in [("age", age), ("leaves", leaves), ("stage", stage)] {
        properties.iter_mut().find(|(key, _)| key == name)?.1 = value.to_string();
    }
    blocks.by_name_and_props(&current.block.id, &properties)
}

/// Read the small planned extension before the immutable world. This keeps a
/// two-block bone-meal growth in one atomic native footprint.
struct GrowthRead<'a, W> {
    world: &'a W,
    edits: Vec<PlantBlockEdit>,
}

impl<W: PlantBlockRead> PlantBlockRead for GrowthRead<'_, W> {
    fn get_cached_block(&self, pos: BlockPos) -> Option<BlockStateId> {
        self.edits
            .iter()
            .rev()
            .find(|edit| edit.pos == pos)
            .map(|edit| edit.new_state)
            .or_else(|| self.world.get_cached_block(pos))
    }

    fn combined_light_at(&self, pos: BlockPos) -> Option<u8> {
        self.world.combined_light_at(pos)
    }
}

fn bamboo_bonemeal_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let mut top = pos;
    // Bamboo's maximum natural height is sixteen. Never walk an unbounded
    // operator-built column in a gameplay action.
    for _ in 0..16 {
        let above = BlockPos {
            y: top.y + 1,
            ..top
        };
        if !same_block_at(blocks, world, "bamboo", above)? {
            break;
        }
        top = above;
    }
    let mut read = GrowthRead {
        world,
        edits: Vec::new(),
    };
    for step in 0..1 + (splitmix64(seed) & 1) {
        let id = read.get_cached_block(top)?;
        let roll = splitmix64(seed.wrapping_add(step)) / 3 * 3;
        let growth = if blocks.by_id(id)?.block.id.path() == "bamboo_sapling" {
            bamboo_sapling_growth_edits(blocks, &read, top, roll)
        } else {
            bamboo_growth_edits(blocks, &read, top, id, roll)
        };
        let Some(growth) = growth else { break };
        for edit in growth {
            if let Some(existing) = read.edits.iter_mut().find(|old| old.pos == edit.pos) {
                *existing = edit;
            } else {
                read.edits.push(edit);
            }
        }
        top.y += 1;
    }
    (!read.edits.is_empty()).then_some(read.edits)
}

fn vertical_plant_supported_base(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    path: &str,
    support: BlockPos,
    air: BlockStateId,
) -> bool {
    let Some(support_state) = world.get_cached_block(support) else {
        return false;
    };
    if support_state == air {
        return false;
    }
    let Some(support_path) = blocks
        .by_id(support_state)
        .map(|state| state.block.id.path())
    else {
        return false;
    };

    match path {
        "cactus" => matches!(support_path, "sand" | "red_sand" | "suspicious_sand"),
        "sugar_cane" => {
            supports_overworld_plant(support_path)
                && has_adjacent_sugar_cane_support(blocks, world, support)
        }
        "bamboo" => {
            supports_overworld_plant(support_path)
                || matches!(
                    support_path,
                    "bamboo" | "bamboo_sapling" | "gravel" | "suspicious_gravel"
                )
        }
        _ => false,
    }
}

fn supports_overworld_plant(path: &str) -> bool {
    matches!(
        path,
        "sand"
            | "red_sand"
            | "suspicious_sand"
            | "dirt"
            | "coarse_dirt"
            | "rooted_dirt"
            | "mud"
            | "muddy_mangrove_roots"
            | "moss_block"
            | "pale_moss_block"
            | "grass_block"
            | "podzol"
            | "mycelium"
    )
}

fn has_adjacent_sugar_cane_support(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .any(|(dx, dz)| {
            let side = BlockPos {
                x: pos.x + dx,
                z: pos.z + dz,
                ..pos
            };
            let Some(state) = world.get_cached_block(side) else {
                return false;
            };
            blocks.by_id(state).is_some_and(|state| {
                matches!(
                    state.block.id.path(),
                    "water" | "flowing_water" | "frosted_ice"
                )
            })
        })
}

fn cactus_growth_target_clear(
    world: &impl PlantBlockRead,
    pos: BlockPos,
    air: BlockStateId,
) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .all(|(dx, dz)| {
            let side = BlockPos {
                x: pos.x + dx,
                z: pos.z + dz,
                ..pos
            };
            matches!(world.get_cached_block(side), Some(found) if found == air)
        })
}

fn same_block_at(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    path: &str,
    pos: BlockPos,
) -> Option<bool> {
    let state = world.get_cached_block(pos)?;
    Some(
        blocks
            .by_id(state)
            .is_some_and(|found| found.block.id.path() == path),
    )
}

pub fn next_crop_growth_state(blocks: &BlockRegistry, state: BlockStateId) -> Option<BlockStateId> {
    let current = blocks.by_id(state)?;
    if !is_supported_age_crop(&current.block.id) {
        return None;
    }
    let age = block_state_property(current, "age")?.parse::<u8>().ok()?;
    sibling_state_with_property(blocks, current, "age", &age.checked_add(1)?.to_string())
}

/// Wiki growth gate for age crops (wheat/carrots/potatoes/beetroots/stems):
/// light >= 9 at the plant block, then a `1/(floor(25/points)+1)` chance
/// roll, where points = soil base (dry farmland 2 / hydrated 4) + the 8
/// neighbours (dry +0.25 / hydrated +0.75 each), halved when crowded (same
/// crop on a diagonal, or on both axes). Returns whether the random tick
/// may advance the crop; `random_seed` is the tick's own roll seed.
/// Nether wart and cocoa use independent one-in-ten/one-in-five gates.
pub fn crop_growth_gate_passes(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    state: BlockStateId,
    random_seed: u64,
) -> bool {
    let current = match blocks.by_id(state) {
        Some(current) => current,
        None => return false,
    };
    if !is_supported_age_crop(&current.block.id) {
        return false;
    }
    match current.block.id.path() {
        "nether_wart" => return random_seed.is_multiple_of(10),
        "cocoa" => return random_seed.is_multiple_of(5),
        "sweet_berry_bush" => {
            return random_seed.is_multiple_of(5)
                && world.combined_light_at(pos).is_none_or(|light| light >= 9);
        }
        _ => {}
    }
    // Wiki: crops need light >= 9 at the plant block. Missing baked light
    // (tests, unbaked planning) reads as light (fail-open).
    if world.combined_light_at(pos).is_some_and(|light| light < 9) {
        return false;
    }
    // Only farmland contributes; adjacent stone/air contributes nothing.
    let below = BlockPos {
        y: pos.y - 1,
        ..pos
    };
    let mut points = 1.0 + soil_points(blocks, world, below);
    // The 8 neighbours around the soil: dry +0.25, hydrated +0.75.
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            let neighbour = BlockPos {
                x: below.x + dx,
                y: below.y,
                z: below.z + dz,
            };
            points += soil_points(blocks, world, neighbour) / 4.0;
        }
    }
    // Crowding halves the points: same crop on a diagonal, or on both axes
    // (vanilla). Bounded: 8 soil reads above + 8 crop reads here.
    if crop_crowded(blocks, world, &current.block.id, pos) {
        points /= 2.0;
    }
    let divisor = (25.0_f64 / points).floor() as u64 + 1;
    random_seed.is_multiple_of(divisor)
}

fn crop_crowded(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    crop: &Identifier,
    pos: BlockPos,
) -> bool {
    let mut x_side = false;
    let mut z_side = false;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            let neighbour = BlockPos {
                x: pos.x + dx,
                y: pos.y,
                z: pos.z + dz,
            };
            if !same_crop_at(blocks, world, crop, neighbour) {
                continue;
            }
            if dx != 0 && dz != 0 {
                return true;
            } else if dx != 0 {
                x_side = true;
            } else {
                z_side = true;
            }
        }
    }
    x_side && z_side
}
fn soil_points(blocks: &BlockRegistry, world: &impl PlantBlockRead, soil: BlockPos) -> f64 {
    let Some(block) = world.get_cached_block(soil).and_then(|id| blocks.by_id(id)) else {
        return 0.0;
    };
    if block.block.id.path() != "farmland" {
        return 0.0;
    }
    if block_state_property(block, "moisture").is_some_and(|moisture| moisture != "0") {
        3.0
    } else {
        1.0
    }
}

fn same_crop_at(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    crop: &Identifier,
    pos: BlockPos,
) -> bool {
    world
        .get_cached_block(pos)
        .and_then(|state| blocks.by_id(state))
        .is_some_and(|found| found.block.id == *crop)
}

fn is_supported_age_crop(block: &Identifier) -> bool {
    matches!(
        block.as_str(),
        "minecraft:wheat"
            | "minecraft:carrots"
            | "minecraft:potatoes"
            | "minecraft:beetroots"
            | "minecraft:nether_wart"
            | "minecraft:melon_stem"
            | "minecraft:pumpkin_stem"
            | "minecraft:sweet_berry_bush"
            | "minecraft:cocoa"
    )
}

pub fn bonemeal_growth_edit(
    blocks: &BlockRegistry,
    pos: BlockPos,
    state: BlockStateId,
) -> Option<PlantBlockEdit> {
    bonemeal_crop_growth_edit(blocks, pos, state, 0)
}

fn bonemeal_crop_growth_edit(
    blocks: &BlockRegistry,
    pos: BlockPos,
    state: BlockStateId,
    seed: u64,
) -> Option<PlantBlockEdit> {
    let current = blocks.by_id(state)?;
    let (maximum_age, growth) = match current.block.id.as_str() {
        "minecraft:wheat"
        | "minecraft:carrots"
        | "minecraft:potatoes"
        | "minecraft:melon_stem"
        | "minecraft:pumpkin_stem" => (7, 2 + (splitmix64(seed ^ u64::from(state.0)) % 4) as u8),
        "minecraft:beetroots" | "minecraft:sweet_berry_bush" => (3, 1),
        "minecraft:cocoa" => (2, 1),
        _ => return None,
    };
    let age = block_state_property(current, "age")?.parse::<u8>().ok()?;
    if age >= maximum_age {
        return None;
    }
    let new_age = age.saturating_add(growth).min(maximum_age);
    const AGES: [&str; 8] = ["0", "1", "2", "3", "4", "5", "6", "7"];
    let new_state =
        sibling_state_with_property(blocks, current, "age", AGES[usize::from(new_age)])?;
    Some(PlantBlockEdit { pos, new_state })
}

pub fn bonemeal_growth_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    state: BlockStateId,
    tree_seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    if matches!(
        blocks.by_id(state)?.block.id.path(),
        "bamboo" | "bamboo_sapling"
    ) {
        return bamboo_bonemeal_edits(blocks, world, pos, tree_seed);
    }
    if let Some(edit) = bonemeal_crop_growth_edit(blocks, pos, state, tree_seed) {
        // A newly mature stem may bear fruit; an already mature stem is not
        // a valid bone-meal target and must wait for a natural random tick.
        if let Some(edits) = stem_fruit_edits(blocks, world, pos, edit.new_state) {
            return Some(edits);
        }
        return Some(vec![edit]);
    }
    if let Some(edits) = sapling_tree_edits(blocks, world, pos, state, tree_seed) {
        return Some(edits);
    }
    grass_block_bonemeal_edits(blocks, world, pos, state, tree_seed)
}

/// Vanilla 26.1 lets bone meal on `grass_block` roll a 45% attempt gate and then
/// scatter vegetation in a random walk around the clicked block. Solaris keeps
/// the gate and the ±3 neighbourhood reach but replaces the walk with
/// deterministic picks derived from the tree seed: candidate supports are only
/// grass_block/dirt-family soil with an air plant cell, the clicked grass_block
/// itself is never edited, and a failed attempt plans nothing.
pub fn grass_block_bonemeal_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    state: BlockStateId,
    tree_seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() != "minecraft:grass_block" {
        return None;
    }
    let air = named_block_default(blocks, "minecraft:air")?;
    let above = BlockPos {
        y: pos.y.checked_add(1)?,
        ..pos
    };
    if world.get_cached_block(above) != Some(air) {
        return None;
    }

    const GRASS_BONEMEAL_SALT: u64 = 0x4752_4153_5353_5F45; // "GRASS_SE"
    let attempt_seed = tree_seed ^ GRASS_BONEMEAL_SALT;
    if splitmix64(attempt_seed) % 100 >= 45 {
        return None;
    }

    let short_grass = named_block_default(blocks, "minecraft:short_grass")?;
    let poppy = named_block_default(blocks, "minecraft:poppy")?;
    let dandelion = named_block_default(blocks, "minecraft:dandelion")?;
    let tall_grass_lower = named_block_default(blocks, "minecraft:tall_grass")?;
    let tall_grass_upper =
        tree_state_with_props(blocks, "minecraft:tall_grass", &[("half", "upper")])?;

    let mut edits = Vec::new();
    let mut placed = Vec::new();
    for attempt in 0..8u64 {
        let stream = attempt_seed ^ attempt.rotate_left(19);
        let dx = (splitmix64(stream) % 7) as i32 - 3;
        let dz = (splitmix64(stream.rotate_left(17)) % 7) as i32 - 3;
        let dy = (splitmix64(stream.rotate_left(37)) % 5) as i32 - 3;
        let (Some(soil_x), Some(soil_y), Some(soil_z)) = (
            pos.x.checked_add(dx),
            pos.y.checked_add(dy),
            pos.z.checked_add(dz),
        ) else {
            continue;
        };
        let support = BlockPos {
            x: soil_x,
            y: soil_y,
            z: soil_z,
        };
        let Some(plant_y) = soil_y.checked_add(1) else {
            continue;
        };
        let plant_cell = BlockPos {
            x: soil_x,
            y: plant_y,
            z: soil_z,
        };
        if placed.contains(&plant_cell) {
            continue;
        }
        let supported = matches!(
            world.get_cached_block(support).and_then(|soil| blocks.by_id(soil)),
            Some(soil) if is_grass_scatter_support(soil.block.id.path())
        );
        if !supported || world.get_cached_block(plant_cell) != Some(air) {
            continue;
        }
        match splitmix64(stream.rotate_left(53)) % 10 {
            0 => {
                let Some(top_y) = plant_y.checked_add(1) else {
                    continue;
                };
                let top_cell = BlockPos {
                    x: soil_x,
                    y: top_y,
                    z: soil_z,
                };
                if world.get_cached_block(top_cell) != Some(air) {
                    continue;
                }
                placed.push(top_cell);
                edits.push(PlantBlockEdit {
                    pos: plant_cell,
                    new_state: tall_grass_lower,
                });
                edits.push(PlantBlockEdit {
                    pos: top_cell,
                    new_state: tall_grass_upper,
                });
            }
            1 => edits.push(PlantBlockEdit {
                pos: plant_cell,
                new_state: poppy,
            }),
            2 => edits.push(PlantBlockEdit {
                pos: plant_cell,
                new_state: dandelion,
            }),
            _ => edits.push(PlantBlockEdit {
                pos: plant_cell,
                new_state: short_grass,
            }),
        }
        placed.push(plant_cell);
    }

    if edits.is_empty() { None } else { Some(edits) }
}

fn is_grass_scatter_support(path: &str) -> bool {
    matches!(
        path,
        "grass_block" | "dirt" | "coarse_dirt" | "rooted_dirt" | "podzol" | "mycelium"
    )
}

pub fn stem_fruit_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    state: BlockStateId,
) -> Option<Vec<PlantBlockEdit>> {
    let current = blocks.by_id(state)?;
    let (fruit_name, attached_name) = stem_lifecycle_blocks(current.block.id.as_str())?;
    let age = block_state_property(current, "age")?.parse::<u8>().ok()?;
    if sibling_state_with_property(blocks, current, "age", &age.checked_add(1)?.to_string())
        .is_some()
    {
        return None;
    }

    let fruit_state = named_block_default(blocks, fruit_name)?;
    let attached = Identifier::parse(attached_name).expect("static identifier");
    let attached_default = blocks
        .block(&attached)
        .and_then(|block| blocks.by_id(block.default))?;
    let air = named_block_default(blocks, "minecraft:air")?;

    for (facing, dx, dz) in [
        ("north", 0, -1),
        ("south", 0, 1),
        ("west", -1, 0),
        ("east", 1, 0),
    ] {
        let fruit_pos = BlockPos {
            x: pos.x + dx,
            z: pos.z + dz,
            ..pos
        };
        if !matches!(world.get_cached_block(fruit_pos), Some(found) if found == air) {
            continue;
        }
        let soil = world.get_cached_block(BlockPos {
            y: fruit_pos.y - 1,
            ..fruit_pos
        })?;
        if !blocks.by_id(soil).is_some_and(|block| {
            matches!(
                block.block.id.path(),
                "farmland"
                    | "dirt"
                    | "grass_block"
                    | "coarse_dirt"
                    | "podzol"
                    | "mycelium"
                    | "rooted_dirt"
                    | "moss_block"
                    | "pale_moss_block"
                    | "mud"
                    | "muddy_mangrove_roots"
            )
        }) {
            continue;
        }
        let attached_state =
            sibling_state_with_property(blocks, attached_default, "facing", facing)?;
        return Some(vec![
            PlantBlockEdit {
                pos,
                new_state: attached_state,
            },
            PlantBlockEdit {
                pos: fruit_pos,
                new_state: fruit_state,
            },
        ]);
    }

    None
}

fn stem_lifecycle_blocks(stem: &str) -> Option<(&'static str, &'static str)> {
    match stem {
        "minecraft:melon_stem" => Some(("minecraft:melon", "minecraft:attached_melon_stem")),
        "minecraft:pumpkin_stem" => Some(("minecraft:pumpkin", "minecraft:attached_pumpkin_stem")),
        _ => None,
    }
}

pub fn sapling_tree_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    pos: BlockPos,
    state: BlockStateId,
    tree_seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let current = blocks.by_id(state)?;
    let sapling_name = current.block.id.as_str();
    let (log_name, leaves_name, base_height, random_height) = sapling_tree_blocks(sapling_name)?;

    let two_by_two = find_two_by_two_saplings(blocks, world, pos, sapling_name);
    if matches!(
        sapling_name,
        "minecraft:dark_oak_sapling" | "minecraft:pale_oak_sapling"
    ) && two_by_two.is_none()
    {
        return None;
    }

    let log = tree_state_with_props(blocks, log_name, &[("axis", "y")])?;
    let leaves = tree_leaves_state(blocks, leaves_name)?;
    let air = named_block_default(blocks, "minecraft:air")?;

    match block_state_property(current, "stage")? {
        "0" => {
            let staged = sibling_state_with_property(blocks, current, "stage", "1")?;
            return Some(vec![PlantBlockEdit {
                pos,
                new_state: staged,
            }]);
        }
        "1" => {}
        _ => return None,
    }

    if matches!(
        sapling_name,
        "minecraft:dark_oak_sapling" | "minecraft:pale_oak_sapling"
    ) {
        return dark_oak_two_by_two_edits(
            blocks,
            world,
            two_by_two?,
            log_name,
            leaves_name,
            base_height,
            tree_seed,
        );
    }

    if matches!(
        sapling_name,
        "minecraft:spruce_sapling" | "minecraft:jungle_sapling"
    ) && two_by_two.is_some()
    {
        return spruce_or_jungle_two_by_two_edits(
            blocks,
            world,
            two_by_two?,
            sapling_name,
            log_name,
            leaves_name,
            tree_seed,
        );
    }

    let tree_height = base_height + (tree_seed % (random_height + 1)) as i32;

    let mut edits = Vec::new();
    for dy in 0..tree_height {
        edits.push(PlantBlockEdit {
            pos: BlockPos {
                y: pos.y + dy,
                ..pos
            },
            new_state: log,
        });
    }

    for (layer, radius) in [(0, 1i32), (-1, 1), (-2, 2)] {
        let foliage_y = pos.y + tree_height + layer;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let corner =
                    dx.unsigned_abs() == radius as u32 && dz.unsigned_abs() == radius as u32;
                if corner
                    && (layer != 0
                        || ((tree_seed
                            ^ (dx as i64 as u64).rotate_left(17)
                            ^ (dz as i64 as u64).rotate_left(41))
                            & 1
                            != 0))
                {
                    continue;
                }
                if dx == 0 && dz == 0 && foliage_y < pos.y + tree_height {
                    continue;
                }
                edits.push(PlantBlockEdit {
                    pos: BlockPos {
                        x: pos.x + dx,
                        y: foliage_y,
                        z: pos.z + dz,
                    },
                    new_state: leaves,
                });
            }
        }
    }

    for edit in &edits {
        if edit.pos == pos {
            continue;
        }
        match world.get_cached_block(edit.pos) {
            Some(found) if tree_growth_can_replace(blocks, found, air) => {}
            Some(_) | None => return None,
        }
    }

    Some(edits)
}

fn find_two_by_two_saplings(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    clicked: BlockPos,
    sapling_name: &str,
) -> Option<[BlockPos; 4]> {
    for (dx, dz) in [(0, 0), (-1, 0), (0, -1), (-1, -1)] {
        let northwest = BlockPos {
            x: clicked.x.checked_add(dx)?,
            z: clicked.z.checked_add(dz)?,
            ..clicked
        };
        let square = [
            northwest,
            BlockPos {
                x: northwest.x.checked_add(1)?,
                ..northwest
            },
            BlockPos {
                z: northwest.z.checked_add(1)?,
                ..northwest
            },
            BlockPos {
                x: northwest.x.checked_add(1)?,
                z: northwest.z.checked_add(1)?,
                ..northwest
            },
        ];
        if square.iter().all(|position| {
            world
                .get_cached_block(*position)
                .and_then(|state| blocks.by_id(state))
                .is_some_and(|state| state.block.id.as_str() == sapling_name)
        }) {
            return Some(square);
        }
    }
    None
}

fn dark_oak_two_by_two_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    saplings: [BlockPos; 4],
    log_name: &str,
    leaves_name: &str,
    tree_height: i32,
    tree_seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let northwest = saplings[0];
    let log = tree_state_with_props(blocks, log_name, &[("axis", "y")])?;
    let leaves = tree_leaves_state(blocks, leaves_name)?;
    let air = named_block_default(blocks, "minecraft:air")?;
    let mut edits = Vec::new();

    for dy in 0..tree_height {
        for trunk in saplings {
            edits.push(PlantBlockEdit {
                pos: BlockPos {
                    y: northwest.y.checked_add(dy)?,
                    ..trunk
                },
                new_state: log,
            });
        }
    }

    for layer in [0, -1, -2] {
        let foliage_y = northwest.y.checked_add(tree_height)?.checked_add(layer)?;
        for dx in -1i32..=2 {
            for dz in -1i32..=2 {
                let outer_x = dx == -1 || dx == 2;
                let outer_z = dz == -1 || dz == 2;
                if outer_x
                    && outer_z
                    && (layer != 0
                        || ((tree_seed
                            ^ (dx as i64 as u64).rotate_left(17)
                            ^ (dz as i64 as u64).rotate_left(41))
                            & 1
                            != 0))
                {
                    continue;
                }
                if (0..=1).contains(&dx)
                    && (0..=1).contains(&dz)
                    && foliage_y < northwest.y + tree_height
                {
                    continue;
                }
                edits.push(PlantBlockEdit {
                    pos: BlockPos {
                        x: northwest.x.checked_add(dx)?,
                        y: foliage_y,
                        z: northwest.z.checked_add(dz)?,
                    },
                    new_state: leaves,
                });
            }
        }
    }

    for edit in &edits {
        if saplings.contains(&edit.pos) {
            continue;
        }
        match world.get_cached_block(edit.pos) {
            Some(found) if tree_growth_can_replace(blocks, found, air) => {}
            Some(_) | None => return None,
        }
    }

    Some(edits)
}

fn spruce_or_jungle_two_by_two_edits(
    blocks: &BlockRegistry,
    world: &impl PlantBlockRead,
    saplings: [BlockPos; 4],
    sapling_name: &str,
    log_name: &str,
    leaves_name: &str,
    tree_seed: u64,
) -> Option<Vec<PlantBlockEdit>> {
    let northwest = saplings[0];
    let log = tree_state_with_props(blocks, log_name, &[("axis", "y")])?;
    let leaves = tree_leaves_state(blocks, leaves_name)?;
    let air = named_block_default(blocks, "minecraft:air")?;

    // Vanilla 26.1.2 uses trunk placer parameters (13, 2, 14) for mega spruce
    // and (10, 2, 19) for mega jungle. Solaris keeps those height ranges but
    // uses its deterministic tree seed instead of vanilla's RandomSource.
    let (base_height, first_random, second_random) = match sapling_name {
        "minecraft:spruce_sapling" => (13, 2u64, 14u64),
        "minecraft:jungle_sapling" => (10, 2u64, 19u64),
        _ => return None,
    };
    let tree_height = base_height
        + (tree_seed % (first_random + 1)) as i32
        + (splitmix64(tree_seed ^ 0x4d45_4741_5f48_4549) % (second_random + 1)) as i32;
    let trunk_top = northwest.y.checked_add(tree_height)?;
    let mut edits = Vec::new();

    for dy in 0..tree_height {
        for trunk in saplings {
            edits.push(PlantBlockEdit {
                pos: BlockPos {
                    y: northwest.y.checked_add(dy)?,
                    ..trunk
                },
                new_state: log,
            });
        }
    }

    if sapling_name == "minecraft:spruce_sapling" {
        let crown_depth = 6 + (splitmix64(tree_seed ^ 0x5350_5255_4345_5f43) % 4) as i32;
        for depth in 0..=crown_depth {
            let radius = ((depth + 1) / 2).min(3);
            push_two_by_two_leaf_layer(
                &mut edits,
                northwest,
                trunk_top.checked_sub(depth)?,
                radius,
                leaves,
                tree_seed ^ depth as u64,
                trunk_top,
            )?;
        }
    } else {
        for (dy, radius) in [(1, 1), (0, 2), (-1, 2), (-2, 2), (-3, 1)] {
            push_two_by_two_leaf_layer(
                &mut edits,
                northwest,
                trunk_top.checked_add(dy)?,
                radius,
                leaves,
                tree_seed ^ (dy as i64 as u64).rotate_left(23),
                trunk_top,
            )?;
        }
    }

    for edit in &edits {
        if saplings.contains(&edit.pos) {
            continue;
        }
        match world.get_cached_block(edit.pos) {
            Some(found) if tree_growth_can_replace(blocks, found, air) => {}
            Some(_) | None => return None,
        }
    }

    Some(edits)
}

fn push_two_by_two_leaf_layer(
    edits: &mut Vec<PlantBlockEdit>,
    northwest: BlockPos,
    y: i32,
    radius: i32,
    leaves: BlockStateId,
    layer_seed: u64,
    trunk_top: i32,
) -> Option<()> {
    for dx in -radius..=radius + 1 {
        for dz in -radius..=radius + 1 {
            if y < trunk_top && (0..=1).contains(&dx) && (0..=1).contains(&dz) {
                continue;
            }
            let corner = (dx == -radius || dx == radius + 1) && (dz == -radius || dz == radius + 1);
            if corner
                && (splitmix64(
                    layer_seed
                        ^ (dx as i64 as u64).rotate_left(17)
                        ^ (dz as i64 as u64).rotate_left(41),
                ) & 1
                    != 0)
            {
                continue;
            }
            edits.push(PlantBlockEdit {
                pos: BlockPos {
                    x: northwest.x.checked_add(dx)?,
                    y,
                    z: northwest.z.checked_add(dz)?,
                },
                new_state: leaves,
            });
        }
    }
    Some(())
}

fn tree_growth_can_replace(blocks: &BlockRegistry, state: BlockStateId, air: BlockStateId) -> bool {
    state == air
        || blocks
            .by_id(state)
            .is_some_and(|found| is_supported_tree_replaceable(found.block.id.as_str()))
}

fn is_supported_tree_replaceable(block: &str) -> bool {
    matches!(
        block,
        "minecraft:acacia_leaves"
            | "minecraft:azalea_leaves"
            | "minecraft:birch_leaves"
            | "minecraft:cherry_leaves"
            | "minecraft:dark_oak_leaves"
            | "minecraft:flowering_azalea_leaves"
            | "minecraft:jungle_leaves"
            | "minecraft:mangrove_leaves"
            | "minecraft:oak_leaves"
            | "minecraft:pale_oak_leaves"
            | "minecraft:spruce_leaves"
            | "minecraft:allium"
            | "minecraft:azure_bluet"
            | "minecraft:blue_orchid"
            | "minecraft:closed_eyeblossom"
            | "minecraft:cornflower"
            | "minecraft:dandelion"
            | "minecraft:golden_dandelion"
            | "minecraft:lily_of_the_valley"
            | "minecraft:open_eyeblossom"
            | "minecraft:orange_tulip"
            | "minecraft:oxeye_daisy"
            | "minecraft:pink_tulip"
            | "minecraft:poppy"
            | "minecraft:red_tulip"
            | "minecraft:torchflower"
            | "minecraft:white_tulip"
            | "minecraft:wither_rose"
            | "minecraft:bush"
            | "minecraft:crimson_roots"
            | "minecraft:dead_bush"
            | "minecraft:fern"
            | "minecraft:firefly_bush"
            | "minecraft:glow_lichen"
            | "minecraft:hanging_roots"
            | "minecraft:large_fern"
            | "minecraft:leaf_litter"
            | "minecraft:lilac"
            | "minecraft:nether_sprouts"
            | "minecraft:pale_moss_carpet"
            | "minecraft:peony"
            | "minecraft:pitcher_plant"
            | "minecraft:rose_bush"
            | "minecraft:seagrass"
            | "minecraft:short_dry_grass"
            | "minecraft:short_grass"
            | "minecraft:sunflower"
            | "minecraft:tall_dry_grass"
            | "minecraft:tall_grass"
            | "minecraft:tall_seagrass"
            | "minecraft:vine"
            | "minecraft:water"
            | "minecraft:warped_roots"
    )
}

fn sapling_tree_blocks(sapling: &str) -> Option<(&'static str, &'static str, i32, u64)> {
    match sapling {
        "minecraft:oak_sapling" => Some(("minecraft:oak_log", "minecraft:oak_leaves", 4, 2)),
        "minecraft:birch_sapling" => Some(("minecraft:birch_log", "minecraft:birch_leaves", 5, 2)),
        "minecraft:spruce_sapling" => {
            Some(("minecraft:spruce_log", "minecraft:spruce_leaves", 4, 0))
        }
        "minecraft:jungle_sapling" => {
            Some(("minecraft:jungle_log", "minecraft:jungle_leaves", 4, 0))
        }
        "minecraft:acacia_sapling" => {
            Some(("minecraft:acacia_log", "minecraft:acacia_leaves", 4, 0))
        }
        "minecraft:dark_oak_sapling" => {
            Some(("minecraft:dark_oak_log", "minecraft:dark_oak_leaves", 4, 0))
        }
        "minecraft:cherry_sapling" => {
            Some(("minecraft:cherry_log", "minecraft:cherry_leaves", 5, 2))
        }
        "minecraft:pale_oak_sapling" => {
            Some(("minecraft:pale_oak_log", "minecraft:pale_oak_leaves", 6, 2))
        }
        _ => None,
    }
}

fn tree_state_with_props(
    blocks: &BlockRegistry,
    name: &str,
    props: &[(&str, &str)],
) -> Option<BlockStateId> {
    let id = Identifier::parse(name).expect("static identifier");
    let props = props
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(&id, &props)
        .or_else(|| blocks.block(&id).map(|block| block.default))
}

fn tree_leaves_state(blocks: &BlockRegistry, name: &str) -> Option<BlockStateId> {
    tree_state_with_props(
        blocks,
        name,
        &[
            ("distance", "1"),
            ("persistent", "false"),
            ("waterlogged", "false"),
        ],
    )
    .or_else(|| {
        tree_state_with_props(
            blocks,
            name,
            &[
                ("distance", "1"),
                ("persistent", "true"),
                ("waterlogged", "false"),
            ],
        )
    })
    .or_else(|| {
        tree_state_with_props(
            blocks,
            name,
            &[("persistent", "true"), ("waterlogged", "false")],
        )
    })
}

pub fn sweet_berry_harvest(
    blocks: &BlockRegistry,
    pos: BlockPos,
    state: BlockStateId,
) -> Option<(PlantBlockEdit, PlantItemDrop)> {
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() != "minecraft:sweet_berry_bush" {
        return None;
    }
    let age = block_state_property(current, "age")?.parse::<u8>().ok()?;
    if age < 2 {
        return None;
    }

    let harvested_state = sibling_state_with_property(blocks, current, "age", "1")?;
    Some((
        PlantBlockEdit {
            pos,
            new_state: harvested_state,
        },
        PlantItemDrop::new("minecraft:sweet_berries", i32::from(age - 1)),
    ))
}

pub fn plant_drop_stacks(block: &BlockState) -> Option<Vec<PlantItemDrop>> {
    const WHEAT_MATURE_DROPS: &[(&str, i32)] =
        &[("minecraft:wheat", 1), ("minecraft:wheat_seeds", 1)];
    const WHEAT_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:wheat_seeds", 1)];
    const CARROT_MATURE_DROPS: &[(&str, i32)] = &[("minecraft:carrot", 2)];
    const CARROT_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:carrot", 1)];
    const POTATO_MATURE_DROPS: &[(&str, i32)] = &[("minecraft:potato", 2)];
    const POTATO_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:potato", 1)];
    const BEETROOT_MATURE_DROPS: &[(&str, i32)] =
        &[("minecraft:beetroot", 1), ("minecraft:beetroot_seeds", 1)];
    const BEETROOT_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:beetroot_seeds", 1)];
    const NETHER_WART_MATURE_DROPS: &[(&str, i32)] = &[("minecraft:nether_wart", 2)];
    const NETHER_WART_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:nether_wart", 1)];
    const COCOA_MATURE_DROPS: &[(&str, i32)] = &[("minecraft:cocoa_beans", 3)];
    const COCOA_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:cocoa_beans", 1)];

    let (mature_age, mature_drops, immature_drops) = match block.block.id.as_str() {
        "minecraft:wheat" => (7, WHEAT_MATURE_DROPS, WHEAT_IMMATURE_DROPS),
        "minecraft:carrots" => (7, CARROT_MATURE_DROPS, CARROT_IMMATURE_DROPS),
        "minecraft:potatoes" => (7, POTATO_MATURE_DROPS, POTATO_IMMATURE_DROPS),
        "minecraft:beetroots" => (3, BEETROOT_MATURE_DROPS, BEETROOT_IMMATURE_DROPS),
        "minecraft:nether_wart" => (3, NETHER_WART_MATURE_DROPS, NETHER_WART_IMMATURE_DROPS),
        "minecraft:cocoa" => (2, COCOA_MATURE_DROPS, COCOA_IMMATURE_DROPS),
        _ => return None,
    };

    let age = block_state_property(block, "age")?.parse::<u8>().ok()?;
    let drops = if age >= mature_age {
        mature_drops
    } else {
        immature_drops
    };
    Some(
        drops
            .iter()
            .map(|(item, count)| PlantItemDrop::new(item, *count))
            .collect(),
    )
}

pub fn is_cocoa_beans_item(item: &Identifier) -> bool {
    item.as_str() == "minecraft:cocoa_beans"
}

pub fn cocoa_state_for_use_on(
    clicked_state: BlockStateId,
    direction: PlantHorizontalDirection,
    blocks: &BlockRegistry,
) -> Option<BlockStateId> {
    let facing = cocoa_facing_for_direction(direction)?;
    let clicked = blocks.by_id(clicked_state)?;
    if !matches!(clicked.block.id.as_str(), "minecraft:jungle_log") {
        return None;
    }
    let cocoa = Identifier::parse("minecraft:cocoa").expect("static identifier");
    blocks.by_name_and_props(
        &cocoa,
        &[
            ("age".to_string(), "0".to_string()),
            ("facing".to_string(), facing.to_string()),
        ],
    )
}

fn cocoa_facing_for_direction(direction: PlantHorizontalDirection) -> Option<&'static str> {
    match direction {
        PlantHorizontalDirection::North => Some("north"),
        PlantHorizontalDirection::South => Some("south"),
        PlantHorizontalDirection::West => Some("west"),
        PlantHorizontalDirection::East => Some("east"),
    }
}

fn named_block_default(blocks: &BlockRegistry, name: &str) -> Option<BlockStateId> {
    blocks
        .block(&Identifier::parse(name).expect("static identifier"))
        .map(|block| block.default)
}

fn block_state_property<'a>(state: &'a BlockState, name: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value.as_str()))
}

fn sibling_state_with_property(
    blocks: &BlockRegistry,
    state: &BlockState,
    name: &str,
    value: &str,
) -> Option<BlockStateId> {
    let mut props = state.properties.clone();
    let (_, current) = props.iter_mut().find(|(key, _)| key == name)?;
    *current = value.to_string();
    blocks.by_name_and_props(&state.block.id, &props)
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}
