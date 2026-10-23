//! Vanilla block placement states, roughly `BlockItem.getPlacementState`:
//! the placed block's properties follow the clicked face, the cursor and the
//! placer's look, standing blocks become their wall variant on a side face,
//! a slab clicked from the open side doubles, and doors and beds claim their
//! second cell. Pure: the caller reads the world through closures.

use mc_data::Identifier;
use mc_physics::mth;
use mc_world::{Block, BlockRegistry, BlockStateId};

use crate::boundary::BlockPos;

/// Horizontal directions in vanilla `Direction.from2DDataValue` order.
const HORIZONTAL: [Face; 4] = [Face::South, Face::West, Face::North, Face::East];

/// A block face / direction (wire face codes 0..=5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Face {
    #[must_use]
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Down,
            2 => Self::North,
            3 => Self::South,
            4 => Self::West,
            5 => Self::East,
            _ => Self::Up,
        }
    }

    #[must_use]
    pub fn offset(self) -> [i32; 3] {
        match self {
            Self::Down => [0, -1, 0],
            Self::Up => [0, 1, 0],
            Self::North => [0, 0, -1],
            Self::South => [0, 0, 1],
            Self::West => [-1, 0, 0],
            Self::East => [1, 0, 0],
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::North => "north",
            Self::South => "south",
            Self::West => "west",
            Self::East => "east",
        }
    }

    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
        }
    }

    pub(crate) fn clockwise(self) -> Self {
        match self {
            Self::North => Self::East,
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
            vertical => vertical,
        }
    }

    pub(crate) fn counter_clockwise(self) -> Self {
        self.clockwise().opposite()
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "down" => Self::Down,
            "up" => Self::Up,
            "north" => Self::North,
            "south" => Self::South,
            "west" => Self::West,
            "east" => Self::East,
            _ => return None,
        })
    }

    pub(crate) fn horizontal(self) -> bool {
        !matches!(self, Self::Down | Self::Up)
    }

    fn axis(self) -> &'static str {
        match self {
            Self::Down | Self::Up => "y",
            Self::North | Self::South => "z",
            Self::West | Self::East => "x",
        }
    }
}

/// What the placer clicked and how it looks.
#[derive(Debug, Clone, Copy)]
pub struct Click {
    /// The clicked block.
    pub clicked: BlockPos,
    /// The clicked face.
    pub face: Face,
    /// Hit point inside the clicked block, 0..1 per axis.
    pub cursor: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// The placer's secondary use is active (sneaking).
    pub sneaking: bool,
}

impl Click {
    /// Vanilla `getHorizontalDirection`: where the placer faces.
    fn facing(&self) -> Face {
        HORIZONTAL[((f64::from(self.yaw) / 90.0 + 0.5).floor() as i64).rem_euclid(4) as usize]
    }

    /// Vanilla `getNearestLookingDirection`: the axis the look vector
    /// leans on most.
    fn looking(&self) -> Face {
        let (yaw, pitch) = (self.yaw.to_radians(), self.pitch.to_radians());
        let look = [
            -mth::sin(f64::from(yaw)) * mth::cos(f64::from(pitch)),
            -mth::sin(f64::from(pitch)),
            mth::cos(f64::from(yaw)) * mth::cos(f64::from(pitch)),
        ];
        let [x, y, z] = look.map(f32::abs);
        if y >= x && y >= z {
            if look[1] < 0.0 { Face::Down } else { Face::Up }
        } else if x >= z {
            if look[0] < 0.0 {
                Face::West
            } else {
                Face::East
            }
        } else if look[2] < 0.0 {
            Face::North
        } else {
            Face::South
        }
    }

    /// The clicked half: the top half on a face's upper part or from below.
    fn upper_half(&self) -> bool {
        match self.face {
            Face::Down => true,
            Face::Up => false,
            _ => self.cursor[1] > 0.5,
        }
    }
}

/// World reads the placement needs.
pub struct Reads<'a> {
    pub block_at: &'a dyn Fn(BlockPos) -> Option<u32>,
    /// Whether a placement may overwrite this state (air, fluid, plants).
    pub replaceable: &'a dyn Fn(u32) -> bool,
    pub water_source: u32,
}

/// The cells a placement writes, the first one the clicked target; `None`
/// when the block cannot go there.
#[must_use]
pub fn place(
    blocks: &BlockRegistry,
    item_block: &Block,
    click: &Click,
    reads: &Reads<'_>,
) -> Option<Vec<(BlockPos, u32)>> {
    let id = &item_block.id;
    // A slab clicked from its open side doubles in place.
    let clicked_state = (reads.block_at)(click.clicked)?;
    if is_slab(item_block)
        && let Some(double) = doubled_slab(blocks, item_block, clicked_state, |slab_type| {
            (slab_type == "bottom" && click.face == Face::Up)
                || (slab_type == "top" && click.face == Face::Down)
        })
    {
        return Some(vec![(click.clicked, double)]);
    }
    let offset = click.face.offset();
    // Replaceable vegetation is clicked in-place; liquids remain an
    // adjacent-target click (bucket handling owns source collection).
    let liquid = blocks
        .by_id(BlockStateId(clicked_state))
        .is_some_and(|state| matches!(state.block.id.path(), "water" | "lava"));
    let target = if (reads.replaceable)(clicked_state) && !liquid {
        click.clicked
    } else {
        BlockPos {
            x: click.clicked.x + offset[0],
            y: click.clicked.y + offset[1],
            z: click.clicked.z + offset[2],
        }
    };
    let current = (reads.block_at)(target)?;
    if is_slab(item_block)
        && let Some(double) = doubled_slab(blocks, item_block, current, |_| true)
    {
        return Some(vec![(target, double)]);
    }
    if !(reads.replaceable)(current) {
        return None;
    }
    // Hanging signs use the ceiling variant below and the bar-ended wall
    // variant on other faces; ordinary standing blocks cannot hang below.
    let (block, wall) = if id.path().ends_with("_hanging_sign") {
        if click.face == Face::Down {
            (item_block, false)
        } else {
            (wall_variant(blocks, id)?, true)
        }
    } else {
        match (click.face, wall_variant(blocks, id)) {
            (face, Some(wall)) if face.horizontal() => (wall, true),
            (Face::Down, Some(_)) => return None,
            _ => (item_block, false),
        }
    };
    let path = block.id.path();
    let mut props = blocks.by_id(block.default)?.properties.clone();
    if matches!(
        path,
        "rail" | "powered_rail" | "detector_rail" | "activator_rail"
    ) {
        set(
            &mut props,
            "shape",
            if click.facing().axis() == "x" {
                "east_west"
            } else {
                "north_south"
            },
        );
    }
    let has = |name: &str| block.properties.iter().any(|(key, _)| key == name);
    let allows = |name: &str, value: &str| {
        block
            .properties
            .iter()
            .any(|(key, values)| key == name && values.iter().any(|v| v == value))
    };
    if path.ends_with("_leaves") {
        set(&mut props, "persistent", "true");
    }

    if has("axis") && allows("axis", "y") {
        set(&mut props, "axis", click.face.axis());
    }
    if has("orientation") {
        let front = click.looking().opposite();
        let top = match front {
            Face::Down => click.facing().opposite(),
            Face::Up => click.facing(),
            _ => Face::Up,
        };
        set(
            &mut props,
            "orientation",
            &format!("{}_{}", front.name(), top.name()),
        );
    }
    let attached = has("face");
    if attached {
        set(
            &mut props,
            "face",
            match click.face {
                Face::Up => "floor",
                Face::Down => "ceiling",
                _ => "wall",
            },
        );
    }
    if has("facing") {
        let facing = if allows("facing", "up") {
            match path {
                "observer" => click.looking(),
                _ if path.ends_with("shulker_box")
                    || path.ends_with("_rod")
                    || path.contains("amethyst") =>
                {
                    click.face
                }
                _ => click.looking().opposite(),
            }
        } else if allows("facing", "down") {
            // Hoppers face into the block they were placed against.
            match click.face.opposite() {
                Face::Up => Face::Down,
                face => face,
            }
        } else if path.ends_with("_wall_hanging_sign") {
            let yaw = click.yaw.to_radians();
            let look = [-mth::sin(f64::from(yaw)), mth::cos(f64::from(yaw))];
            let mut directions = [Face::North, Face::South, Face::West, Face::East];
            let score = |face: Face| {
                let [x, _, z] = face.offset();
                x as f32 * look[0] + z as f32 * look[1]
            };
            directions.sort_unstable_by(|a, b| score(*b).total_cmp(&score(*a)));
            let mut selected = None;
            for looking in directions {
                if looking.axis() == click.face.axis() {
                    continue;
                }
                let facing = looking.opposite();
                set(&mut props, "facing", facing.name());
                let candidate = blocks.by_name_and_props(&block.id, &props)?;
                if hanging_supported(blocks, target, candidate.0, reads.block_at) == Some(true) {
                    selected = Some(facing);
                    break;
                }
            }
            selected?
        } else if wall || path == "ladder" {
            click.face
        } else if path == "cocoa" {
            if !click.face.horizontal() {
                return None;
            }
            click.face.opposite()
        } else if attached {
            if click.face.horizontal() {
                click.face
            } else {
                click.facing()
            }
        } else if path.ends_with("_trapdoor") {
            if click.face.horizontal() {
                click.face
            } else {
                click.facing().opposite()
            }
        } else if path.ends_with("anvil") {
            click.facing().clockwise()
        } else if path.ends_with("_stairs")
            || path.ends_with("_door")
            || path.ends_with("_fence_gate")
            || path.ends_with("_bed")
            || path.ends_with("campfire")
        {
            click.facing()
        } else {
            click.facing().opposite()
        };
        set(&mut props, "facing", facing.name());
    }
    if path.ends_with("_trapdoor") {
        let top = if click.face.horizontal() {
            click.cursor[1] > 0.5
        } else {
            click.face == Face::Down
        };
        set(&mut props, "half", if top { "top" } else { "bottom" });
    } else if allows("half", "top") {
        set(
            &mut props,
            "half",
            if click.upper_half() { "top" } else { "bottom" },
        );
    }
    if is_slab(block) {
        set(
            &mut props,
            "type",
            if click.upper_half() { "top" } else { "bottom" },
        );
    }
    if has("rotation") {
        let mut yaw = f64::from(click.yaw);
        if path.ends_with("_hanging_sign") {
            use mc_data::block_facts::{SturdyFace, has_full_sturdy_face};
            let above_id = (reads.block_at)(step(target, Face::Up))?;
            let above = blocks.by_id(BlockStateId(above_id))?;
            let mut middle = click.sneaking
                || !has_full_sturdy_face(
                    above_id,
                    &above.block.id,
                    &above.properties,
                    SturdyFace::Down,
                );
            if !click.sneaking && above.block.id.path().ends_with("_hanging_sign") {
                let axis = above
                    .properties
                    .iter()
                    .find_map(|(key, value)| match key.as_str() {
                        "facing" => Face::from_name(value).map(Face::axis),
                        "rotation" => {
                            value
                                .parse::<u8>()
                                .ok()
                                .and_then(|rotation| match rotation % 8 {
                                    0 => Some("z"),
                                    4 => Some("x"),
                                    _ => None,
                                })
                        }
                        _ => None,
                    });
                if axis == Some(click.facing().axis()) {
                    middle = false;
                }
            }
            set(
                &mut props,
                "attached",
                if middle { "true" } else { "false" },
            );
            if !middle {
                yaw = HORIZONTAL.iter().position(|face| *face == click.facing())? as f64 * 90.0;
            }
        }
        let segment = ((yaw + 180.0) * 16.0 / 360.0 + 0.5).floor() as i64;
        set(&mut props, "rotation", &segment.rem_euclid(16).to_string());
    }
    if has("waterlogged") {
        set(
            &mut props,
            "waterlogged",
            if current == reads.water_source {
                "true"
            } else {
                "false"
            },
        );
    }
    if path.ends_with("_door") {
        set(
            &mut props,
            "hinge",
            door_hinge(blocks, target, click, reads.block_at),
        );
    }
    // A chest beside a single chest of its kind joins it into a double.
    let mut partner = None;
    if has("facing") && allows("type", "left") {
        let joined = crate::chests::placement(
            blocks,
            &block.id,
            target,
            click.facing().opposite(),
            click,
            reads.block_at,
        );
        set(&mut props, "facing", joined.facing.name());
        set(&mut props, "type", joined.kind.name());
        partner = joined.partner;
    }
    if allows("half", "lower") {
        set(&mut props, "half", "lower");
    }
    if path.ends_with("_bed") {
        set(&mut props, "part", "foot");
        set(&mut props, "occupied", "false");
    }
    let mut state = blocks.by_name_and_props(&block.id, &props)?.0;
    if let Some((pos, partner_state)) = partner
        && let Some((joined, other)) = crate::copper::joined(blocks, state, partner_state)
    {
        state = joined;
        partner = Some((pos, other));
    }
    if hanging_supported(blocks, target, state, reads.block_at) == Some(false) {
        return None;
    }
    if let Some(hardened) = crate::falling::concrete(blocks, state) {
        let water = |id| {
            blocks
                .by_id(BlockStateId(id))
                .is_some_and(|entry| entry.block.id.as_str() == "minecraft:water")
        };
        let touches = water(current)
            || [
                Face::Up,
                Face::Down,
                Face::North,
                Face::South,
                Face::West,
                Face::East,
            ]
            .into_iter()
            .any(|face| (reads.block_at)(step(target, face)).is_some_and(water));
        if touches {
            state = hardened;
        }
    }
    let mut cells = vec![(target, state)];

    // Double plants use the same lower/upper occupancy contract as doors.
    let second = if path.ends_with("_door") || allows("half", "lower") {
        set(&mut props, "half", "upper");
        Some(BlockPos {
            y: target.y + 1,
            ..target
        })
    } else if path.ends_with("_bed") {
        set(&mut props, "part", "head");
        let step = click.facing().offset();
        Some(BlockPos {
            x: target.x + step[0],
            z: target.z + step[2],
            ..target
        })
    } else {
        None
    };
    if let Some(pos) = second {
        if !(reads.block_at)(pos).is_some_and(|state| (reads.replaceable)(state)) {
            return None;
        }
        if path.ends_with("_door") {
            set(&mut props, "waterlogged", "false");
        }
        cells.push((pos, blocks.by_name_and_props(&block.id, &props)?.0));
    }
    cells.extend(partner);
    shape_updates(blocks, &mut cells, reads.block_at)?;
    Some(cells)
}

/// Vanilla hand use of an interactive block: wooden doors (both halves),
/// trapdoors and fence gates open or close, levers flip. Fence gates swing
/// away from the user. `None` when the block does not react to a hand.
#[must_use]
pub fn use_block(
    blocks: &BlockRegistry,
    pos: BlockPos,
    yaw: f32,
    block_at: &dyn Fn(BlockPos) -> Option<u32>,
) -> Option<Vec<(BlockPos, u32)>> {
    let current = block_at(pos)?;
    let state = blocks.by_id(BlockStateId(current))?;
    let path = state.block.id.path();
    let flip = |state: u32, name: &str, facing: Option<Face>| {
        let state = blocks.by_id(BlockStateId(state))?;
        let mut props = state.properties.clone();
        let entry = props.iter_mut().find(|(key, _)| key == name)?;
        entry.1 = if entry.1 == "true" { "false" } else { "true" }.to_owned();
        if let Some(facing) = facing {
            set(&mut props, "facing", facing.name());
        }
        blocks
            .by_name_and_props(&state.block.id, &props)
            .map(|state| state.0)
    };
    if path == "iron_door" || path == "iron_trapdoor" {
        return None;
    }
    if path.ends_with("_door") {
        let mut cells = vec![(pos, flip(current, "open", None)?)];
        if let Some((other, _)) = other_half(blocks, pos, current)
            && let Some(other_state) = block_at(other)
            && blocks.by_id(BlockStateId(other_state))?.block.id == state.block.id
        {
            cells.push((other, flip(other_state, "open", None)?));
        }
        return Some(cells);
    }
    if path.ends_with("_trapdoor") {
        return Some(vec![(pos, flip(current, "open", None)?)]);
    }
    if path.ends_with("_fence_gate") {
        let opening = state
            .properties
            .iter()
            .any(|(key, value)| key == "open" && value == "false");
        let user = Click {
            clicked: pos,
            face: Face::Up,
            cursor: [0.5; 3],
            yaw,
            pitch: 0.0,
            sneaking: false,
        }
        .facing();
        let facing_user = state
            .properties
            .iter()
            .any(|(key, value)| key == "facing" && value == user.opposite().name());
        let swing = (opening && facing_user).then_some(user);
        return Some(vec![(pos, flip(current, "open", swing)?)]);
    }
    if path == "lever" {
        return Some(vec![(pos, flip(current, "powered", None)?)]);
    }
    let property = |name: &str| {
        state
            .properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let replace_property = |name: &str, value: &str| {
        let mut props = state.properties.clone();
        let (_, entry) = props.iter_mut().find(|(key, _)| key == name)?;
        *entry = value.to_owned();
        Some(vec![(
            pos,
            blocks.by_name_and_props(&state.block.id, &props)?.0,
        )])
    };
    if path.ends_with("_button") {
        return if property("powered") == Some("true") {
            Some(Vec::new())
        } else {
            replace_property("powered", "true")
        };
    }
    match path {
        "note_block" => {
            let note = property("note")?.parse::<u8>().ok()?;
            return replace_property("note", &((note + 1) % 25).to_string());
        }
        "repeater" => {
            let delay = property("delay")?.parse::<u8>().ok()?;
            return replace_property("delay", &(delay % 4 + 1).to_string());
        }
        "comparator" => {
            return replace_property(
                "mode",
                if property("mode") == Some("subtract") {
                    "compare"
                } else {
                    "subtract"
                },
            );
        }
        "daylight_detector" => {
            return replace_property(
                "inverted",
                if property("inverted") == Some("true") {
                    "false"
                } else {
                    "true"
                },
            );
        }
        _ => {}
    }
    None
}

/// Where a block hangs from: vanilla `canSurvive` for the common attached
/// blocks. `None` for free-standing blocks.
#[must_use]
pub fn support(
    blocks: &BlockRegistry,
    tags: &mc_data::tags::TagsData,
    pos: BlockPos,
    state: u32,
) -> Option<BlockPos> {
    let state = blocks.by_id(BlockStateId(state))?;
    let path = state.block.id.path();
    let prop = |name: &str| {
        state
            .properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let face = |name: &str| match name {
        "north" => Some(Face::North),
        "south" => Some(Face::South),
        "west" => Some(Face::West),
        "east" => Some(Face::East),
        _ => None,
    };
    let tagged = |tag: &str| tags.contains_raw_id("minecraft:block", tag, state.block.raw_id);
    let at = |face: Face| {
        let step = face.offset();
        BlockPos {
            x: pos.x + step[0],
            y: pos.y + step[1],
            z: pos.z + step[2],
        }
    };
    if path.ends_with("_wall_hanging_sign") {
        return Some(at(face(prop("facing")?)?.clockwise()));
    }
    if path.ends_with("_hanging_sign") {
        return Some(at(Face::Up));
    }
    // Wall-hung: the wall is behind the facing.
    if path.ends_with("wall_torch")
        || path.ends_with("_wall_banner")
        || path == "ladder"
        || tagged("minecraft:wall_signs")
    {
        return Some(at(face(prop("facing")?)?.opposite()));
    }
    // Buttons and levers hang on their attachment face.
    if let Some(attach) = prop("face") {
        return Some(match attach {
            "floor" => at(Face::Down),
            "ceiling" => at(Face::Up),
            _ => at(face(prop("facing")?)?.opposite()),
        });
    }
    // Upper halves hang on their lower half (`other_half` pairs them).
    if prop("half") == Some("upper") {
        return None;
    }
    let floor = path.ends_with("_door")
        || matches!(
            path,
            "torch"
                | "soul_torch"
                | "redstone_torch"
                | "copper_torch"
                | "snow"
                | "redstone_wire"
                | "repeater"
                | "comparator"
        )
        || [
            "minecraft:rails",
            "minecraft:wool_carpets",
            "minecraft:pressure_plates",
            "minecraft:standing_signs",
            "minecraft:banners",
            "minecraft:small_flowers",
        ]
        .into_iter()
        .any(tagged);
    floor.then(|| at(Face::Down))
}

fn step(pos: BlockPos, face: Face) -> BlockPos {
    let [x, y, z] = face.offset();
    BlockPos {
        x: pos.x + x,
        y: pos.y + y,
        z: pos.z + z,
    }
}

/// A waterlogged block leaves its source behind when broken.
pub(crate) fn break_state(blocks: &BlockRegistry, state: u32, air: u32, water: u32) -> u32 {
    blocks
        .by_id(BlockStateId(state))
        .filter(|state| {
            state
                .properties
                .iter()
                .any(|(key, value)| key == "waterlogged" && value == "true")
        })
        .map_or(air, |_| water)
}

/// Pair identity includes complementary parts and reciprocal coordinates;
/// same-name adjacent malformed doors/beds never destroy one another.
pub(crate) fn paired(
    blocks: &BlockRegistry,
    pos: BlockPos,
    state: u32,
    other: BlockPos,
    other_state: u32,
) -> bool {
    let Some(first) = blocks.by_id(BlockStateId(state)) else {
        return false;
    };
    let Some(second) = blocks.by_id(BlockStateId(other_state)) else {
        return false;
    };
    first.block.id == second.block.id
        && first
            .properties
            .iter()
            .find(|(key, _)| key == "facing")
            .map(|(_, value)| value)
            == second
                .properties
                .iter()
                .find(|(key, _)| key == "facing")
                .map(|(_, value)| value)
        && other_half(blocks, other, other_state).is_some_and(|(back, _)| back == pos)
        && other_half(blocks, pos, state).is_some_and(|(next, _)| next == other)
}

#[path = "placement_shapes.rs"]
mod shapes;
pub(crate) use shapes::shape_updates;

/// Whether a neighbor update affects one of the block's support cells.
pub(crate) fn support_changed(
    blocks: &BlockRegistry,
    tags: &mc_data::tags::TagsData,
    pos: BlockPos,
    state: u32,
    changed: BlockPos,
) -> bool {
    let Some(at) = support(blocks, tags, pos, state) else {
        return false;
    };
    at == changed
        || (blocks
            .by_id(BlockStateId(state))
            .is_some_and(|entry| entry.block.id.path().ends_with("_wall_hanging_sign"))
            && changed
                == BlockPos {
                    x: pos.x + (pos.x - at.x),
                    y: pos.y,
                    z: pos.z + (pos.z - at.z),
                })
}

fn hanging_supported(
    blocks: &BlockRegistry,
    pos: BlockPos,
    state: u32,
    read: &dyn Fn(BlockPos) -> Option<u32>,
) -> Option<bool> {
    use mc_data::block_facts::{SturdyFace, has_center_sturdy_face, has_full_sturdy_face};
    let entry = blocks.by_id(BlockStateId(state))?;
    if !entry.block.id.path().ends_with("_hanging_sign") {
        return None;
    }
    if entry.block.id.path().ends_with("_wall_hanging_sign") {
        let facing = entry
            .properties
            .iter()
            .find(|(key, _)| key == "facing")
            .and_then(|(_, value)| Face::from_name(value))?;
        return Some(
            [facing.clockwise(), facing.counter_clockwise()]
                .into_iter()
                .any(|side| {
                    let Some(id) = read(step(pos, side)) else {
                        return false;
                    };
                    let Some(neighbor) = blocks.by_id(BlockStateId(id)) else {
                        return false;
                    };
                    if neighbor.block.id.path().ends_with("_wall_hanging_sign") {
                        return neighbor
                            .properties
                            .iter()
                            .find(|(key, _)| key == "facing")
                            .and_then(|(_, value)| Face::from_name(value))
                            .is_some_and(|other| other.axis() == facing.axis());
                    }
                    let face = match side.opposite() {
                        Face::North => SturdyFace::North,
                        Face::South => SturdyFace::South,
                        Face::West => SturdyFace::West,
                        Face::East => SturdyFace::East,
                        _ => return false,
                    };
                    has_full_sturdy_face(id, &neighbor.block.id, &neighbor.properties, face)
                }),
        );
    }
    Some(
        read(step(pos, Face::Up))
            .and_then(|id| {
                blocks.by_id(BlockStateId(id)).map(|above| {
                    has_center_sturdy_face(id, &above.block.id, &above.properties, SturdyFace::Down)
                })
            })
            .unwrap_or(false),
    )
}

pub(crate) fn supported(
    blocks: &BlockRegistry,
    tags: &mc_data::tags::TagsData,
    pos: BlockPos,
    state: u32,
    read: &dyn Fn(BlockPos) -> Option<u32>,
) -> bool {
    use mc_data::block_facts::{SturdyFace, has_full_sturdy_face};
    if let Some(survives) = hanging_supported(blocks, pos, state, read) {
        return survives;
    }
    let Some(at) = support(blocks, tags, pos, state) else {
        return true;
    };
    let Some(id) = read(at) else {
        return false;
    };
    let Some(block) = blocks.by_id(BlockStateId(id)) else {
        return false;
    };
    let face = if at.y < pos.y {
        SturdyFace::Up
    } else if at.y > pos.y {
        SturdyFace::Down
    } else if at.x < pos.x {
        SturdyFace::East
    } else if at.x > pos.x {
        SturdyFace::West
    } else if at.z < pos.z {
        SturdyFace::South
    } else {
        SturdyFace::North
    };
    if at.y < pos.y
        && blocks.by_id(BlockStateId(state)).is_some_and(|placed| {
            matches!(
                placed.block.id.path(),
                "torch" | "soul_torch" | "redstone_torch" | "copper_torch"
            )
        })
    {
        return mc_data::block_facts::has_center_sturdy_face(
            id,
            &block.block.id,
            &block.properties,
            face,
        ) || block.block.id.path().ends_with("_fence")
            || block.block.id.path().ends_with("_wall");
    }
    has_full_sturdy_face(id, &block.block.id, &block.properties, face)
}

/// The other cell of a two-cell block (door, double plant, bed) and the
/// cell whose loot the pair drops (vanilla: the lower half, the bed head).
#[must_use]
pub fn other_half(
    blocks: &BlockRegistry,
    pos: BlockPos,
    state: u32,
) -> Option<(BlockPos, BlockPos)> {
    let state = blocks.by_id(BlockStateId(state))?;
    let prop = |name: &str| {
        state
            .properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let shifted = |step: [i32; 3], sign: i32| BlockPos {
        x: pos.x + sign * step[0],
        y: pos.y + sign * step[1],
        z: pos.z + sign * step[2],
    };
    match (prop("half"), prop("part")) {
        (Some("lower"), _) => Some((shifted([0, 1, 0], 1), pos)),
        (Some("upper"), _) => {
            let lower = shifted([0, 1, 0], -1);
            Some((lower, lower))
        }
        (_, Some(part)) => {
            let facing = match prop("facing")? {
                "north" => Face::North,
                "south" => Face::South,
                "west" => Face::West,
                _ => Face::East,
            };
            if part == "foot" {
                let head = shifted(facing.offset(), 1);
                Some((head, head))
            } else {
                Some((shifted(facing.offset(), -1), pos))
            }
        }
        _ => None,
    }
}

fn set(props: &mut [(String, String)], name: &str, value: &str) {
    if let Some(entry) = props.iter_mut().find(|(key, _)| key == name) {
        entry.1 = value.to_owned();
    }
}

fn is_slab(block: &Block) -> bool {
    block.id.path().ends_with("_slab")
}

/// The double state of `slab` when `existing` is that slab and `fits` its
/// current type.
fn doubled_slab(
    blocks: &BlockRegistry,
    slab: &Block,
    existing: u32,
    fits: impl Fn(&str) -> bool,
) -> Option<u32> {
    let existing = blocks.by_id(BlockStateId(existing))?;
    if existing.block.id != slab.id {
        return None;
    }
    let slab_type = existing
        .properties
        .iter()
        .find(|(key, _)| key == "type")
        .map(|(_, value)| value.as_str())?;
    if slab_type == "double" || !fits(slab_type) {
        return None;
    }
    let mut props = existing.properties.clone();
    for (key, value) in &mut props {
        match key.as_str() {
            "type" => *value = "double".to_owned(),
            "waterlogged" => *value = "false".to_owned(),
            _ => {}
        }
    }
    blocks
        .by_name_and_props(&slab.id, &props)
        .map(|state| state.0)
}

/// The wall twin of a standing block: `torch` -> `wall_torch`,
/// `oak_sign` -> `oak_wall_sign`, `white_banner` -> `white_wall_banner`.
fn wall_variant<'a>(blocks: &'a BlockRegistry, id: &Identifier) -> Option<&'a Block> {
    let path = id.path();
    let wall = if let Some(wood) = path.strip_suffix("_hanging_sign") {
        format!("{wood}_wall_hanging_sign")
    } else {
        match path.rsplit_once('_') {
            Some((head, tail)) => format!("{head}_wall_{tail}"),
            None => format!("wall_{path}"),
        }
    };
    blocks.block(&Identifier::parse(format!("{}:{wall}", id.namespace())).ok()?)
}

/// Prefer a neighboring door's opposite hinge, then the obstructed side.
/// Cursor position breaks ties only when neither neighborhood rule decides.
fn door_hinge(
    blocks: &BlockRegistry,
    target: BlockPos,
    click: &Click,
    read: &dyn Fn(BlockPos) -> Option<u32>,
) -> &'static str {
    let left = step(target, click.facing().counter_clockwise());
    let right = step(target, click.facing().clockwise());
    let door = |pos| {
        read(pos)
            .and_then(|id| blocks.by_id(BlockStateId(id)))
            .is_some_and(|block| {
                block.block.id.path().ends_with("_door")
                    && block
                        .properties
                        .iter()
                        .any(|(key, value)| key == "half" && value == "lower")
            })
    };
    match (door(left), door(right)) {
        (true, false) => return "right",
        (false, true) => return "left",
        _ => {}
    }
    let obstruction = |pos| {
        [pos, step(pos, Face::Up)]
            .into_iter()
            .filter(|pos| {
                read(*pos)
                    .and_then(|id| blocks.by_id(BlockStateId(id)).map(|block| (id, block)))
                    .is_some_and(|(id, block)| {
                        mc_data::block_facts::has_full_sturdy_face(
                            id,
                            &block.block.id,
                            &block.properties,
                            mc_data::block_facts::SturdyFace::Up,
                        )
                    })
            })
            .count()
    };
    match obstruction(left).cmp(&obstruction(right)) {
        std::cmp::Ordering::Less => return "right",
        std::cmp::Ordering::Greater => return "left",
        std::cmp::Ordering::Equal => {}
    }
    let facing = click.facing().offset();
    let (hit_x, hit_z) = (click.cursor[0], click.cursor[2]);
    let right = (facing[0] < 0 && hit_z < 0.5)
        || (facing[0] > 0 && hit_z > 0.5)
        || (facing[2] < 0 && hit_x > 0.5)
        || (facing[2] > 0 && hit_x < 0.5);
    if right { "right" } else { "left" }
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;
