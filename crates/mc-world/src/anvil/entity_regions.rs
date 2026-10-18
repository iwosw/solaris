//! Vanilla entity-region container (`entities/r.x.z.mca`): one chunk
//! compound per 32×32 region slot with `DataVersion`, `Position` and the
//! `Entities` list (records via [`mc_data::entity_nbt`]). Written through the
//! existing mc-world region file writer (atomic); Solaris-only bookkeeping
//! stays in the manifest, never in these files.

use std::collections::BTreeMap;
use std::path::Path;

use mc_data::entity_nbt::{VanillaEntity, entity_from_fields, entity_to_fields};
use mc_data::items::ItemRegistry;
use mc_nbt::{ListTag, Tag, tag_type};

use super::region::{ChunkPayload, read_region, write_region};

/// Vanilla data version stamped on entity chunks: the 26.1.2 world version
/// (`data/vanilla/version.json`), shared with the block-chunk writer so one
/// constant drives every `DataVersion` we emit.
pub use super::chunk_nbt::TARGET_DATA_VERSION as ENTITY_DATA_VERSION;

/// One region slot's entity chunk: position + records.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntityChunk {
    pub x: i32,
    pub z: i32,
    pub entities: Vec<VanillaEntity>,
}

/// Codec failure (I/O or invalid entity NBT).
#[derive(Debug)]
pub enum EntityRegionError {
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    Region(super::region::RegionError),
    Chunk(mc_nbt::NbtError),
    Item(mc_data::item_nbt::ItemNbtError),
    Entity {
        path: std::path::PathBuf,
        chunk: [i32; 2],
        entity_index: usize,
        uuid: String,
        id: String,
        type_id: Option<u32>,
        reason: String,
    },
}

impl std::fmt::Display for EntityRegionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "entity region I/O at {}: {source}", path.display())
            }
            Self::Region(source) => write!(f, "entity region: {source}"),
            Self::Chunk(source) => write!(f, "entity region NBT: {source}"),
            Self::Item(source) => write!(f, "entity region item: {source}"),
            Self::Entity {
                path,
                chunk,
                entity_index,
                uuid,
                id,
                type_id,
                reason,
            } => {
                write!(
                    f,
                    "{}: chunk ({}, {}) entity-index {entity_index} UUID {uuid} type-id ",
                    path.display(),
                    chunk[0],
                    chunk[1]
                )?;
                if let Some(type_id) = type_id {
                    write!(f, "{type_id}")?;
                } else {
                    write!(f, "unknown")?;
                }
                write!(f, " ({id}): {reason}")
            }
        }
    }
}

impl std::error::Error for EntityRegionError {}

/// Region file name for an entity region (same layout as block regions).
#[must_use]
pub fn entity_region_file_name(rx: i32, rz: i32) -> String {
    format!("r.{rx}.{rz}.mca")
}

fn chunk_to_tag(items: &ItemRegistry, chunk: &EntityChunk) -> Result<Tag, EntityRegionError> {
    let mut elements = Vec::new();
    for entity in &chunk.entities {
        let fields = entity_to_fields(items, entity).map_err(EntityRegionError::Item)?;
        elements.push(Tag::Compound(fields));
    }
    Ok(Tag::Compound(vec![
        ("DataVersion".into(), Tag::Int(ENTITY_DATA_VERSION)),
        // Vanilla `EntityStorage` writes `Position` through `ChunkPos`'s
        // `Codec.INT_STREAM`, which NBT encodes as a two-element IntArray
        // (26.1.2 `dvu.java:11-12`, `vn.java:399-400`).
        ("Position".into(), Tag::IntArray(vec![chunk.x, chunk.z])),
        (
            "Entities".into(),
            Tag::List(ListTag {
                element_type: if elements.is_empty() {
                    tag_type::END
                } else {
                    tag_type::COMPOUND
                },
                elements,
            }),
        ),
    ]))
}

/// Decode a complete entity-region record after any required offline upgrade.
/// The source path is retained in shared-field and per-entity error context.
pub fn entity_chunk_from_nbt(
    items: &ItemRegistry,
    tag: &Tag,
    path: &Path,
) -> Result<EntityChunk, EntityRegionError> {
    let mc_nbt::Tag::Compound(root) = tag else {
        return Err(EntityRegionError::Item(
            mc_data::item_nbt::ItemNbtError::InvalidComponent("entity chunk root".to_owned()),
        ));
    };
    let find = |name: &str| root.iter().find(|(key, _)| key == name).map(|(_, tag)| tag);
    // Only format: vanilla two-element IntArray (`Codec.INT_STREAM`).
    let (x, z) = match find("Position") {
        Some(Tag::IntArray(values)) if values.len() == 2 => (values[0], values[1]),
        _ => {
            return Err(EntityRegionError::Item(
                mc_data::item_nbt::ItemNbtError::InvalidComponent("Position".to_owned()),
            ));
        }
    };
    let mut entities = Vec::new();
    if let Some(tag) = find("Entities") {
        let Tag::List(list) = tag else {
            return Err(record_error(
                path,
                [x, z],
                0,
                &[],
                "Entities: expected list".into(),
            ));
        };
        for (entity_index, element) in list.elements.iter().enumerate() {
            let Tag::Compound(fields) = element else {
                return Err(record_error(
                    path,
                    [x, z],
                    entity_index,
                    &[],
                    "Entities: expected entity compound".into(),
                ));
            };
            let invalid = |reason| record_error(path, [x, z], entity_index, fields, reason);
            validate_shared_fields(fields).map_err(&invalid)?;
            let entity =
                entity_from_fields(items, VanillaEntity::default(), fields).map_err(|err| {
                    use mc_data::item_nbt::ItemNbtError;
                    let reason = match &err {
                        ItemNbtError::InvalidId(_) | ItemNbtError::UnknownItem(_) => {
                            format!("Item.id: {err}")
                        }
                        ItemNbtError::InvalidEnchantment(_) => {
                            format!("Item.components.minecraft:enchantments: {err}")
                        }
                        ItemNbtError::InvalidComponent(_) => format!("Item.components: {err}"),
                    };
                    invalid(reason)
                })?;
            entities.push(entity);
        }
    }
    Ok(EntityChunk { x, z, entities })
}

/// Capture identity from the raw compound before the shared decoder consumes it.
/// Strings and the path are allocated only on refusal.
pub fn record_error(
    path: &Path,
    chunk: [i32; 2],
    entity_index: usize,
    fields: &[(String, Tag)],
    reason: String,
) -> EntityRegionError {
    let find = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    };
    let id = match find("id") {
        Some(Tag::String(id)) => id.clone(),
        Some(tag) => format!("{tag:?}"),
        None => "<missing>".into(),
    };
    let type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name(&id)
        .map(|contract| contract.protocol_id);
    let uuid = match find("UUID") {
        Some(Tag::IntArray(words)) if words.len() == 4 => {
            let bits = words
                .iter()
                .fold(0_u128, |bits, word| (bits << 32) | u128::from(*word as u32));
            uuid::Uuid::from_u128(bits).to_string()
        }
        Some(tag) => format!("{tag:?}"),
        None => "<missing>".into(),
    };
    EntityRegionError::Entity {
        path: path.to_owned(),
        chunk,
        entity_index,
        uuid,
        id,
        type_id,
        reason,
    }
}

/// Present shared keys cannot silently default before species validation.
/// Missing keys keep the shared codec's vanilla import defaults.
pub fn validate_shared_fields(fields: &[(String, Tag)]) -> Result<(), String> {
    for (index, (name, tag)) in fields.iter().enumerate() {
        if fields[..index].iter().any(|(other, _)| other == name) {
            return Err(format!("{name}: duplicate field"));
        }
        match name.as_str() {
            "id" if !matches!(tag, Tag::String(_)) => return Err("id: expected string".into()),
            "UUID" if !matches!(tag, Tag::IntArray(words) if words.len() == 4) => {
                return Err("UUID: expected four-int UUID".into());
            }
            "Pos" | "Motion" => {
                let Tag::List(list) = tag else {
                    return Err(format!("{name}: expected three-double list"));
                };
                if list.elements.len() != 3 {
                    return Err(format!("{name}: expected three-double list"));
                }
                for (index, value) in list.elements.iter().enumerate() {
                    if !matches!(value, Tag::Double(value) if value.is_finite()) {
                        return Err(format!("{name}[{index}]: expected finite double"));
                    }
                }
            }
            "Rotation" => {
                let Tag::List(list) = tag else {
                    return Err("Rotation: expected two-float list".into());
                };
                if list.elements.len() != 2 {
                    return Err("Rotation: expected two-float list".into());
                }
                for (index, value) in list.elements.iter().enumerate() {
                    if !matches!(value, Tag::Float(value) if value.is_finite()) {
                        return Err(format!("Rotation[{index}]: expected finite float"));
                    }
                }
            }
            "Health" => match tag {
                Tag::Float(value) if value.is_finite() => {}
                Tag::Double(value) if (*value as f32).is_finite() => {}
                Tag::Byte(_) | Tag::Short(_) | Tag::Int(_) | Tag::Long(_) => {}
                _ => return Err("Health: expected finite numeric health".into()),
            },
            "solaris:WireId" if !matches!(tag, Tag::Int(value) if *value >= 0) => {
                return Err("solaris:WireId: expected nonnegative int".into());
            }
            "Item" => {
                let Tag::Compound(item) = tag else {
                    return Err("Item: expected item compound".into());
                };
                if !matches!(mc_data::item_nbt::field(item, "id"), Some(Tag::String(_))) {
                    return Err("Item.id: missing or not a string".into());
                }
                for (key, value) in item {
                    if key == "count"
                        && !matches!(value, Tag::Byte(_) | Tag::Short(_) | Tag::Int(_))
                    {
                        return Err("Item.count: expected byte, short or int".into());
                    }
                }
            }
            _ => {}
        }
    }
    for required in ["id", "UUID", "Pos"] {
        if !fields.iter().any(|(name, _)| name == required) {
            return Err(format!("{required}: missing required saved-entity field"));
        }
    }
    Ok(())
}

/// Write entity-region files under `entities_dir` (atomic via the region
/// writer's tmp+rename): groups chunks by region, one file per region.
/// A complete checkpoint writes the full entity map: regions with no live
/// chunk are deleted (atomic tmp+rename or remove + fsync) BEFORE the
/// caller publishes the manifest, so an emptied region can never resurrect
/// its old entities after the manifest advances.
pub fn write_entity_regions(
    entities_dir: &Path,
    items: &ItemRegistry,
    chunks: &BTreeMap<(i32, i32), EntityChunk>,
    timestamp: u32,
) -> Result<(), EntityRegionError> {
    std::fs::create_dir_all(entities_dir).map_err(|err| EntityRegionError::Io {
        path: entities_dir.to_path_buf(),
        source: err,
    })?;
    let mut by_region: BTreeMap<(i32, i32), Vec<&EntityChunk>> = BTreeMap::new();
    for chunk in chunks.values() {
        by_region
            .entry((chunk.x.div_euclid(32), chunk.z.div_euclid(32)))
            .or_default()
            .push(chunk);
    }
    let mut wanted: std::collections::BTreeSet<(i32, i32)> = std::collections::BTreeSet::new();
    for ((rx, rz), group) in by_region {
        wanted.insert((rx, rz));
        let mut payloads = Vec::new();
        for chunk in group {
            let tag = chunk_to_tag(items, chunk)?;
            let mut raw = Vec::new();
            mc_nbt::write_named(&mut raw, "", &tag).map_err(EntityRegionError::Chunk)?;
            payloads.push(ChunkPayload {
                local_x: (chunk.x.rem_euclid(32)) as u8,
                local_z: (chunk.z.rem_euclid(32)) as u8,
                timestamp,
                uncompressed_nbt: raw,
            });
        }
        // Atomic: tmp file -> fsync -> rename -> fsync file + dir.
        let tmp = entities_dir.join(format!("{}.tmp", entity_region_file_name(rx, rz)));
        write_region(&tmp, &payloads).map_err(EntityRegionError::Region)?;
        fsync_file(&tmp).map_err(|err| EntityRegionError::Io {
            path: tmp.clone(),
            source: err,
        })?;
        let final_path = entities_dir.join(entity_region_file_name(rx, rz));
        std::fs::rename(&tmp, &final_path).map_err(|err| EntityRegionError::Io {
            path: final_path.clone(),
            source: err,
        })?;
        fsync_file(&final_path).map_err(|err| EntityRegionError::Io {
            path: final_path.clone(),
            source: err,
        })?;
        fsync_dir(entities_dir).map_err(|err| EntityRegionError::Io {
            path: entities_dir.to_path_buf(),
            source: err,
        })?;
    }
    // Stale regions: files on disk with no live chunk. Remove them now
    // (fsync the dir after each removal) so the manifest that follows
    // this call never points at resurrected entities.
    let mut stale = Vec::new();
    if let Ok(entries) = std::fs::read_dir(entities_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("r.") && name.ends_with(".mca") && !name.ends_with(".tmp")) {
                continue;
            }
            let Some(region) = parse_region_name(&name) else {
                continue;
            };
            if !wanted.contains(&region) {
                stale.push(entry.path());
            }
        }
    }
    stale.sort();
    for path in stale {
        std::fs::remove_file(&path).map_err(|err| EntityRegionError::Io {
            path: path.clone(),
            source: err,
        })?;
        fsync_dir(entities_dir).map_err(|err| EntityRegionError::Io {
            path: entities_dir.to_path_buf(),
            source: err,
        })?;
    }
    Ok(())
}

/// Parse `r.<rx>.<rz>.mca` into its region coordinates.
fn parse_region_name(name: &str) -> Option<(i32, i32)> {
    let rest = name.strip_prefix("r.")?.strip_suffix(".mca")?;
    let (rx, rz) = rest.split_once('.')?;
    Some((rx.parse().ok()?, rz.parse().ok()?))
}

/// Read every entity-region file under `entities_dir`.
pub fn read_entity_regions(
    entities_dir: &Path,
    items: &ItemRegistry,
) -> Result<BTreeMap<(i32, i32), EntityChunk>, EntityRegionError> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(entities_dir) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("r.") && name.ends_with(".mca") && !name.ends_with(".tmp")) {
            continue;
        }
        let path = entry.path();
        let payloads = read_region(&path).map_err(EntityRegionError::Region)?;
        for payload in payloads {
            let mut cursor = std::io::Cursor::new(&payload.uncompressed_nbt);
            let (_, tag) = mc_nbt::read_named(&mut cursor).map_err(EntityRegionError::Chunk)?;
            let chunk = entity_chunk_from_nbt(items, &tag, &path)?;
            out.insert((chunk.x, chunk.z), chunk);
        }
    }
    Ok(out)
}

fn fsync_file(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

fn fsync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> ItemRegistry {
        mc_data::items::solaris_required_items()
    }

    #[test]
    fn entity_regions_round_trip() {
        let items = items();
        let dir = std::env::temp_dir().join(format!("world-entity-regions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut chunks = BTreeMap::new();
        chunks.insert(
            (0, 0),
            EntityChunk {
                x: 0,
                z: 0,
                entities: vec![VanillaEntity {
                    id: "minecraft:cow".to_owned(),
                    uuid: uuid::Uuid::from_u128(7),
                    pos: [8.0, 80.0, 8.0],
                    motion: [0.0; 3],
                    yaw: 0.0,
                    pitch: 0.0,
                    on_ground: true,
                    health: 10.0,
                    age: 0,
                    breed_cooldown: 6000,
                    pickup_delay: 0,
                    item: None,
                    experience_orb: None,
                    sheep_wool: None,
                    active_effects: Vec::new(),
                    absorption_amount: 0.0,
                    wire_id: 3,
                    extra_fields: Vec::new(),
                }],
            },
        );
        write_entity_regions(&dir, &items, &chunks, 1234).expect("write");
        // Atomic write leaves no tmp files behind.
        assert!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
        let back = read_entity_regions(&dir, &items).expect("read");
        assert_eq!(back, chunks);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn cow(wire_id: i32) -> VanillaEntity {
        VanillaEntity {
            id: "minecraft:cow".to_owned(),
            uuid: uuid::Uuid::from_u128(7),
            pos: [8.0, 80.0, 8.0],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            health: 10.0,
            age: 0,
            breed_cooldown: 6000,
            pickup_delay: 0,
            item: None,
            experience_orb: None,
            sheep_wool: None,
            active_effects: Vec::new(),
            absorption_amount: 0.0,
            wire_id,
            extra_fields: Vec::new(),
        }
    }

    /// `Position` writes as a vanilla two-element IntArray
    /// (`Codec.INT_STREAM`), with the shared 26.1.2 `DataVersion`.
    #[test]
    fn position_writes_vanilla_int_array_with_shared_data_version() {
        let items = items();
        let chunk = EntityChunk {
            x: 3,
            z: -5,
            entities: vec![cow(3)],
        };
        let tag = chunk_to_tag(&items, &chunk).expect("encode");
        let mc_nbt::Tag::Compound(root) = &tag else {
            panic!("root");
        };
        let find = |name: &str| root.iter().find(|(key, _)| key == name).map(|(_, tag)| tag);
        assert_eq!(
            find("Position"),
            Some(&mc_nbt::Tag::IntArray(vec![3, -5])),
            "vanilla IntArray Position"
        );
        assert_eq!(
            find("DataVersion"),
            Some(&mc_nbt::Tag::Int(
                super::super::chunk_nbt::TARGET_DATA_VERSION
            )),
            "shared 26.1.2 DataVersion"
        );
        assert_eq!(
            super::super::chunk_nbt::TARGET_DATA_VERSION,
            4790,
            "26.1.2 world_version"
        );
        // And it reads back.
        let back = entity_chunk_from_nbt(&items, &tag, Path::new("r.0.0.mca")).expect("decode");
        assert_eq!(back, chunk);
    }

    /// A hand-built vanilla region (IntArray `Position`) reads.
    #[test]
    fn vanilla_int_array_region_reads() {
        let items = items();
        let dir = std::env::temp_dir().join(format!("world-entity-vanilla-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut chunks = BTreeMap::new();
        chunks.insert(
            (3, -5),
            EntityChunk {
                x: 3,
                z: -5,
                entities: vec![cow(9)],
            },
        );
        write_entity_regions(&dir, &items, &chunks, 7).expect("write");
        let back = read_entity_regions(&dir, &items).expect("read");
        assert_eq!(back, chunks);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Emptying a region removes its file: no resurrection after the next
    /// manifest. Pre-fix this file survived and its entities came back.
    #[test]
    fn obsolete_region_files_are_removed() {
        let items = items();
        let dir = std::env::temp_dir().join(format!("world-entity-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut chunks = BTreeMap::new();
        chunks.insert(
            (0, 0),
            EntityChunk {
                x: 0,
                z: 0,
                entities: vec![cow(1)],
            },
        );
        // Second region far away (different region file).
        chunks.insert(
            (64, 64),
            EntityChunk {
                x: 64,
                z: 64,
                entities: vec![cow(2)],
            },
        );
        write_entity_regions(&dir, &items, &chunks, 1).expect("write both");
        assert!(dir.join("r.0.0.mca").is_file());
        assert!(dir.join("r.2.2.mca").is_file());
        // All entities leave region (2,2): complete checkpoint rewrites.
        let mut remaining = BTreeMap::new();
        remaining.insert(
            (0, 0),
            EntityChunk {
                x: 0,
                z: 0,
                entities: vec![cow(1)],
            },
        );
        write_entity_regions(&dir, &items, &remaining, 2).expect("rewrite");
        assert!(
            !dir.join("r.2.2.mca").exists(),
            "stale region removed before manifest"
        );
        let back = read_entity_regions(&dir, &items).expect("read");
        assert_eq!(back, remaining, "no resurrected entities");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
#[path = "entity_regions_validation_tests.rs"]
mod validation_tests;
