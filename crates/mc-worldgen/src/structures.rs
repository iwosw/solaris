use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::bufread::GzDecoder;
use mc_data::Identifier;
use mc_data::items::ItemRegistry;
use mc_data::loot::chest_26_1_2::{CHEST_SLOT_COUNT, ChestLootCatalog, ChestRng};
use mc_nbt::{ListTag, Tag};
use mc_world::{BlockRegistry, BlockStateId, ChestBlockEntity, FurnaceSlot};
use thiserror::Error;

const MAX_STRUCTURE_COMPRESSED_NBT_BYTES: usize = mc_nbt::MAX_NBT_TOTAL_BYTES;
const MAX_STRUCTURE_DECOMPRESSED_NBT_BYTES: usize = mc_nbt::MAX_NBT_TOTAL_BYTES;

#[derive(Debug, Error)]
pub enum StructureError {
    #[error("reading structure template {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("decoding NBT structure template {path}: {source}")]
    Nbt {
        path: String,
        #[source]
        source: mc_nbt::NbtError,
    },
    #[error("structure template {kind} at {path} is at least {bytes} bytes, exceeding limit {max}")]
    DataTooLarge {
        path: String,
        kind: &'static str,
        bytes: usize,
        max: usize,
    },
    #[error("structure template {kind} at {path} has {bytes} trailing byte(s)")]
    TrailingData {
        path: String,
        kind: &'static str,
        bytes: usize,
    },
    #[error("structure template {path} is missing {field}")]
    MissingField { path: String, field: &'static str },
    #[error("structure template {path} has invalid {field}")]
    InvalidField { path: String, field: &'static str },
    #[error("structure template {path} references unknown block {block}")]
    UnknownBlock { path: String, block: Identifier },
    #[error("structure template {path} references unresolved block state {block}")]
    UnknownState { path: String, block: Identifier },
}

#[derive(Debug, Clone)]
pub struct StructureTemplate {
    size: [i32; 3],
    blocks: Vec<TemplateBlock>,
    chests: Vec<TemplateChest>,
    villager_markers: Vec<[i32; 3]>,
    jigsaws: Vec<TemplateJigsaw>,
    entities: Vec<TemplateEntity>,
}

/// `minecraft:jigsaw` joint type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Joint {
    Aligned,
    Rollable,
}

/// A block face, as the jigsaw block's `orientation` property names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockFace {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl BlockFace {
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
        }
    }

    #[must_use]
    pub fn step(self) -> [i32; 3] {
        match self {
            Self::Down => [0, -1, 0],
            Self::Up => [0, 1, 0],
            Self::North => [0, 0, -1],
            Self::South => [0, 0, 1],
            Self::West => [-1, 0, 0],
            Self::East => [1, 0, 0],
        }
    }
}

/// One `minecraft:jigsaw` block of a piece, in template-local coordinates.
///
/// `front`/`top` come from the jigsaw state's `orientation`
/// (`JigsawBlock.getFrontFacing`/`getTopFacing` are
/// `state.getValue(ORIENTATION).front()/top()`), and `final_state` is the state
/// the jigsaw is replaced with once the structure is placed.
#[derive(Debug, Clone)]
pub struct TemplateJigsaw {
    pub pos: [i32; 3],
    pub state: BlockStateId,
    pub front: BlockFace,
    pub top: BlockFace,
    pub name: Identifier,
    pub target: Identifier,
    pub pool: Identifier,
    pub joint: Joint,
    pub final_state: BlockStateId,
    /// `JigsawBlockInfo.selection_priority`: the order the jigsaws of one piece
    /// are offered in, highest first (`SinglePoolElement.sortBySelectionPriority`).
    pub selection_priority: i32,
    /// `JigsawBlockInfo.placement_priority`: the priority the pieces placed
    /// through this jigsaw are queued at (`JigsawPlacement$Placer`, a
    /// `SequencedPriorityIterator`).
    pub placement_priority: i32,
}

/// `FrontAndTop`'s names, as they appear in a jigsaw state's `orientation`.
const ORIENTATIONS: &[(&str, BlockFace, BlockFace)] = &[
    ("down_east", BlockFace::Down, BlockFace::East),
    ("down_north", BlockFace::Down, BlockFace::North),
    ("down_south", BlockFace::Down, BlockFace::South),
    ("down_west", BlockFace::Down, BlockFace::West),
    ("up_east", BlockFace::Up, BlockFace::East),
    ("up_north", BlockFace::Up, BlockFace::North),
    ("up_south", BlockFace::Up, BlockFace::South),
    ("up_west", BlockFace::Up, BlockFace::West),
    ("west_up", BlockFace::West, BlockFace::Up),
    ("east_up", BlockFace::East, BlockFace::Up),
    ("north_up", BlockFace::North, BlockFace::Up),
    ("south_up", BlockFace::South, BlockFace::Up),
];

/// Decode `FrontAndTop` from a jigsaw state's `orientation` value.
#[must_use]
pub fn orientation_faces(orientation: &str) -> Option<(BlockFace, BlockFace)> {
    ORIENTATIONS
        .iter()
        .find(|(name, _, _)| *name == orientation)
        .map(|(_, front, top)| (*front, *top))
}

#[derive(Debug, Clone, Copy)]
pub struct TemplateBlock {
    pub pos: [i32; 3],
    pub state: BlockStateId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateChest {
    pub pos: [i32; 3],
    pub chest: ChestBlockEntity,
    /// Vanilla chest table rolled with the production catalog, retaining
    /// canonical item components in the generated block entity.
    pub loot_table: Option<Identifier>,
}

impl TemplateChest {
    /// Resolve the production table and distribute canonical stacks into
    /// distinct slots. Missing catalog entries remain unopened vanilla loot
    /// instead of substituting a handcrafted inventory.
    #[must_use]
    pub fn resolve_contents(&self, loot: &StructureLoot<'_>, loot_seed: u64) -> ChestBlockEntity {
        let Some(table_id) = &self.loot_table else {
            return self.chest.clone();
        };
        let Some(table) = loot.catalog.get(table_id) else {
            return ChestBlockEntity {
                loot_table: Some(table_id.clone()),
                loot_seed: loot_seed as i64,
                ..ChestBlockEntity::default()
            };
        };
        let mut rng = ChestRng::new(loot_seed);
        let mut out = ChestBlockEntity::default();
        let mut available: [usize; CHEST_SLOT_COUNT as usize] = std::array::from_fn(|slot| slot);
        let mut remaining = available.len();
        for stack in table
            .roll(&mut rng, loot.items, 0.0)
            .expect("production structure loot")
        {
            if remaining == 0 {
                break;
            }
            let choice = rng.next_bounded(remaining as u64) as usize;
            let slot = available[choice];
            remaining -= 1;
            available[choice] = available[remaining];
            out.slots[slot] = FurnaceSlot {
                item_id: stack.item_id,
                count: stack.count,
                damage: stack.damage,
                enchantments: stack.enchantments,
                custom_name: stack.custom_name.map(|name| name.into_string()),
                item_model: stack.item_model,
                stew_effects: stack.stew_effects,
                components: stack.components,
            };
        }
        out
    }
}

/// Loot context threaded through structure pasting: the world seed plus the
/// compiled chest tables and the item registry that resolves rolled stacks.
/// A chest whose table id is absent from the catalog pastes its fixed
/// contents instead, so generators without wired loot keep legacy output.
#[derive(Debug)]
pub struct StructureLoot<'a> {
    pub seed: i64,
    pub catalog: &'a ChestLootCatalog,
    pub items: &'a ItemRegistry,
}

enum BoundedLength {
    Exact(usize),
    TooLarge { at_least: usize },
}

fn count_bounded_bytes(reader: &mut impl Read, max: usize) -> std::io::Result<BoundedLength> {
    let mut total = 0_usize;
    let mut scratch = [0_u8; 8192];
    loop {
        let remaining = max.saturating_sub(total);
        if remaining == 0 {
            let mut extra = [0_u8; 1];
            return if reader.read(&mut extra)? == 0 {
                Ok(BoundedLength::Exact(total))
            } else {
                Ok(BoundedLength::TooLarge {
                    at_least: max.saturating_add(1),
                })
            };
        }

        let chunk_len = remaining.min(scratch.len());
        let read = reader.read(&mut scratch[..chunk_len])?;
        if read == 0 {
            return Ok(BoundedLength::Exact(total));
        }
        total = total.saturating_add(read);
    }
}

fn allocate_exact_bytes(len: usize, context: &'static str) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|error| {
        std::io::Error::other(format!("reserve exact {context} buffer: {error}"))
    })?;
    bytes.resize(len, 0);
    Ok(bytes)
}

fn read_bounded_structure_file(path: &Path, max: usize) -> Result<Vec<u8>, StructureError> {
    let display = path.display().to_string();
    // The opened descriptor pins the file object; later path replacement cannot
    // switch this read to a different inode, and concurrent growth is caught by
    // the one-byte-over-limit probe.
    let mut file = File::open(path).map_err(|source| StructureError::Io {
        path: display.clone(),
        source,
    })?;
    let len = match count_bounded_bytes(&mut file, max).map_err(|source| StructureError::Io {
        path: display.clone(),
        source,
    })? {
        BoundedLength::Exact(len) => len,
        BoundedLength::TooLarge { at_least } => {
            return Err(StructureError::DataTooLarge {
                path: display,
                kind: "compressed or raw NBT",
                bytes: at_least,
                max,
            });
        }
    };
    file.seek(SeekFrom::Start(0))
        .map_err(|source| StructureError::Io {
            path: display.clone(),
            source,
        })?;
    let mut bytes =
        allocate_exact_bytes(len, "structure input").map_err(|source| StructureError::Io {
            path: display.clone(),
            source,
        })?;
    file.read_exact(&mut bytes)
        .map_err(|source| StructureError::Io {
            path: display.clone(),
            source,
        })?;
    let mut extra = [0_u8; 1];
    if file.read(&mut extra).map_err(|source| StructureError::Io {
        path: display.clone(),
        source,
    })? != 0
    {
        return Err(StructureError::Io {
            path: display,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "structure file changed while being read",
            ),
        });
    }
    Ok(bytes)
}

fn decode_single_structure_gzip_member(
    display: &str,
    compressed: &[u8],
    max: usize,
) -> Result<Vec<u8>, StructureError> {
    let mut decoder = GzDecoder::new(Cursor::new(compressed));
    let decoded_len =
        match count_bounded_bytes(&mut decoder, max).map_err(|source| StructureError::Io {
            path: display.to_owned(),
            source,
        })? {
            BoundedLength::Exact(len) => len,
            BoundedLength::TooLarge { at_least } => {
                return Err(StructureError::DataTooLarge {
                    path: display.to_owned(),
                    kind: "decompressed NBT",
                    bytes: at_least,
                    max,
                });
            }
        };
    let consumed = decoder.into_inner().position() as usize;
    if consumed != compressed.len() {
        return Err(StructureError::TrailingData {
            path: display.to_owned(),
            kind: "compressed gzip member",
            bytes: compressed.len().saturating_sub(consumed),
        });
    }

    let mut decoded =
        allocate_exact_bytes(decoded_len, "decompressed structure NBT").map_err(|source| {
            StructureError::Io {
                path: display.to_owned(),
                source,
            }
        })?;
    let mut decoder = GzDecoder::new(Cursor::new(compressed));
    decoder
        .read_exact(&mut decoded)
        .map_err(|source| StructureError::Io {
            path: display.to_owned(),
            source,
        })?;
    let mut extra = [0_u8; 1];
    if decoder
        .read(&mut extra)
        .map_err(|source| StructureError::Io {
            path: display.to_owned(),
            source,
        })?
        != 0
    {
        return Err(StructureError::Io {
            path: display.to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "gzip output changed between bounded passes",
            ),
        });
    }
    let consumed = decoder.into_inner().position() as usize;
    if consumed != compressed.len() {
        return Err(StructureError::TrailingData {
            path: display.to_owned(),
            kind: "compressed gzip member",
            bytes: compressed.len().saturating_sub(consumed),
        });
    }
    Ok(decoded)
}

impl StructureTemplate {
    #[must_use]
    pub fn new(size: [i32; 3], blocks: Vec<TemplateBlock>) -> Self {
        Self {
            size,
            blocks,
            chests: Vec::new(),
            villager_markers: Vec::new(),
            jigsaws: Vec::new(),
            entities: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_chests(mut self, chests: Vec<TemplateChest>) -> Self {
        self.chests = chests;
        self
    }

    #[must_use]
    pub fn with_jigsaws(mut self, jigsaws: Vec<TemplateJigsaw>) -> Self {
        self.jigsaws = jigsaws;
        self
    }

    pub fn from_nbt_file(
        path: impl AsRef<Path>,
        registry: &BlockRegistry,
    ) -> Result<Self, StructureError> {
        let path = path.as_ref();
        let display = path.display().to_string();
        let raw = read_bounded_structure_file(path, MAX_STRUCTURE_COMPRESSED_NBT_BYTES)?;
        let decoded = if raw.starts_with(&[0x1f, 0x8b]) {
            decode_single_structure_gzip_member(
                &display,
                &raw,
                MAX_STRUCTURE_DECOMPRESSED_NBT_BYTES,
            )?
        } else {
            raw
        };

        let mut bytes = decoded.as_slice();
        let (_name, root) =
            mc_nbt::read_named(&mut bytes).map_err(|source| StructureError::Nbt {
                path: display.clone(),
                source,
            })?;
        if !bytes.is_empty() {
            return Err(StructureError::TrailingData {
                path: display,
                kind: "decompressed NBT",
                bytes: bytes.len(),
            });
        }
        Self::from_tag(&display, &root, registry)
    }

    #[must_use]
    pub fn size(&self) -> [i32; 3] {
        self.size
    }

    #[must_use]
    pub fn blocks(&self) -> &[TemplateBlock] {
        &self.blocks
    }

    #[must_use]
    pub fn chests(&self) -> &[TemplateChest] {
        &self.chests
    }

    #[must_use]
    pub fn villager_markers(&self) -> &[[i32; 3]] {
        &self.villager_markers
    }

    /// The piece's jigsaw blocks, in template-local coordinates.
    #[must_use]
    pub fn jigsaws(&self) -> &[TemplateJigsaw] {
        &self.jigsaws
    }

    /// The entities the template places, in template-local coordinates: vanilla
    /// `StructureTemplate.load`'s `entities` list.
    #[must_use]
    pub fn entities(&self) -> &[TemplateEntity] {
        &self.entities
    }

    #[must_use]
    pub fn with_entities(mut self, entities: Vec<TemplateEntity>) -> Self {
        self.entities = entities;
        self
    }

    fn from_tag(path: &str, root: &Tag, registry: &BlockRegistry) -> Result<Self, StructureError> {
        let compound = expect_compound(path, root, "root")?;
        let size = expect_int_triplet(path, require(compound, path, "size")?, "size")?;
        let palette = parse_palette(path, require(compound, path, "palette")?, registry)?;
        let parsed = parse_blocks(path, require(compound, path, "blocks")?, &palette, registry)?;
        let mut template = Self::new(size, parsed.blocks);
        template.villager_markers = parsed.villager_markers;
        template.chests = parsed.chests;
        template.jigsaws = parsed.jigsaws;
        template.entities = parse_entities(path, compound)?;
        Ok(template)
    }
}

#[derive(Debug, Clone, Copy)]
enum TemplatePaletteEntry {
    Block(BlockStateId),
    /// A `minecraft:jigsaw` state whose orientation supplies the connector face.
    Jigsaw(BlockStateId),
    Ignored,
}

fn parse_palette(
    path: &str,
    tag: &Tag,
    registry: &BlockRegistry,
) -> Result<Vec<TemplatePaletteEntry>, StructureError> {
    let list = expect_list(path, tag, "palette")?;
    list.elements
        .iter()
        .map(|entry| parse_palette_entry(path, entry, registry))
        .collect()
}

fn parse_palette_entry(
    path: &str,
    entry: &Tag,
    registry: &BlockRegistry,
) -> Result<TemplatePaletteEntry, StructureError> {
    let compound = expect_compound(path, entry, "palette[]")?;
    let name = expect_string(path, require(compound, path, "Name")?, "Name")?;
    let id = Identifier::parse(name.clone()).map_err(|_| StructureError::InvalidField {
        path: path.to_string(),
        field: "palette[].Name",
    })?;
    if id.as_str() == "minecraft:jigsaw" {
        let props = match compound.iter().find(|(key, _)| key == "Properties") {
            Some((_, tag)) => parse_properties(path, tag)?,
            None => Vec::new(),
        };
        let state = registry.by_name_and_props(&id, &props).ok_or_else(|| {
            StructureError::InvalidField {
                path: path.to_string(),
                field: "palette[].jigsaw orientation",
            }
        })?;
        return Ok(TemplatePaletteEntry::Jigsaw(state));
    }
    if matches!(id.as_str(), "minecraft:air" | "minecraft:structure_void") {
        return Ok(TemplatePaletteEntry::Ignored);
    }
    let props = match compound.iter().find(|(key, _)| key == "Properties") {
        Some((_, tag)) => parse_properties(path, tag)?,
        None => Vec::new(),
    };
    if registry.block(&id).is_none() {
        return Err(StructureError::UnknownBlock {
            path: path.to_string(),
            block: id,
        });
    }
    registry
        .by_name_and_props(&id, &props)
        .map(TemplatePaletteEntry::Block)
        .ok_or_else(|| StructureError::UnknownState {
            path: path.to_string(),
            block: id,
        })
}

fn parse_properties(path: &str, tag: &Tag) -> Result<Vec<(String, String)>, StructureError> {
    let compound = expect_compound(path, tag, "Properties")?;
    compound
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                expect_string(path, value, "Properties[]")?.clone(),
            ))
        })
        .collect()
}

/// One entity a structure template places, in template-local coordinates.
///
/// `StructureTemplate.load`'s `entities` list: `pos` (the double position the
/// entity is moved to) and `blockPos` (the block the piece's bounding box is
/// tested with), both defaulting to zero when absent, plus the `nbt` compound
/// the entity is created from. Vanilla drops an entry whose `nbt` is missing.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateEntity {
    /// The NBT `id`, e.g. `minecraft:villager`.
    pub entity_type: String,
    /// The NBT `pos`, in template-local doubles.
    pub position: [f64; 3],
    /// The NBT `blockPos`, in template-local blocks.
    pub block_position: [i32; 3],
    /// The NBT `Rotation`'s yaw (`Rotation[0]`, `Entity.getYRot` for a freshly
    /// created entity), `0.0` when the template omits it.
    pub yaw: f32,
    /// The NBT `Rotation`'s pitch (`Rotation[1]`), `0.0` when the template omits
    /// it. Vanilla clamps it when the entity loads it, not when the piece places
    /// it, so the authored value is kept here.
    pub pitch: f32,
    /// The entity's `VillagerData` and `Age`, when its NBT carries them.
    pub villager: Option<TemplateVillagerData>,
}

/// The villager state a template entity's `nbt` authors.
///
/// The names stay the NBT's own (`VillagerData.type`/`profession` paths, e.g.
/// `plains`/`none`): the core's marker layer is where they are resolved, and the
/// closure keeps what the data says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateVillagerData {
    /// `VillagerData.type`'s path, e.g. `plains`.
    pub kind: String,
    /// `VillagerData.profession`'s path, e.g. `none` or `nitwit`.
    pub profession: String,
    /// `VillagerData.level`.
    pub level: u8,
    /// `Age`: negative for a baby, `0` for an adult.
    pub age: i32,
}

/// Read one template's `entities` list. An absent list is no entities; a
/// malformed entry is a load failure, named by its field.
fn parse_entities(
    path: &str,
    root: &[(String, Tag)],
) -> Result<Vec<TemplateEntity>, StructureError> {
    let Some(tag) = root
        .iter()
        .find(|(key, _)| key == "entities")
        .map(|(_, v)| v)
    else {
        return Ok(Vec::new());
    };
    let list = expect_list(path, tag, "entities")?;
    let mut entities = Vec::with_capacity(list.elements.len());
    for entry in &list.elements {
        let compound = expect_compound(path, entry, "entities[]")?;
        // `getDoubleOr`/`getIntOr`: a short or absent list reads as zeros, and a
        // missing `nbt` drops the entry outright.
        let position = optional_double_triplet(path, compound, "pos")?;
        let block_position = optional_int_triplet(path, compound, "blockPos")?;
        let Some(nbt) = compound
            .iter()
            .find(|(key, _)| key == "nbt")
            .map(|(_, tag)| tag)
            .and_then(|tag| match tag {
                Tag::Compound(fields) => Some(fields),
                _ => None,
            })
        else {
            continue;
        };
        let entity_type = nbt
            .iter()
            .find(|(key, _)| key == "id")
            .and_then(|(_, tag)| match tag {
                Tag::String(value) => Some(value.clone()),
                _ => None,
            })
            .ok_or_else(|| StructureError::InvalidField {
                path: path.to_string(),
                field: "entities[].nbt.id",
            })?;
        // `Rotation` is `Vec2(yRot, xRot)`: `Entity.load` sets the yaw from index
        // 0 and the pitch from index 1, each defaulting to zero when the list is
        // short.
        let (yaw, pitch) = match nbt.iter().find(|(key, _)| key == "Rotation") {
            None => (0.0, 0.0),
            Some((_, Tag::List(rotation))) => {
                let component = |index: usize| match rotation.elements.get(index) {
                    None => Ok(0.0),
                    Some(Tag::Float(value)) => Ok(*value),
                    Some(_) => Err(StructureError::InvalidField {
                        path: path.to_string(),
                        field: "entities[].nbt.Rotation[]",
                    }),
                };
                (component(0)?, component(1)?)
            }
            Some(_) => {
                return Err(StructureError::InvalidField {
                    path: path.to_string(),
                    field: "entities[].nbt.Rotation",
                });
            }
        };
        let villager = nbt
            .iter()
            .find(|(key, _)| key == "VillagerData")
            .map(|(_, tag)| -> Result<TemplateVillagerData, StructureError> {
                let data = expect_compound(path, tag, "entities[].nbt.VillagerData")?;
                let text = |field: &'static str| -> Result<String, StructureError> {
                    data.iter()
                        .find(|(key, _)| key == field)
                        .and_then(|(_, tag)| match tag {
                            Tag::String(value) => Some(value.clone()),
                            _ => None,
                        })
                        .ok_or_else(|| StructureError::InvalidField {
                            path: path.to_string(),
                            field: "entities[].nbt.VillagerData",
                        })
                };
                // Vanilla reads `level` through `VillagerData.CODEC`, whose
                // range is 1..=5 (`VillagerData.MIN_LEVEL`/`MAX_LEVEL`); a value
                // outside it is a data error, not something to clamp.
                let level = expect_int(
                    path,
                    require(data, path, "level")?,
                    "entities[].nbt.VillagerData.level",
                )?;
                let level = u8::try_from(level)
                    .ok()
                    .filter(|level| (1..=5).contains(level))
                    .ok_or_else(|| StructureError::InvalidField {
                        path: path.to_string(),
                        field: "entities[].nbt.VillagerData.level",
                    })?;
                Ok(TemplateVillagerData {
                    kind: Identifier::parse(text("type")?)
                        .map_err(|_| StructureError::InvalidField {
                            path: path.to_string(),
                            field: "entities[].nbt.VillagerData.type",
                        })?
                        .path()
                        .to_owned(),
                    profession: Identifier::parse(text("profession")?)
                        .map_err(|_| StructureError::InvalidField {
                            path: path.to_string(),
                            field: "entities[].nbt.VillagerData.profession",
                        })?
                        .path()
                        .to_owned(),
                    level,
                    // `Age` is the entity's own tag, next to `VillagerData`:
                    // negative for a baby. A template that omits it authors an
                    // adult.
                    age: match nbt.iter().find(|(key, _)| key == "Age") {
                        None => 0,
                        Some((_, tag)) => expect_int(path, tag, "entities[].nbt.Age")?,
                    },
                })
            })
            .transpose()?;
        entities.push(TemplateEntity {
            entity_type,
            position,
            block_position,
            yaw,
            pitch,
            villager,
        });
    }
    Ok(entities)
}

/// An optional NBT double list, read as `[f64; 3]`. `NbtUtils`' `getDoubleOr`
/// answers `0.0` for a missing index, so a short list reads as zeros.
fn optional_double_triplet(
    path: &str,
    compound: &[(String, Tag)],
    field: &'static str,
) -> Result<[f64; 3], StructureError> {
    let Some((_, tag)) = compound.iter().find(|(key, _)| key == field) else {
        return Ok([0.0; 3]);
    };
    let list = expect_list(path, tag, field)?;
    let invalid = || StructureError::InvalidField {
        path: path.to_string(),
        field,
    };
    let mut out = [0.0; 3];
    for (slot, element) in list.elements.iter().take(3).enumerate() {
        match element {
            Tag::Double(value) => out[slot] = *value,
            _ => return Err(invalid()),
        }
    }
    Ok(out)
}

/// An optional NBT int list, read as `[i32; 3]` (`getIntOr`).
fn optional_int_triplet(
    path: &str,
    compound: &[(String, Tag)],
    field: &'static str,
) -> Result<[i32; 3], StructureError> {
    let Some((_, tag)) = compound.iter().find(|(key, _)| key == field) else {
        return Ok([0; 3]);
    };
    let list = expect_list(path, tag, field)?;
    let invalid = || StructureError::InvalidField {
        path: path.to_string(),
        field,
    };
    let mut out = [0; 3];
    for (slot, element) in list.elements.iter().take(3).enumerate() {
        match element {
            Tag::Int(value) => out[slot] = *value,
            _ => return Err(invalid()),
        }
    }
    Ok(out)
}

/// What one piece's `blocks` list yields.
struct ParsedBlocks {
    blocks: Vec<TemplateBlock>,
    villager_markers: Vec<[i32; 3]>,
    jigsaws: Vec<TemplateJigsaw>,
    chests: Vec<TemplateChest>,
}

fn parse_blocks(
    path: &str,
    tag: &Tag,
    palette: &[TemplatePaletteEntry],
    registry: &BlockRegistry,
) -> Result<ParsedBlocks, StructureError> {
    let list = expect_list(path, tag, "blocks")?;
    let mut blocks = Vec::new();
    let mut villager_markers = Vec::new();
    let mut jigsaws = Vec::new();
    let mut chests = Vec::new();
    for entry in &list.elements {
        let compound = expect_compound(path, entry, "blocks[]")?;
        let pos = expect_int_triplet(path, require(compound, path, "pos")?, "blocks[].pos")?;
        let state_idx = expect_int(path, require(compound, path, "state")?, "blocks[].state")?;
        let state =
            palette
                .get(state_idx as usize)
                .ok_or_else(|| StructureError::InvalidField {
                    path: path.to_string(),
                    field: "blocks[].state",
                })?;
        match state {
            TemplatePaletteEntry::Block(state) => {
                blocks.push(TemplateBlock { pos, state: *state });
                // Vanilla `StructureTemplate` stores every block's parsed
                // block-entity payload in `blocks[].nbt`; this loader used to
                // read it only for jigsaws, so village chest blocks pasted as
                // BE-less states (invisible to the client). Lift chest
                // containers here so `place_piece` pastes them through
                // `PieceWriter::set_chest` like the hand-declared toolsmith
                // chest. Bed halves need no lift: vanilla's bed block entity
                // carries no payload (the colour lives in the block id, e.g.
                // `white_bed`), and `mc_world::wire::client_block_entities`
                // synthesizes the client-render record from the block state.
                if let Some(chest) = template_chest_from_nbt(pos, compound, registry, *state) {
                    chests.push(chest);
                }
            }
            TemplatePaletteEntry::Jigsaw(jigsaw_state) => {
                let jigsaw = parse_template_jigsaw(path, pos, *jigsaw_state, compound, registry)?;
                if is_plains_villager_jigsaw(compound) {
                    villager_markers.push(pos);
                }
                jigsaws.push(jigsaw);
            }
            TemplatePaletteEntry::Ignored => {}
        }
    }
    Ok(ParsedBlocks {
        blocks,
        villager_markers,
        jigsaws,
        chests,
    })
}

/// A template chest block's paste-time entity: the `LootTable` the vanilla
/// village templates author on their chests (`blocks[].nbt`), with empty fixed
/// contents because vanilla rolls the table at paste time instead of shipping
/// stacks. `LootTableSeed` is deliberately ignored — `place_piece` re-draws it
/// from the structure's placement random. Only chest-family containers lift;
/// the templates' other block entities are either payloadless (beds — their
/// client-render record is synthesized from the block state at wire time) or
/// unmodelled here.
fn template_chest_from_nbt(
    pos: [i32; 3],
    compound: &[(String, Tag)],
    registry: &BlockRegistry,
    state: BlockStateId,
) -> Option<TemplateChest> {
    if !registry
        .by_id(state)
        .is_some_and(|state| matches!(state.block.id.path(), "chest" | "trapped_chest"))
    {
        return None;
    }
    let nbt = compound
        .iter()
        .find(|(key, _)| key == "nbt")
        .and_then(|(_, tag)| match tag {
            Tag::Compound(fields) => Some(fields),
            _ => None,
        })?;
    let loot_table = nbt.iter().find_map(|(key, value)| {
        if key == "LootTable"
            && let Tag::String(text) = value
        {
            Identifier::parse(text.clone()).ok()
        } else {
            None
        }
    });
    Some(TemplateChest {
        pos,
        chest: ChestBlockEntity::default(),
        loot_table,
    })
}

/// Read one jigsaw block entity into a [`TemplateJigsaw`].
fn parse_template_jigsaw(
    path: &str,
    pos: [i32; 3],
    state: BlockStateId,
    compound: &[(String, Tag)],
    registry: &BlockRegistry,
) -> Result<TemplateJigsaw, StructureError> {
    let nbt = compound
        .iter()
        .find(|(key, _)| key == "nbt")
        .map(|(_, tag)| tag)
        .and_then(|tag| match tag {
            Tag::Compound(fields) => Some(fields),
            _ => None,
        })
        .ok_or_else(|| StructureError::InvalidField {
            path: path.to_string(),
            field: "blocks[].nbt",
        })?;
    let text = |field: &'static str| -> Result<String, StructureError> {
        nbt.iter()
            .find(|(key, _)| key == field)
            .and_then(|(_, tag)| match tag {
                Tag::String(value) => Some(value.clone()),
                _ => None,
            })
            .ok_or_else(|| StructureError::InvalidField {
                path: path.to_string(),
                field: "jigsaw field",
            })
    };
    let identifier = |field: &'static str| -> Result<Identifier, StructureError> {
        Identifier::parse(text(field)?).map_err(|_| StructureError::InvalidField {
            path: path.to_string(),
            field: "jigsaw identifier",
        })
    };
    // The jigsaw's own state carries `orientation`
    // (`JigsawBlock.getFrontFacing` = `state.getValue(ORIENTATION).front()`).
    let orientation = registry
        .by_id(state)
        .and_then(|state| {
            state
                .properties
                .iter()
                .find(|(key, _)| key == "orientation")
                .map(|(_, value)| value.clone())
        })
        .ok_or_else(|| StructureError::InvalidField {
            path: path.to_string(),
            field: "jigsaw orientation",
        })?;
    let (front, top) =
        orientation_faces(&orientation).ok_or_else(|| StructureError::InvalidField {
            path: path.to_string(),
            field: "jigsaw orientation",
        })?;
    let final_state = parse_state_string(path, &text("final_state")?, registry)?;
    let joint = match text("joint")?.as_str() {
        "rollable" => Joint::Rollable,
        _ => Joint::Aligned,
    };
    let selection_priority = nbt
        .iter()
        .find(|(key, _)| key == "selection_priority")
        .and_then(|(_, tag)| match tag {
            Tag::Int(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(0);
    let placement_priority = nbt
        .iter()
        .find(|(key, _)| key == "placement_priority")
        .and_then(|(_, tag)| match tag {
            Tag::Int(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(0);
    Ok(TemplateJigsaw {
        pos,
        state,
        front,
        top,
        name: identifier("name")?,
        target: identifier("target")?,
        pool: identifier("pool")?,
        joint,
        final_state,
        selection_priority,
        placement_priority,
    })
}

/// `minecraft:oak_planks` / `minecraft:oak_stairs[facing=east,half=bottom]`.
fn parse_state_string(
    path: &str,
    text: &str,
    registry: &BlockRegistry,
) -> Result<BlockStateId, StructureError> {
    let invalid = || StructureError::InvalidField {
        path: path.to_string(),
        field: "final_state",
    };
    let (name, properties) = match text.split_once('[') {
        Some((name, rest)) => {
            // Vanilla's block-state string reader stops at the closing bracket
            // and does not assert end-of-input, so real piece NBT carries values
            // such as `minecraft:acacia_fence[...west=false]]` (a trailing `]`
            // the game itself reads and ignores); everything after the closing
            // bracket is ignored here too.
            let closing = rest.find(']').ok_or_else(invalid)?;
            let inner = &rest[..closing];
            let mut pairs = Vec::new();
            if !inner.is_empty() {
                for pair in inner.split(',') {
                    let (key, value) = pair.split_once('=').ok_or_else(invalid)?;
                    pairs.push((key.to_owned(), value.to_owned()));
                }
            }
            (name, pairs)
        }
        None => (text, Vec::new()),
    };
    let id = Identifier::parse(name.to_owned()).map_err(|_| invalid())?;
    // `BlockStateParser` fills the properties the string omits from the block's
    // default state and rejects unknown ones, so `minecraft:sandstone_stairs
    // [facing=south]` and a bare `minecraft:grass_block` both resolve.
    let block = registry.block(&id).ok_or_else(invalid)?;
    let mut resolved: Vec<(String, String)> = registry
        .by_id(block.default)
        .ok_or_else(invalid)?
        .properties
        .clone();
    for (key, value) in properties {
        match resolved.iter_mut().find(|(name, _)| *name == key) {
            Some(slot) => slot.1 = value,
            None => return Err(invalid()),
        }
    }
    registry
        .by_name_and_props(&id, &resolved)
        .ok_or_else(invalid)
}

fn is_plains_villager_jigsaw(compound: &[(String, Tag)]) -> bool {
    compound
        .iter()
        .find(|(key, _)| key == "nbt")
        .and_then(|(_, tag)| match tag {
            Tag::Compound(fields) => Some(fields),
            _ => None,
        })
        .is_some_and(|fields| {
            fields.iter().any(|(key, value)| {
                key == "pool"
                    && matches!(
                        value,
                        Tag::String(pool) if pool == "minecraft:village/plains/villagers"
                    )
            })
        })
}

fn require<'a>(
    compound: &'a [(String, Tag)],
    path: &str,
    field: &'static str,
) -> Result<&'a Tag, StructureError> {
    compound
        .iter()
        .find(|(key, _)| key == field)
        .map(|(_, value)| value)
        .ok_or_else(|| StructureError::MissingField {
            path: path.to_string(),
            field,
        })
}

fn expect_compound<'a>(
    path: &str,
    tag: &'a Tag,
    field: &'static str,
) -> Result<&'a [(String, Tag)], StructureError> {
    match tag {
        Tag::Compound(entries) => Ok(entries),
        _ => Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        }),
    }
}

fn expect_list<'a>(
    path: &str,
    tag: &'a Tag,
    field: &'static str,
) -> Result<&'a ListTag, StructureError> {
    match tag {
        Tag::List(list) => Ok(list),
        _ => Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        }),
    }
}

fn expect_string<'a>(
    path: &str,
    tag: &'a Tag,
    field: &'static str,
) -> Result<&'a String, StructureError> {
    match tag {
        Tag::String(value) => Ok(value),
        _ => Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        }),
    }
}

fn expect_int(path: &str, tag: &Tag, field: &'static str) -> Result<i32, StructureError> {
    match tag {
        Tag::Int(value) => Ok(*value),
        _ => Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        }),
    }
}

fn expect_int_triplet(
    path: &str,
    tag: &Tag,
    field: &'static str,
) -> Result<[i32; 3], StructureError> {
    let list = expect_list(path, tag, field)?;
    if list.elements.len() != 3 {
        return Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        });
    }
    Ok([
        expect_int(path, &list.elements[0], field)?,
        expect_int(path, &list.elements[1], field)?,
        expect_int(path, &list.elements[2], field)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mc_data::blocks::{BlockReport, BlockStateReport};
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    fn gzip_bytes(payload: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).unwrap();
        encoder.finish().unwrap()
    }

    fn named_nbt_bytes(root: &Tag) -> Vec<u8> {
        let mut bytes = Vec::new();
        mc_nbt::write_named(&mut bytes, "", root).unwrap();
        bytes
    }

    fn deterministic_noise(len: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_u32;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect()
    }

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    #[test]
    fn structure_file_and_gzip_limits_fail_before_unbounded_growth() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bounded.nbt");
        std::fs::write(&path, vec![0; 9]).unwrap();
        assert!(matches!(
            read_bounded_structure_file(&path, 8),
            Err(StructureError::DataTooLarge {
                kind: "compressed or raw NBT",
                bytes: 9,
                max: 8,
                ..
            })
        ));

        let exact = vec![0xA5; 8];
        assert_eq!(
            decode_single_structure_gzip_member("test.nbt", &gzip_bytes(&exact), 8).unwrap(),
            exact
        );
        assert!(matches!(
            decode_single_structure_gzip_member("test.nbt", &gzip_bytes(&[0xA5; 9]), 8),
            Err(StructureError::DataTooLarge {
                kind: "decompressed NBT",
                bytes: 9,
                max: 8,
                ..
            })
        ));
    }

    #[test]
    fn structure_loader_rejects_second_gzip_member_and_truncated_stream() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("members.nbt");
        let payload = named_nbt_bytes(&Tag::Compound(vec![(
            "blob".into(),
            Tag::ByteArray(
                deterministic_noise(128 * 1024)
                    .into_iter()
                    .map(|byte| byte as i8)
                    .collect(),
            ),
        )]));
        let mut concatenated = gzip_bytes(&payload);
        concatenated.extend_from_slice(&gzip_bytes(&payload));
        std::fs::write(&path, concatenated).unwrap();
        let registry = BlockRegistry::from_report(&[]).unwrap();
        assert!(matches!(
            StructureTemplate::from_nbt_file(&path, &registry),
            Err(StructureError::TrailingData {
                kind: "compressed gzip member",
                bytes,
                ..
            }) if bytes > 0
        ));

        let mut truncated = gzip_bytes(&payload);
        truncated.truncate(truncated.len() - 4);
        std::fs::write(&path, truncated).unwrap();
        assert!(matches!(
            StructureTemplate::from_nbt_file(&path, &registry),
            Err(StructureError::Io { .. })
        ));
    }

    #[test]
    fn structure_loader_rejects_decompressed_trailing_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trailing.nbt");
        let mut payload = named_nbt_bytes(&Tag::Compound(Vec::new()));
        payload.push(0xA5);
        std::fs::write(&path, gzip_bytes(&payload)).unwrap();
        let registry = BlockRegistry::from_report(&[]).unwrap();

        assert!(matches!(
            StructureTemplate::from_nbt_file(&path, &registry),
            Err(StructureError::TrailingData {
                kind: "decompressed NBT",
                bytes: 1,
                ..
            })
        ));
    }

    #[test]
    fn template_chest_nbt_lifts_loot_table_and_skips_undeclared_blocks() {
        let registry = BlockRegistry::from_report(&[
            BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:chest").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 2,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
        ])
        .unwrap();
        let root = Tag::Compound(vec![
            ("size".into(), Tag::List(int_triplet([1, 2, 3]))),
            (
                "palette".into(),
                Tag::List(ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![
                        Tag::Compound(vec![("Name".into(), Tag::String("minecraft:air".into()))]),
                        Tag::Compound(vec![("Name".into(), Tag::String("minecraft:chest".into()))]),
                        Tag::Compound(vec![("Name".into(), Tag::String("minecraft:stone".into()))]),
                    ],
                }),
            ),
            (
                "blocks".into(),
                Tag::List(ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![
                        Tag::Compound(vec![
                            ("pos".into(), Tag::List(int_triplet([0, 0, 0]))),
                            ("state".into(), Tag::Int(0)),
                        ]),
                        // The village small-house chest: block state plus a
                        // loot-table payload the loader must keep.
                        Tag::Compound(vec![
                            ("pos".into(), Tag::List(int_triplet([0, 0, 1]))),
                            ("state".into(), Tag::Int(1)),
                            (
                                "nbt".into(),
                                Tag::Compound(vec![
                                    ("id".into(), Tag::String("minecraft:chest".into())),
                                    (
                                        "LootTable".into(),
                                        Tag::String(
                                            "minecraft:chests/village/village_plains_house".into(),
                                        ),
                                    ),
                                ]),
                            ),
                        ]),
                        // A chest block with no block-entity payload stays
                        // BE-less: only template-declared containers lift.
                        Tag::Compound(vec![
                            ("pos".into(), Tag::List(int_triplet([0, 0, 2]))),
                            ("state".into(), Tag::Int(1)),
                        ]),
                        // A non-chest block's payload is not a chest.
                        Tag::Compound(vec![
                            ("pos".into(), Tag::List(int_triplet([0, 1, 0]))),
                            ("state".into(), Tag::Int(2)),
                            (
                                "nbt".into(),
                                Tag::Compound(vec![(
                                    "id".into(),
                                    Tag::String("minecraft:bed".into()),
                                )]),
                            ),
                        ]),
                    ],
                }),
            ),
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("village_chest.nbt");
        std::fs::write(&path, named_nbt_bytes(&root)).unwrap();

        let template = StructureTemplate::from_nbt_file(&path, &registry).unwrap();

        assert_eq!(
            template.chests(),
            &[TemplateChest {
                pos: [0, 0, 1],
                chest: ChestBlockEntity::default(),
                loot_table: Some(
                    Identifier::parse("minecraft:chests/village/village_plains_house").unwrap()
                ),
            }],
            "exactly the template-declared chest lifts, with its loot table"
        );
    }

    fn int_triplet(values: [i32; 3]) -> ListTag {
        ListTag {
            element_type: mc_nbt::tag_type::INT,
            elements: values.into_iter().map(Tag::Int).collect(),
        }
    }

    #[test]
    #[ignore = "requires local 26.1.2 blocks report and plains fountain structure"]
    fn loads_real_plains_fountain_template_when_present() {
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        let template_path = workspace_path(
            "data/vanilla/data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt",
        );
        assert!(
            blocks_path.is_file(),
            "requires local 26.1.2 blocks report at {}",
            blocks_path.display()
        );
        assert!(
            template_path.is_file(),
            "requires local 26.1.2 plains fountain structure at {}",
            template_path.display()
        );
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = BlockRegistry::from_report(&report).unwrap();
        let template = StructureTemplate::from_nbt_file(&template_path, &registry).unwrap();

        assert_eq!(template.size(), [9, 4, 9]);
        assert!(template.blocks().len() > 100);
        assert!(
            template
                .blocks()
                .iter()
                .all(|block| registry.by_id(block.state).is_some())
        );
    }
}
