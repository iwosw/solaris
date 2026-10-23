//! End-city templates loaded once from the imported vanilla resource cache.
//! Assembly is Solaris-owned; resource geometry/markers and placement are data-driven.
use super::*;
use crate::structures::{StructureTemplate, TemplateBlock, TemplateChest};
use crate::village::piece::rotate_state;
use crate::village::solver::Rotation;
use mc_world::{BlockPos, GeneratedEntityMarker};
use std::path::Path;

const NAMES: [&str; 20] = [
    "base_floor",
    "second_floor_1",
    "second_floor_2",
    "third_floor_1",
    "third_floor_2",
    "base_roof",
    "second_roof",
    "third_roof",
    "tower_base",
    "tower_piece",
    "tower_floor",
    "tower_top",
    "fat_tower_base",
    "fat_tower_middle",
    "fat_tower_top",
    "bridge_end",
    "bridge_piece",
    "bridge_gentle_stairs",
    "bridge_steep_stairs",
    "ship",
];
struct Piece {
    blocks: Vec<TemplateBlock>,
    air: Vec<[i32; 3]>,
    chests: Vec<TemplateChest>,
    entities: Vec<crate::structures::TemplateBlockEntity>,
    markers: Vec<([i32; 3], String)>,
    min: [i32; 3],
    max: [i32; 3],
}
pub(super) struct Cities {
    pieces: Vec<[Piece; 4]>,
}
fn rotate([x, y, z]: [i32; 3], turn: usize) -> [i32; 3] {
    match turn {
        0 => [x, y, z],
        1 => [-z, y, x],
        2 => [-x, y, -z],
        _ => [z, y, -x],
    }
}
fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn rotated_piece(
    template: &StructureTemplate,
    registry: &BlockRegistry,
    turn: usize,
) -> Result<Piece, EndGeneratorError> {
    let rotation = [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::CounterClockwise90,
    ][turn];
    let mut blocks = Vec::new();
    let mut markers = Vec::new();
    let mut entities = Vec::new();
    for block in template.blocks() {
        if registry
            .by_id(block.state)
            .is_some_and(|s| s.block.id.path() == "structure_block")
        {
            continue;
        }
        blocks.push(TemplateBlock {
            pos: rotate(block.pos, turn),
            state: rotate_state(registry, block.state, rotation).ok_or_else(|| {
                EndGeneratorError::Resources("unresolved rotated End city block".into())
            })?,
        });
    }
    for entity in template.block_entities() {
        if registry
            .by_id(entity.state)
            .is_some_and(|s| s.block.id.path() == "structure_block")
        {
            if let Some((_, mc_nbt::Tag::String(value))) =
                entity.nbt.iter().find(|(key, _)| key == "metadata")
            {
                if !matches!(value.as_str(), "Sentry" | "Chest" | "Elytra") {
                    return Err(EndGeneratorError::Resources(format!(
                        "unsupported End city marker {value}"
                    )));
                }
                markers.push((rotate(entity.pos, turn), value.clone()));
            }
        } else {
            let mut entity = entity.clone();
            entity.pos = rotate(entity.pos, turn);
            entity.state = rotate_state(registry, entity.state, rotation).ok_or_else(|| {
                EndGeneratorError::Resources("unresolved rotated End city block entity".into())
            })?;
            entities.push(entity);
        }
    }
    let mut chests = template.chests().to_vec();
    for chest in &mut chests {
        chest.pos = rotate(chest.pos, turn);
        if markers.iter().any(|(p, kind)| {
            kind == "Chest" && *p == [chest.pos[0], chest.pos[1] + 1, chest.pos[2]]
        }) {
            chest.loot_table = Some(
                Identifier::parse("minecraft:chests/end_city_treasure").expect("static identifier"),
            );
        }
    }
    let size = template.size();
    let corners = [[0, 0, 0], [size[0] - 1, size[1] - 1, size[2] - 1]].map(|p| rotate(p, turn));
    Ok(Piece {
        blocks,
        air: template
            .air_cells()
            .iter()
            .map(|p| rotate(*p, turn))
            .collect(),
        chests,
        entities,
        markers,
        min: std::array::from_fn(|i| corners[0][i].min(corners[1][i])),
        max: std::array::from_fn(|i| corners[0][i].max(corners[1][i])),
    })
}
impl Cities {
    fn load(root: &Path, registry: &BlockRegistry) -> Result<Self, EndGeneratorError> {
        let mut pieces = Vec::with_capacity(NAMES.len());
        for name in NAMES {
            let template = StructureTemplate::from_nbt_file(
                root.join(format!("data/minecraft/structure/end_city/{name}.nbt")),
                registry,
            )
            .map_err(|e| EndGeneratorError::Resources(e.to_string()))?;
            pieces.push([
                rotated_piece(&template, registry, 0)?,
                rotated_piece(&template, registry, 1)?,
                rotated_piece(&template, registry, 2)?,
                rotated_piece(&template, registry, 3)?,
            ]);
        }
        Ok(Self { pieces })
    }
    pub(super) fn paste(
        &self,
        generator: &EndGenerator,
        chunk: &mut Chunk,
        origin: [i32; 3],
        hash: u64,
        markers: &mut Vec<GeneratedEntityMarker>,
    ) {
        let turn = (hash % 4) as usize;
        let mut index = 0_u32;
        let record_start =
            chunk.pos.x == origin[0].div_euclid(16) && chunk.pos.z == origin[2].div_euclid(16);
        let mut min = [i32::MAX; 3];
        let mut max = [i32::MIN; 3];
        let mut paste = |kind: usize, offset: [i32; 3], local_turn: usize| {
            let piece_turn = (turn + local_turn) % 4;
            let piece = &self.pieces[kind][piece_turn];
            let mut at = add(origin, rotate(offset, turn));
            // Keep the generated frame and its backing in one chunk. This is
            // a one-block ship translation, independent of chunk arrival order.
            if kind == 19 {
                let frame = add(at, rotate([6, 5, 7], piece_turn));
                let backing = rotate([0, 0, -1], piece_turn);
                for axis in [0, 2] {
                    if frame[axis].div_euclid(16) != (frame[axis] + backing[axis]).div_euclid(16) {
                        at[axis] -= backing[axis];
                    }
                }
            }
            if record_start {
                for axis in 0..3 {
                    min[axis] = min[axis].min(at[axis] + piece.min[axis]);
                    max[axis] = max[axis].max(at[axis] + piece.max[axis]);
                }
            }
            index += 1;
            let bx = chunk.pos.x * 16;
            let bz = chunk.pos.z * 16;
            if at[0] + piece.max[0] < bx
                || at[0] + piece.min[0] >= bx + 16
                || at[2] + piece.max[2] < bz
                || at[2] + piece.min[2] >= bz + 16
            {
                return;
            }
            // Only the standalone ship clears authored air. Stacked city floors
            // overlap stair wells; their air must not erase the preceding floor.
            if kind == 19 {
                for p in &piece.air {
                    let p = add(at, *p);
                    super::outer::put(chunk, p[0], p[1], p[2], generator.air);
                }
            }
            for block in &piece.blocks {
                let p = add(at, block.pos);
                super::outer::put(chunk, p[0], p[1], p[2], block.state);
            }
            let inside = |p: [i32; 3]| {
                p[0] >= bx
                    && p[0] < bx + 16
                    && p[2] >= bz
                    && p[2] < bz + 16
                    && p[1] >= 0
                    && p[1] < 256
            };
            for chest in &piece.chests {
                let p = add(at, chest.pos);
                if !inside(p) {
                    continue;
                }
                let pos = BlockPos {
                    x: p[0],
                    y: p[1],
                    z: p[2],
                };
                chunk.chests.insert(
                    pos,
                    chest.unopened_contents(splitmix64(
                        hash ^ u64::from(index) ^ (p[0] as u64).rotate_left(23) ^ p[2] as u64,
                    )),
                );
            }
            for entity in &piece.entities {
                let p = add(at, entity.pos);
                if inside(p) {
                    let pos = BlockPos {
                        x: p[0],
                        y: p[1],
                        z: p[2],
                    };
                    chunk
                        .block_entities
                        .insert(pos, entity.at_world_position(pos));
                }
            }
            for (p, kind) in &piece.markers {
                let p = add(at, *p);
                if !inside(p) {
                    continue;
                }
                super::outer::put(chunk, p[0], p[1], p[2], generator.air);
                if kind == "Chest" {
                    continue;
                }
                let reward = kind == "Elytra";
                markers.push(GeneratedEntityMarker {
                    claim: format!(
                        "minecraft:end_city/{}@{}:{}:{}#{index}:{}:{}:{}{}",
                        if reward { "ship" } else { "guard" },
                        origin[0],
                        origin[1],
                        origin[2],
                        p[0],
                        p[1],
                        p[2],
                        if reward { "#elytra" } else { "" }
                    ),
                    entity_type: if reward {
                        "minecraft:item_frame"
                    } else {
                        "minecraft:shulker"
                    }
                    .into(),
                    position: [
                        f64::from(p[0]) + 0.5,
                        f64::from(p[1]),
                        f64::from(p[2]) + 0.5,
                    ],
                    persistent: true,
                    villager_kind: String::new(),
                    profession: String::new(),
                    level: 1,
                    age: 0,
                    yaw: piece_turn as f32 * 90.0,
                    pitch: 0.0,
                });
            }
        };
        paste(0, [0, 0, 0], 0);
        paste(1 + ((hash >> 3) & 1) as usize, [-1, 0, -1], 0);
        paste(3 + ((hash >> 4) & 1) as usize, [-2, 4, -2], 0);
        paste(7, [-3, 12, -3], 0);
        paste(8, [1, 12, 1], 0);
        let floors = 2 + ((hash >> 5) % 4) as i32;
        for floor in 0..floors {
            paste(9, [1, 19 + floor * 4, 1], 0);
        }
        let top = 19 + floors * 4;
        paste(11, [0, top, 0], 0);
        // Minecraft Wiki: each tower direction has a 50% bridge chance;
        // each bridge has a 1/8 ship chance, capped at one ship per city.
        let mut ship = false;
        for side in 0..4 {
            let branch = splitmix64(hash ^ (side as u64).wrapping_add(0x00B4_1D6E));
            if branch & 1 == 0 {
                continue;
            }
            let offset = |[x, y, z]: [i32; 3]| add([4, 0, 4], rotate([x - 4, y, z - 4], side));
            for segment in 0..5 {
                paste(16, offset([2, top, 7 + segment * 4]), side);
            }
            if !ship && (branch >> 1).is_multiple_of(8) {
                paste(19, offset([-2, top - 5, 32]), side);
                ship = true;
            } else {
                paste(12, offset([-2, top, 27]), side);
                let wide_floors = 1 + ((branch >> 5) % 3) as i32;
                for floor in 0..wide_floors {
                    paste(13, offset([-2, top + 4 + floor * 8, 27]), side);
                }
                paste(14, offset([-4, top + 4 + wide_floors * 8, 25]), side);
            }
        }
        if record_start {
            mc_world::structure_starts::insert(
                chunk,
                &mc_world::chunk::GeneratedStructureSite {
                    structure: Identifier::parse("minecraft:end_city").expect("static identifier"),
                    start_chunk: (chunk.pos.x, chunk.pos.z),
                    position: BlockPos {
                        x: origin[0],
                        y: origin[1],
                        z: origin[2],
                    },
                },
                BlockPos {
                    x: min[0],
                    y: min[1],
                    z: min[2],
                },
                BlockPos {
                    x: max[0],
                    y: max[1],
                    z: max[2],
                },
            );
        }
    }
}
impl EndGenerator {
    /// Load the End placement set, templates and gateway features at startup.
    /// Invalid or missing imported content fails startup, never drops a reward.
    pub fn with_vanilla_data(
        mut self,
        root: &Path,
        registry: &BlockRegistry,
    ) -> Result<Self, EndGeneratorError> {
        use mc_data::village_data::{PlacementSpec, RandomSpreadType, VillageDataLoader};
        let loader = VillageDataLoader::new(root.join("data/minecraft/worldgen"));
        let set = loader
            .load_structure_set(&Identifier::parse("minecraft:end_cities").unwrap())
            .map_err(|e| EndGeneratorError::Resources(e.to_string()))?;
        let PlacementSpec::RandomSpread(placement) = set.placement;
        if set.structures.len() != 1
            || set.structures[0].structure.as_str() != "minecraft:end_city"
            || placement.frequency != 1.0
        {
            return Err(EndGeneratorError::Resources(
                "unsupported End city structure set".into(),
            ));
        }
        self.outer.placement = crate::village::placement::RandomSpreadPlacement {
            spacing: placement.spacing,
            separation: placement.separation,
            salt: i64::from(placement.salt),
            triangular: placement.spread_type == RandomSpreadType::Triangular,
        };
        self.cities = Some(Cities::load(root, registry)?);
        self.gateways = Some(super::gateways::Gateways::load(root, registry)?);
        Ok(self)
    }
}
