use std::collections::HashMap;

use mc_data::Identifier;

use super::{Click, Face, Reads, place};
use crate::boundary::BlockPos;
use crate::registry::CoreRegistry;

/// Placed cells as `((x, y, z), state)`.
type Cells = Vec<((i32, i32, i32), u32)>;

/// Stone floor at y = 0 with whatever `extra` holds; air elsewhere.
struct World {
    registry: CoreRegistry,
    extra: HashMap<(i32, i32, i32), u32>,
    /// The placer sneaks.
    sneaking: bool,
}

impl World {
    fn new() -> Self {
        Self {
            registry: CoreRegistry::load(None).expect("registry loads"),
            extra: HashMap::new(),
            sneaking: false,
        }
    }

    fn state(&self, name: &str, props: &[(&str, &str)]) -> u32 {
        let id = Identifier::parse(name).expect("id");
        let props: Vec<(String, String)> = props
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        let block = self.registry.blocks.block(&id).expect("block");
        let mut full = self
            .registry
            .blocks
            .by_id(block.default)
            .expect("default")
            .properties
            .clone();
        for (key, value) in props {
            if let Some(entry) = full.iter_mut().find(|(name, _)| *name == key) {
                entry.1 = value;
            }
        }
        self.registry
            .blocks
            .by_name_and_props(&id, &full)
            .expect("state")
            .0
    }

    /// Places `item` clicking `face` of `clicked` with the cursor at
    /// height `cursor_y`, looking along `yaw` (0 = south).
    fn place(
        &self,
        item: &str,
        clicked: (i32, i32, i32),
        face: Face,
        cursor_y: f32,
        yaw: f32,
    ) -> Option<Cells> {
        let air = self.registry.states.air;
        let stone = self.registry.states.stone;
        let block_at = |pos: BlockPos| {
            Some(
                self.extra
                    .get(&(pos.x, pos.y, pos.z))
                    .copied()
                    .unwrap_or(if pos.y <= 0 { stone } else { air }),
            )
        };
        let replaceable = |state: u32| state == air;
        let reads = Reads {
            block_at: &block_at,
            replaceable: &replaceable,
            water_source: self.registry.states.water,
        };
        let item_block = self
            .registry
            .blocks
            .block(&Identifier::parse(item).expect("id"))
            .expect("block");
        let click = Click {
            clicked: BlockPos {
                x: clicked.0,
                y: clicked.1,
                z: clicked.2,
            },
            face,
            cursor: [0.5, cursor_y, 0.5],
            yaw,
            pitch: 0.0,
            sneaking: self.sneaking,
        };
        place(&self.registry.blocks, item_block, &click, &reads).map(|cells| {
            cells
                .into_iter()
                .map(|(pos, state)| ((pos.x, pos.y, pos.z), state))
                .collect()
        })
    }
}

#[test]
fn logs_follow_the_clicked_face_axis() {
    let mut world = World::new();
    let log = |world: &World, axis| world.state("minecraft:oak_log", &[("axis", axis)]);
    assert_eq!(
        world.place("minecraft:oak_log", (0, 0, 0), Face::Up, 1.0, 0.0),
        Some(vec![((0, 1, 0), log(&world, "y"))])
    );
    world.extra.insert((0, 1, 0), world.registry.states.stone);
    assert_eq!(
        world.place("minecraft:oak_log", (0, 1, 0), Face::East, 0.5, 0.0),
        Some(vec![((1, 1, 0), log(&world, "x"))])
    );
    assert_eq!(
        world.place("minecraft:oak_log", (0, 1, 0), Face::North, 0.5, 0.0),
        Some(vec![((0, 1, -1), log(&world, "z"))])
    );
}

/// Stairs face where the placer looks, upside down from a face's upper
/// half; furnaces face the placer.
#[test]
fn stairs_and_furnaces_face_from_the_placer() {
    let mut world = World::new();
    let stairs = |world: &World, half| {
        world.state(
            "minecraft:oak_stairs",
            &[("facing", "south"), ("half", half)],
        )
    };
    assert_eq!(
        world.place("minecraft:oak_stairs", (0, 0, 0), Face::Up, 1.0, 0.0),
        Some(vec![((0, 1, 0), stairs(&world, "bottom"))])
    );
    world.extra.insert((0, 1, 0), world.registry.states.stone);
    assert_eq!(
        world.place("minecraft:oak_stairs", (0, 1, 0), Face::South, 0.8, 0.0),
        Some(vec![((0, 1, 1), stairs(&world, "top"))])
    );
    world.extra.remove(&(0, 1, 0));
    assert_eq!(
        world.place("minecraft:furnace", (0, 0, 0), Face::Up, 1.0, 90.0),
        Some(vec![(
            (0, 1, 0),
            world.state("minecraft:furnace", &[("facing", "east")])
        )])
    );
}

#[test]
fn torches_stand_on_floors_and_hang_on_walls_only() {
    let mut world = World::new();
    assert_eq!(
        world.place("minecraft:torch", (0, 0, 0), Face::Up, 1.0, 0.0),
        Some(vec![((0, 1, 0), world.state("minecraft:torch", &[]))])
    );
    world.extra.insert((0, 1, 0), world.registry.states.stone);
    assert_eq!(
        world.place("minecraft:torch", (0, 1, 0), Face::West, 0.5, 0.0),
        Some(vec![(
            (-1, 1, 0),
            world.state("minecraft:wall_torch", &[("facing", "west")])
        )])
    );
    assert_eq!(
        world.place("minecraft:torch", (0, 2, 0), Face::Down, 0.0, 0.0),
        None
    );
}

/// Doors and beds claim two cells, or none when the second is taken.
#[test]
fn doors_and_beds_place_both_halves() {
    let mut world = World::new();
    let door = |half| {
        world.state(
            "minecraft:oak_door",
            &[("facing", "south"), ("half", half), ("hinge", "left")],
        )
    };
    assert_eq!(
        world.place("minecraft:oak_door", (0, 0, 0), Face::Up, 1.0, 0.0),
        Some(vec![((0, 1, 0), door("lower")), ((0, 2, 0), door("upper"))])
    );
    let bed = |part| world.state("minecraft:red_bed", &[("facing", "west"), ("part", part)]);
    assert_eq!(
        world.place("minecraft:red_bed", (0, 0, 0), Face::Up, 1.0, 90.0),
        Some(vec![((0, 1, 0), bed("foot")), ((-1, 1, 0), bed("head"))])
    );
    let stone = world.registry.states.stone;
    world.extra.insert((0, 2, 0), stone);
    assert_eq!(
        world.place("minecraft:oak_door", (0, 0, 0), Face::Up, 1.0, 0.0),
        None
    );
}

/// A slab clicked from its open side doubles in place; a side click on the
/// upper half places a top slab.
#[test]
fn slabs_double_or_take_the_clicked_half() {
    let mut world = World::new();
    let [bottom, top, double] = ["bottom", "top", "double"]
        .map(|slab_type| world.state("minecraft:oak_slab", &[("type", slab_type)]));
    world.extra.insert((0, 1, 0), bottom);
    assert_eq!(
        world.place("minecraft:oak_slab", (0, 1, 0), Face::Up, 0.5, 0.0),
        Some(vec![((0, 1, 0), double)])
    );
    assert_eq!(
        world.place("minecraft:oak_slab", (0, 1, 0), Face::East, 0.4, 0.0),
        Some(vec![((1, 1, 0), bottom)])
    );
    world.extra.insert((0, 1, 0), world.registry.states.stone);
    assert_eq!(
        world.place("minecraft:oak_slab", (0, 1, 0), Face::East, 0.8, 0.0),
        Some(vec![((1, 1, 0), top)])
    );
}

/// A hand opens both halves of a wooden door, never an iron one; a fence
/// gate swings away from the user.
#[test]
fn hands_open_doors_and_gates() {
    let mut world = World::new();
    let door = |half, open| {
        world.state(
            "minecraft:oak_door",
            &[("facing", "south"), ("half", half), ("open", open)],
        )
    };
    let (lower, upper) = (door("lower", "false"), door("upper", "false"));
    let (lower_open, upper_open) = (door("lower", "true"), door("upper", "true"));
    let gate = |facing, open| {
        world.state(
            "minecraft:oak_fence_gate",
            &[("facing", facing), ("open", open)],
        )
    };
    let (gate_north, gate_south_open) = (gate("north", "false"), gate("south", "true"));
    let iron = world.state("minecraft:iron_door", &[("half", "lower")]);
    world.extra.insert((0, 1, 0), lower);
    world.extra.insert((0, 2, 0), upper);
    world.extra.insert((5, 1, 0), gate_north);
    world.extra.insert((9, 1, 0), iron);
    let use_at = |world: &World, pos: (i32, i32, i32), yaw: f32| {
        let stone = world.registry.states.stone;
        let air = world.registry.states.air;
        let block_at = |pos: BlockPos| {
            Some(
                world
                    .extra
                    .get(&(pos.x, pos.y, pos.z))
                    .copied()
                    .unwrap_or(if pos.y <= 0 { stone } else { air }),
            )
        };
        super::use_block(
            &world.registry.blocks,
            BlockPos {
                x: pos.0,
                y: pos.1,
                z: pos.2,
            },
            yaw,
            &block_at,
        )
        .map(|cells| {
            cells
                .into_iter()
                .map(|(pos, state)| ((pos.x, pos.y, pos.z), state))
                .collect::<Cells>()
        })
    };
    assert_eq!(
        use_at(&world, (0, 2, 0), 0.0),
        Some(vec![((0, 2, 0), upper_open), ((0, 1, 0), lower_open)])
    );
    // Facing south (yaw 0) at a north-facing gate: it swings south.
    assert_eq!(
        use_at(&world, (5, 1, 0), 0.0),
        Some(vec![((5, 1, 0), gate_south_open)])
    );
    assert_eq!(use_at(&world, (9, 1, 0), 0.0), None);
}

fn chest(world: &World, facing: &str, kind: &str) -> u32 {
    world.state("minecraft:chest", &[("facing", facing), ("type", kind)])
}

/// A chest faces the placer; beside a single chest of the same facing it
/// joins it (vanilla `ChestBlock.getStateForPlacement`): the new chest and
/// the neighbour get opposite types.
#[test]
fn chest_beside_a_chest_forms_a_double_chest() {
    let mut world = World::new();
    // Facing south (yaw 0), the chest faces north.
    assert_eq!(
        world.place("minecraft:chest", (0, 0, 0), Face::Up, 1.0, 0.0),
        Some(vec![((0, 1, 0), chest(&world, "north", "single"))])
    );
    world
        .extra
        .insert((0, 1, 0), chest(&world, "north", "single"));
    assert_eq!(
        world.place("minecraft:chest", (1, 0, 0), Face::Up, 1.0, 0.0),
        Some(vec![
            ((1, 1, 0), chest(&world, "north", "right")),
            ((0, 1, 0), chest(&world, "north", "left")),
        ])
    );
    assert_eq!(
        world.place("minecraft:chest", (-1, 0, 0), Face::Up, 1.0, 0.0),
        Some(vec![
            ((-1, 1, 0), chest(&world, "north", "left")),
            ((0, 1, 0), chest(&world, "north", "right")),
        ])
    );
    // Another facing, or a chest in line with the facing, stays single.
    assert_eq!(
        world.place("minecraft:chest", (1, 0, 0), Face::Up, 1.0, 90.0),
        Some(vec![((1, 1, 0), chest(&world, "east", "single"))])
    );
    assert_eq!(
        world.place("minecraft:chest", (0, 0, 1), Face::Up, 1.0, 0.0),
        Some(vec![((0, 1, 1), chest(&world, "north", "single"))])
    );
}

/// Sneaking against a chest's side joins it whatever the placer faces, and
/// sneaking on top does not join at all.
#[test]
fn sneaking_chest_joins_the_clicked_side_only() {
    let mut world = World::new();
    world
        .extra
        .insert((0, 1, 0), chest(&world, "north", "single"));
    world.sneaking = true;
    assert_eq!(
        world.place("minecraft:chest", (0, 1, 0), Face::East, 0.5, 180.0),
        Some(vec![
            ((1, 1, 0), chest(&world, "north", "right")),
            ((0, 1, 0), chest(&world, "north", "left")),
        ])
    );
    assert_eq!(
        world.place("minecraft:chest", (1, 0, 0), Face::Up, 1.0, 0.0),
        Some(vec![((1, 1, 0), chest(&world, "north", "single"))])
    );
}

/// A double chest's storage cells run right half first; breaking one half
/// turns the other back into a single chest.
#[test]
fn double_chest_cells_and_break_revert() {
    let mut world = World::new();
    world
        .extra
        .insert((1, 1, 0), chest(&world, "north", "right"));
    world
        .extra
        .insert((0, 1, 0), chest(&world, "north", "left"));
    let air = world.registry.states.air;
    let block_at = |pos: BlockPos| {
        Some(
            world
                .extra
                .get(&(pos.x, pos.y, pos.z))
                .copied()
                .unwrap_or(air),
        )
    };
    let cell = |x| BlockPos { x, y: 1, z: 0 };
    let blocks = &world.registry.blocks;
    assert_eq!(
        crate::chests::storage_cells(blocks, cell(0), &block_at),
        vec![cell(1), cell(0)]
    );
    assert_eq!(
        crate::chests::storage_cells(blocks, cell(1), &block_at),
        vec![cell(1), cell(0)]
    );
    assert_eq!(
        crate::chests::break_partner(blocks, cell(0), chest(&world, "north", "left"), &block_at),
        Some((
            cell(1),
            chest(&world, "north", "right"),
            chest(&world, "north", "single")
        ))
    );
    // A lone half (partner gone) is just itself.
    let lonely = |pos: BlockPos| {
        (pos.x == 0)
            .then(|| chest(&world, "north", "left"))
            .or(Some(air))
    };
    assert_eq!(
        crate::chests::storage_cells(blocks, cell(0), &lonely),
        vec![cell(0)]
    );
}

#[test]
fn hanging_wall_placement_uses_bar_ends_and_perpendicular_looking_axis() {
    let mut world = World::new();
    let stone = world.registry.states.stone;
    world.extra.insert((0, 2, 0), stone);
    let cells = world
        .place(
            "minecraft:oak_hanging_sign",
            (0, 2, 0),
            Face::East,
            0.5,
            0.0,
        )
        .unwrap();
    assert_eq!(
        cells,
        vec![(
            (1, 2, 0),
            world.state("minecraft:oak_wall_hanging_sign", &[("facing", "north")])
        )]
    );
    let pos = BlockPos { x: 1, y: 2, z: 0 };
    let sign = cells[0].1;
    let read = |cell: BlockPos| {
        Some(
            world
                .extra
                .get(&(cell.x, cell.y, cell.z))
                .copied()
                .unwrap_or(world.registry.states.air),
        )
    };
    assert!(super::supported(
        &world.registry.blocks,
        &world.registry.tags,
        pos,
        sign,
        &read
    ));
    assert!(super::support_changed(
        &world.registry.blocks,
        &world.registry.tags,
        pos,
        sign,
        BlockPos { x: 0, ..pos }
    ));
    assert!(super::support_changed(
        &world.registry.blocks,
        &world.registry.tags,
        pos,
        sign,
        BlockPos { x: 2, ..pos }
    ));
    assert!(!super::support_changed(
        &world.registry.blocks,
        &world.registry.tags,
        pos,
        sign,
        BlockPos { z: 1, ..pos }
    ));
    world.extra.remove(&(0, 2, 0));
    world.extra.insert((1, 2, 1), stone);
    let read = |cell: BlockPos| {
        Some(
            world
                .extra
                .get(&(cell.x, cell.y, cell.z))
                .copied()
                .unwrap_or(world.registry.states.air),
        )
    };
    assert!(
        !super::supported(
            &world.registry.blocks,
            &world.registry.tags,
            pos,
            sign,
            &read
        ),
        "the wall behind the text is not bar support"
    );
    let chain = world.state("minecraft:birch_wall_hanging_sign", &[("facing", "south")]);
    world.extra.insert((2, 2, 0), chain);
    let read = |cell: BlockPos| {
        Some(
            world
                .extra
                .get(&(cell.x, cell.y, cell.z))
                .copied()
                .unwrap_or(world.registry.states.air),
        )
    };
    assert!(super::supported(
        &world.registry.blocks,
        &world.registry.tags,
        pos,
        sign,
        &read
    ));
    let crossed = world.state("minecraft:birch_wall_hanging_sign", &[("facing", "east")]);
    world.extra.insert((2, 2, 0), crossed);
    let read = |cell: BlockPos| {
        Some(
            world
                .extra
                .get(&(cell.x, cell.y, cell.z))
                .copied()
                .unwrap_or(world.registry.states.air),
        )
    };
    assert!(
        !super::supported(
            &world.registry.blocks,
            &world.registry.tags,
            pos,
            sign,
            &read
        ),
        "only same-axis hanging bars chain"
    );
}

#[test]
fn hanging_ceiling_attachment_and_rotation_follow_full_underside_and_sneaking() {
    let mut world = World::new();
    world.extra.insert((0, 3, 0), world.registry.states.stone);
    let loose = world
        .place(
            "minecraft:oak_hanging_sign",
            (0, 3, 0),
            Face::Down,
            0.5,
            22.0,
        )
        .unwrap();
    assert_eq!(
        loose,
        vec![(
            (0, 2, 0),
            world.state(
                "minecraft:oak_hanging_sign",
                &[("attached", "false"), ("rotation", "8")]
            )
        )]
    );
    world.sneaking = true;
    let attached = world
        .place(
            "minecraft:oak_hanging_sign",
            (0, 3, 0),
            Face::Down,
            0.5,
            22.0,
        )
        .unwrap();
    assert_eq!(
        attached,
        vec![(
            (0, 2, 0),
            world.state(
                "minecraft:oak_hanging_sign",
                &[("attached", "true"), ("rotation", "9")]
            )
        )]
    );
    world.sneaking = false;
    let aligned = world.state("minecraft:birch_hanging_sign", &[("rotation", "8")]);
    world.extra.insert((0, 3, 0), aligned);
    assert_eq!(
        world
            .place(
                "minecraft:oak_hanging_sign",
                (0, 3, 0),
                Face::Down,
                0.5,
                22.0
            )
            .unwrap(),
        loose
    );
    let diagonal = world.state("minecraft:birch_hanging_sign", &[("rotation", "1")]);
    world.extra.insert((0, 3, 0), diagonal);
    assert_eq!(
        world
            .place(
                "minecraft:oak_hanging_sign",
                (0, 3, 0),
                Face::Down,
                0.5,
                22.0
            )
            .unwrap(),
        attached
    );
    let wall = world.state("minecraft:birch_wall_hanging_sign", &[("facing", "north")]);
    world.extra.insert((0, 3, 0), wall);
    assert_eq!(
        world
            .place(
                "minecraft:oak_hanging_sign",
                (0, 3, 0),
                Face::Down,
                0.5,
                22.0
            )
            .unwrap(),
        loose
    );
}

#[test]
fn ceiling_hanging_sign_requires_center_underside_not_the_whole_face() {
    let mut world = World::new();
    let pos = BlockPos { x: 0, y: 2, z: 0 };
    let sign = world.state("minecraft:oak_hanging_sign", &[]);
    let fence = world.state("minecraft:oak_fence", &[]);
    world.extra.insert((0, 3, 0), fence);
    let read = |cell: BlockPos| {
        Some(
            world
                .extra
                .get(&(cell.x, cell.y, cell.z))
                .copied()
                .unwrap_or(world.registry.states.air),
        )
    };
    assert!(super::supported(
        &world.registry.blocks,
        &world.registry.tags,
        pos,
        sign,
        &read
    ));
    let top_slab = world.state("minecraft:oak_slab", &[("type", "top")]);
    world.extra.insert((0, 3, 0), top_slab);
    let read = |cell: BlockPos| {
        Some(
            world
                .extra
                .get(&(cell.x, cell.y, cell.z))
                .copied()
                .unwrap_or(world.registry.states.air),
        )
    };
    assert!(!super::supported(
        &world.registry.blocks,
        &world.registry.tags,
        pos,
        sign,
        &read
    ));
    world.extra.remove(&(0, 3, 0));
    let read = |cell: BlockPos| {
        Some(
            world
                .extra
                .get(&(cell.x, cell.y, cell.z))
                .copied()
                .unwrap_or(world.registry.states.air),
        )
    };
    assert!(!super::supported(
        &world.registry.blocks,
        &world.registry.tags,
        pos,
        sign,
        &read
    ));
}

#[test]
fn top_face_wall_hanging_sign_tries_secondary_horizontal_looking_axis() {
    let mut world = World::new();
    world.extra.insert((0, 2, 1), world.registry.states.stone);
    let cells = world.place("minecraft:oak_hanging_sign", (0, 0, 0), Face::Up, 0.5, 0.0);
    // This target is y1: neither bar end is supported, despite the floor.
    assert!(cells.is_none());
    world.extra.insert((0, 1, 1), world.registry.states.stone);
    let cells = world
        .place("minecraft:oak_hanging_sign", (0, 0, 0), Face::Up, 0.5, 0.0)
        .unwrap();
    let state = world
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(cells[0].1))
        .unwrap();
    let facing = state
        .properties
        .iter()
        .find(|(key, _)| key == "facing")
        .unwrap()
        .1
        .as_str();
    assert!(
        matches!(facing, "east" | "west"),
        "secondary look axis admits the supported north/south bar ends"
    );
}

#[test]
fn center_only_ceiling_support_keeps_free_rotation_and_attached_chains() {
    let mut world = World::new();
    let fence = world.state("minecraft:oak_fence", &[]);
    world.extra.insert((0, 3, 0), fence);
    let cells = world
        .place(
            "minecraft:oak_hanging_sign",
            (0, 3, 0),
            Face::Down,
            0.5,
            22.0,
        )
        .unwrap();
    assert_eq!(
        cells,
        vec![(
            (0, 2, 0),
            world.state(
                "minecraft:oak_hanging_sign",
                &[("attached", "true"), ("rotation", "9")]
            )
        )]
    );
}

#[test]
fn common_attachment_support_removal_matrix() {
    use crate::crafter::tests::{block, fixture};
    use mc_nbt::Tag;

    let (mut state, _) = fixture(None);
    let vanilla = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    state.registry = std::sync::Arc::new(
        crate::registry::CoreRegistry::load(Some(&vanilla)).expect("full vanilla block loot"),
    );
    let pos = BlockPos { x: 3, y: 201, z: 3 };
    type AttachmentCase<'a> = (
        &'a str,
        &'a [(&'a str, &'a str)],
        [i32; 3],
        &'a str,
        Option<&'a str>,
    );
    let cases: &[AttachmentCase<'_>] = &[
        ("oak_sign", &[], [0, -1, 0], "oak_sign", Some("sign")),
        (
            "oak_wall_sign",
            &[("facing", "east")],
            [-1, 0, 0],
            "oak_sign",
            Some("sign"),
        ),
        (
            "oak_hanging_sign",
            &[],
            [0, 1, 0],
            "oak_hanging_sign",
            Some("hanging_sign"),
        ),
        (
            "oak_wall_hanging_sign",
            &[("facing", "north")],
            [1, 0, 0],
            "oak_hanging_sign",
            Some("hanging_sign"),
        ),
        ("torch", &[], [0, -1, 0], "torch", None),
        (
            "wall_torch",
            &[("facing", "east")],
            [-1, 0, 0],
            "torch",
            None,
        ),
        ("soul_torch", &[], [0, -1, 0], "soul_torch", None),
        (
            "soul_wall_torch",
            &[("facing", "east")],
            [-1, 0, 0],
            "soul_torch",
            None,
        ),
        ("redstone_torch", &[], [0, -1, 0], "redstone_torch", None),
        (
            "redstone_wall_torch",
            &[("facing", "east")],
            [-1, 0, 0],
            "redstone_torch",
            None,
        ),
        ("copper_torch", &[], [0, -1, 0], "copper_torch", None),
        (
            "copper_wall_torch",
            &[("facing", "east")],
            [-1, 0, 0],
            "copper_torch",
            None,
        ),
        (
            "white_banner",
            &[],
            [0, -1, 0],
            "white_banner",
            Some("banner"),
        ),
        (
            "white_wall_banner",
            &[("facing", "east")],
            [-1, 0, 0],
            "white_banner",
            Some("banner"),
        ),
        ("dandelion", &[], [0, -1, 0], "dandelion", None),
        ("poppy", &[], [0, -1, 0], "poppy", None),
        (
            "golden_dandelion",
            &[],
            [0, -1, 0],
            "golden_dandelion",
            None,
        ),
        ("rail", &[], [0, -1, 0], "rail", None),
        ("powered_rail", &[], [0, -1, 0], "powered_rail", None),
        ("detector_rail", &[], [0, -1, 0], "detector_rail", None),
        ("activator_rail", &[], [0, -1, 0], "activator_rail", None),
    ];
    let mut failures = Vec::new();
    for &(name, properties, delta, item, block_entity) in cases {
        let support = BlockPos {
            x: pos.x + delta[0],
            y: pos.y + delta[1],
            z: pos.z + delta[2],
        };
        let supporting = if matches!(name, "dandelion" | "poppy" | "golden_dandelion") {
            state.registry.states.dirt
        } else {
            state.registry.states.stone
        };
        let current = block(&state, name, properties);
        state
            .world
            .set_block(support.x, support.y, support.z, supporting);
        state.world.set_block(pos.x, pos.y, pos.z, current);
        state.world.fold_overlay();
        if let Some(id) = block_entity {
            let mut nbt = Vec::new();
            mc_nbt::write_network(
                &mut nbt,
                &Tag::Compound(vec![
                    ("id".into(), Tag::String(format!("minecraft:{id}"))),
                    ("x".into(), Tag::Int(pos.x)),
                    ("y".into(), Tag::Int(pos.y)),
                    ("z".into(), Tag::Int(pos.z)),
                ]),
            )
            .unwrap();
            state.world.set_opaque_block_entity(pos, Some(nbt));
        }
        assert!(
            super::supported(
                &state.registry.blocks,
                &state.registry.tags,
                pos,
                current,
                &|cell| state.world.block_at(cell.x, cell.y, cell.z)
            ),
            "{name}: valid attachment"
        );
        let air = state.registry.states.air;
        let unrelated = BlockPos {
            z: pos.z + 1,
            ..pos
        };
        let mut unchanged = Vec::new();
        crate::tick::pop_unsupported(&mut state, &mut vec![(unrelated, air)], &mut unchanged);
        assert_eq!(
            state.world.block_at(pos.x, pos.y, pos.z),
            Some(current),
            "{name}: unrelated update"
        );
        assert!(
            unchanged.is_empty(),
            "{name}: unrelated update must not drop an item"
        );
        state.world.set_block(support.x, support.y, support.z, air);
        let mut writes = vec![(support, air)];
        let mut spawned = Vec::new();
        crate::tick::pop_unsupported(&mut state, &mut writes, &mut spawned);
        if state.world.block_at(pos.x, pos.y, pos.z) != Some(air) || !writes.contains(&(pos, air)) {
            failures.push(format!("{name}: did not pop on support removal"));
        }
        let wanted = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{item}")).unwrap())
            .unwrap();
        let drops: Vec<_> = spawned
            .iter()
            .filter_map(|id| state.entities.slot_of(*id))
            .filter_map(|slot| state.entities.row(slot).item.as_ref())
            .map(|stack| (stack.item_id, stack.count))
            .collect();
        if drops != vec![(wanted, 1)] {
            failures.push(format!("{name}: expected one {item} item, got {drops:?}"));
        }
        if block_entity.is_some()
            && state
                .world
                .chunk(crate::boundary::ChunkPos { x: 0, z: 0 })
                .unwrap()
                .block_entities
                .contains_key(&pos)
        {
            failures.push(format!("{name}: retained removed block entity"));
        }
        state.world.set_block(pos.x, pos.y, pos.z, air);
        state.world.fold_overlay();
    }
    assert!(
        failures.is_empty(),
        "support-removal failures: {failures:#?}"
    );
}
/// Ilya (swarm): a valid oak door survived placement, then popped on its own
/// with only an adjacent interior y67 clear nearby. The door's support is its
/// floor cell; an adjacent same-level edit must not pop either half.
#[test]
fn oak_door_survives_adjacent_interior_clear() {
    use crate::crafter::tests::{block, fixture};

    let (mut state, _) = fixture(None);
    // Full vanilla loot so the support-loss drop asserts exactly one item.
    let vanilla = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    state.registry = std::sync::Arc::new(
        crate::registry::CoreRegistry::load(Some(&vanilla)).expect("full vanilla block loot"),
    );
    let air = state.registry.states.air;
    let stone = state.registry.states.stone;
    let lower_pos = BlockPos { x: 3, y: 201, z: 3 };
    let upper_pos = BlockPos {
        y: lower_pos.y + 1,
        ..lower_pos
    };
    let lower = block(
        &state,
        "oak_door",
        &[("facing", "west"), ("half", "lower"), ("hinge", "left")],
    );
    let upper = block(
        &state,
        "oak_door",
        &[("facing", "west"), ("half", "upper"), ("hinge", "left")],
    );
    state
        .world
        .set_block(lower_pos.x, lower_pos.y - 1, lower_pos.z, stone);
    state
        .world
        .set_block(lower_pos.x, lower_pos.y, lower_pos.z, lower);
    state
        .world
        .set_block(upper_pos.x, upper_pos.y, upper_pos.z, upper);
    // Adjacent interior cell with stray fill, as in the Ilya hut.
    let interior = BlockPos {
        x: lower_pos.x + 1,
        ..lower_pos
    };
    state
        .world
        .set_block(interior.x, interior.y, interior.z, stone);
    // Clearing that adjacent interior cell must not disturb the door.
    // Convention (matrix above): apply the source edit to the world first,
    // then hand the same write to pop_unsupported.
    state
        .world
        .set_block(interior.x, interior.y, interior.z, air);
    state.world.fold_overlay();
    let mut writes = vec![(interior, air)];
    let mut spawned = Vec::new();
    crate::tick::pop_unsupported(&mut state, &mut writes, &mut spawned);
    assert_eq!(
        state.world.block_at(lower_pos.x, lower_pos.y, lower_pos.z),
        Some(lower),
        "adjacent interior clear must not pop the door lower half"
    );
    assert_eq!(
        state.world.block_at(upper_pos.x, upper_pos.y, upper_pos.z),
        Some(upper),
        "adjacent interior clear must not pop the door upper half"
    );
    assert!(
        spawned.is_empty(),
        "adjacent interior clear must not drop a door item"
    );
    // Removing the floor under the door pops both halves whole with one item.
    let floor = BlockPos {
        y: lower_pos.y - 1,
        ..lower_pos
    };
    state.world.set_block(floor.x, floor.y, floor.z, air);
    state.world.fold_overlay();
    let mut writes = vec![(floor, air)];
    let mut spawned = Vec::new();
    crate::tick::pop_unsupported(&mut state, &mut writes, &mut spawned);
    assert_eq!(
        state.world.block_at(lower_pos.x, lower_pos.y, lower_pos.z),
        Some(air),
        "floor removal pops the door lower half"
    );
    assert_eq!(
        state.world.block_at(upper_pos.x, upper_pos.y, upper_pos.z),
        Some(air),
        "floor removal pops the door upper half with its pair"
    );
    let wanted = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_door").unwrap())
        .unwrap();
    let drops: Vec<_> = spawned
        .iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .map(|stack| (stack.item_id, stack.count))
        .collect();
    assert_eq!(
        drops,
        vec![(wanted, 1)],
        "legitimate support loss must drop exactly one oak_door item"
    );
}
