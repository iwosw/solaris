//! Offline Java 1.18+ Anvil admission into an unpublished native database.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_nbt::Tag;
use mc_world::anvil::upgrade::{self, PreservationReceipt, RecordKind, UpgradeReport};
use mc_world::native::{NativeCheckpoint, NativePlayerSnapshot, NativeStore};
use mc_world::{Chunk, ChunkPos};

use super::{
    ConversionStats, RecordUpgradeReceipt, check_version, fail, files, player_source_version,
    read_nbt, saved_data_source_version, source_version,
};
use crate::config::{CoreConfig, CoreError};
use crate::dimensions::Dimension;
use crate::journal::{CheckpointManifest, Replay};
use crate::native_checkpoint as bridge;

/// Read one named child of a compound Tag: missing → None, duplicate →
/// fail-closed. Takes `&Tag` (unlike `bees::field`, which takes field
/// slices) so compound-valued callers pass tags directly.
fn field<'a>(tag: &'a Tag, key: &str) -> Result<Option<&'a Tag>, CoreError> {
    let Tag::Compound(fields) = tag else {
        return Err(fail(format!("expected compound for field {key}")));
    };
    let mut values = fields.iter().filter(|(name, _)| name == key);
    let value = values.next().map(|(_, value)| value);
    if values.next().is_some() {
        return Err(fail(format!("duplicate field {key}")));
    }
    Ok(value)
}

const DIMENSIONS: [Dimension; 3] = [Dimension::Overworld, Dimension::Nether, Dimension::End];
// Native global envelopes intentionally have a per-record, not per-world, budget.
const EXTRAS_BUDGET: u64 = 16 * 1024 * 1024;
const METADATA: [&str; 6] = [
    "level.dat",
    "data/minecraft/game_rules.dat",
    "data/minecraft/world_clocks.dat",
    "data/minecraft/weather.dat",
    "data/minecraft/world_gen_settings.dat",
    "data/minecraft/wandering_trader.dat",
];

/// Port of vanilla 26.1.2 `RaidRenamesDataFix` (registered at DataVersion
/// 4309 for `SAVED_DATA_RAIDS`; javap in
/// `.analysis/codex-logs/constantinople/aux-2586-raid-javap.txt`).
/// Renames 1.16.5 PascalCase keys to the snake_case codec fields and folds
/// inline `CX/CY/CZ` into `center`. Missing keys are left alone (DFU
/// `renameField` is a no-op when absent); a non-empty `Raids` list whose
/// entries lack every known shape fails closed downstream in `raids::from_nbt`.
#[cfg(test)]
pub(super) fn upgrade_raids_payload_for_tests(
    data: &mut Vec<(String, Tag)>,
    version: i32,
    steps: &mut Vec<super::UpgradeFixReceipt>,
) {
    upgrade_raids_payload(data, version, steps);
}

fn upgrade_raids_payload(
    data: &mut Vec<(String, Tag)>,
    version: i32,
    steps: &mut Vec<super::UpgradeFixReceipt>,
) {
    if version >= 4309 {
        return;
    }
    let mut renamed = 0_u32;
    let rename = |fields: &mut Vec<(String, Tag)>, old: &str, new: &str, counter: &mut u32| {
        if let Some(index) = fields.iter().position(|(name, _)| name == old)
            && !fields.iter().any(|(name, _)| name == new)
        {
            fields[index].0 = new.into();
            *counter += 1;
        }
    };
    rename(data, "Raids", "raids", &mut renamed);
    rename(data, "Tick", "tick", &mut renamed);
    rename(data, "NextAvailableID", "next_id", &mut renamed);
    if let Some((_, Tag::List(list))) = data.iter_mut().find(|(name, _)| name == "raids") {
        for entry in &mut list.elements {
            let Tag::Compound(fields) = entry else {
                continue;
            };
            // Vanilla `fixInlineBlockPos(CX, CY, CZ -> center)`: only when all
            // three parts read as numbers; otherwise the entry stays for the
            // typed reader to reject.
            let center = ["CX", "CY", "CZ"].iter().all(|key| {
                fields.iter().any(|(name, value)| {
                    name == key
                        && matches!(
                            value,
                            Tag::Byte(_)
                                | Tag::Short(_)
                                | Tag::Int(_)
                                | Tag::Long(_)
                                | Tag::Float(_)
                                | Tag::Double(_)
                        )
                })
            });
            if center && !fields.iter().any(|(name, _)| name == "center") {
                let mut coords = Vec::new();
                for key in ["CX", "CY", "CZ"] {
                    let index = fields
                        .iter()
                        .position(|(name, _)| name == key)
                        .expect("checked");
                    let (_, value) = fields.remove(index);
                    coords.push(match value {
                        Tag::Byte(v) => i32::from(v),
                        Tag::Short(v) => i32::from(v),
                        Tag::Int(v) => v,
                        Tag::Long(v) => v as i32,
                        Tag::Float(v) => v as i32,
                        Tag::Double(v) => v as i32,
                        _ => 0,
                    });
                }
                fields.push(("center".into(), Tag::IntArray(coords)));
                renamed += 1;
            }
            for (old, new) in [
                ("Id", "id"),
                ("Started", "started"),
                ("Active", "active"),
                ("TicksActive", "ticks_active"),
                ("BadOmenLevel", "raid_omen_level"),
                ("GroupsSpawned", "groups_spawned"),
                ("PreRaidTicks", "cooldown_ticks"),
                ("PostRaidTicks", "post_raid_ticks"),
                ("TotalHealth", "total_health"),
                ("NumGroups", "group_count"),
                ("Status", "status"),
                ("HeroesOfTheVillage", "heroes_of_the_village"),
            ] {
                rename(fields, old, new, &mut renamed);
            }
        }
    }
    if renamed > 0 {
        steps.push(super::UpgradeFixReceipt {
            version: 4309,
            fix: "RaidRenamesDataFix".into(),
        });
    }
}

fn exact_nbt(bytes: &[u8], path: &Path) -> Result<Tag, CoreError> {
    let mut input = bytes;
    let (_, tag) = mc_nbt::read_named(&mut input)
        .map_err(|error| fail(format!("{}: {error}", path.display())))?;
    if !input.is_empty() {
        return Err(fail(format!("{}: trailing NBT bytes", path.display())));
    }
    Ok(tag)
}

fn region_position(path: &Path) -> Result<(i32, i32), CoreError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| fail("invalid region filename"))?;
    let parts: Vec<_> = name.split('.').collect();
    if parts.len() != 4 || parts[0] != "r" || parts[3] != "mca" {
        return Err(fail(format!(
            "{}: unsupported region filename",
            path.display()
        )));
    }
    let x: i32 = parts[1].parse().map_err(fail)?;
    let z: i32 = parts[2].parse().map_err(fail)?;
    if name != format!("r.{x}.{z}.mca") {
        return Err(fail("noncanonical region filename"));
    }
    Ok((x, z))
}

fn validate_poi_path(path: &Path) -> Result<(), CoreError> {
    if !matches!(
        path.parent().and_then(Path::to_str),
        Some(
            "dimensions/minecraft/overworld/poi"
                | "dimensions/minecraft/the_nether/poi"
                | "dimensions/minecraft/the_end/poi"
        )
    ) {
        return Err(fail(format!("{}: unsupported POI path", path.display())));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| fail("invalid POI filename"))?;
    let mut parts = name.split('.');
    let prefix = parts.next().unwrap_or_default();
    let x: i32 = parts
        .next()
        .ok_or_else(|| fail("missing POI X"))?
        .parse()
        .map_err(fail)?;
    let z: i32 = parts
        .next()
        .ok_or_else(|| fail("missing POI Z"))?
        .parse()
        .map_err(fail)?;
    let extension = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || !matches!((prefix, extension), ("r", "mca") | ("c", "mcc"))
        || name != format!("{prefix}.{x}.{z}.{extension}")
    {
        return Err(fail(format!(
            "{}: unsupported POI filename",
            path.display()
        )));
    }
    Ok(())
}

fn visit(
    path: &Path,
    mut visitor: impl FnMut(ChunkPos, Tag, &[u8]) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let (rx, rz) = region_position(path)?;
    let mut result = Ok(());
    mc_world::anvil::visit_region(path, |payload| {
        if result.is_err() {
            return;
        }
        result = (|| {
            let pos = ChunkPos {
                x: rx
                    .checked_mul(32)
                    .and_then(|x| x.checked_add(i32::from(payload.local_x)))
                    .ok_or_else(|| fail("region X overflow"))?,
                z: rz
                    .checked_mul(32)
                    .and_then(|z| z.checked_add(i32::from(payload.local_z)))
                    .ok_or_else(|| fail("region Z overflow"))?,
            };
            let tag = exact_nbt(&payload.uncompressed_nbt, path)?;
            visitor(pos, tag, &payload.uncompressed_nbt)
        })();
    })
    .map_err(fail)?;
    result
}

// Named NBT requires a compound root. Move the body into that wrapper and back
// rather than cloning entity payloads solely for the cross-storage comparison.
fn entity_body_bytes(list: &mut Tag) -> Result<Vec<u8>, CoreError> {
    let wrapper = Tag::Compound(vec![(
        "Entities".into(),
        std::mem::replace(list, Tag::Compound(Vec::new())),
    )]);
    let mut bytes = Vec::new();
    let result = mc_nbt::write_named(&mut bytes, "", &wrapper);
    let Tag::Compound(mut fields) = wrapper else {
        unreachable!("comparison wrapper is a compound")
    };
    *list = fields.pop().expect("single comparison body").1;
    result.map_err(fail)?;
    Ok(bytes)
}

// Decode only identity here: discarded later bodies may contain unsupported
// payloads. The normal typed entity decoder validates every retained body.
fn admit_entity_uuids(
    list: &mut Tag,
    source: &Path,
    chunk: [i32; 2],
    uuids: &mut BTreeSet<uuid::Uuid>,
    drops: &mut Vec<super::DuplicateUuidDrop>,
) -> Result<(), CoreError> {
    let Tag::List(list) = list else {
        return Err(fail(format!("{}: invalid Entities list", source.display())));
    };
    let mut retained = 0;
    for entity_index in 0..list.elements.len() {
        let invalid = |reason| {
            if let Tag::Compound(fields) = &list.elements[entity_index] {
                return fail(mc_world::anvil::entity_regions::record_error(
                    source,
                    chunk,
                    entity_index,
                    fields,
                    reason,
                ));
            }
            fail(format!(
                "{} chunk ({}, {}) entity-index {entity_index}: {reason}",
                source.display(),
                chunk[0],
                chunk[1]
            ))
        };
        let uuid = match field(&list.elements[entity_index], "UUID")
            .map_err(|error| invalid(error.to_string()))?
        {
            Some(Tag::IntArray(words)) if words.len() == 4 => {
                let bits = words
                    .iter()
                    .fold(0_u128, |bits, word| (bits << 32) | u128::from(*word as u32));
                uuid::Uuid::from_u128(bits)
            }
            _ => return Err(invalid("UUID: expected four-int UUID".into())),
        };
        if uuid.is_nil() {
            return Err(invalid("nil entity UUID".into()));
        }
        if uuids.insert(uuid) {
            // Swapping into earlier dropped slots leaves later original indices
            // untouched and retains admitted bodies in their source order.
            if retained != entity_index {
                list.elements.swap(retained, entity_index);
            }
            retained += 1;
        } else {
            drops.push(super::DuplicateUuidDrop {
                source: source.to_owned(),
                chunk,
                entity_index,
                uuid: uuid.to_string(),
                staged_nbt: PathBuf::new(),
            });
        }
    }
    list.elements.truncate(retained);
    Ok(())
}

fn stage_duplicate_uuid_drops(
    source: &Path,
    stage: &Path,
    drops: &mut [super::DuplicateUuidDrop],
) -> Result<(), CoreError> {
    for (index, drop) in drops.iter_mut().enumerate() {
        drop.staged_nbt =
            PathBuf::from("duplicate-uuid-dropped").join(format!("{index}-{}.nbt", drop.uuid));
    }
    let mut sources: BTreeMap<_, BTreeMap<_, Vec<_>>> = BTreeMap::new();
    for drop in drops.iter() {
        sources
            .entry(drop.source.as_path())
            .or_default()
            .entry(drop.chunk)
            .or_default()
            .push(drop);
    }
    // Revisit only files with losses; never retain every original entity NBT
    // during the upgrading/typed-admission pass.
    for (relative, mut chunks) in sources {
        visit(&source.join(relative), |pos, tag, _| {
            let Some(records) = chunks.remove(&[pos.x, pos.z]) else {
                return Ok(());
            };
            let legacy = field(&tag, "Level")?
                .map(|level| field(level, "Entities"))
                .transpose()?
                .flatten();
            let lists = [legacy, field(&tag, "entities")?, field(&tag, "Entities")?];
            let mut lists = lists.into_iter().flatten();
            let Some(Tag::List(list)) = lists.next() else {
                return Err(fail(format!(
                    "{} chunk ({}, {}): missing original dropped-entity list",
                    relative.display(),
                    pos.x,
                    pos.z
                )));
            };
            if lists.next().is_some() {
                return Err(fail(format!(
                    "{} chunk ({}, {}): ambiguous original dropped-entity list",
                    relative.display(),
                    pos.x,
                    pos.z
                )));
            }
            for drop in records {
                let original = list.elements.get(drop.entity_index).ok_or_else(|| {
                    fail(format!(
                        "{} chunk ({}, {}) entity-index {} UUID {}: original dropped entity missing",
                        relative.display(), pos.x, pos.z, drop.entity_index, drop.uuid
                    ))
                })?;
                super::write_nbt(&stage.join(&drop.staged_nbt), original)?;
            }
            Ok(())
        })?;
        if !chunks.is_empty() {
            return Err(fail(format!(
                "{}: original dropped-entity chunks missing: {:?}",
                relative.display(),
                chunks.keys().collect::<Vec<_>>()
            )));
        }
    }
    Ok(())
}

fn image(manifest: &mut CheckpointManifest, dimension: Dimension) -> &mut CheckpointManifest {
    if dimension == Dimension::Overworld {
        manifest
    } else {
        manifest
            .dimensions
            .get_mut(&dimension)
            .expect("initialized child")
    }
}

fn ticks(
    chunk: &mut Chunk,
    manifest: &mut CheckpointManifest,
    registry: &crate::registry::CoreRegistry,
) -> Result<(), CoreError> {
    let key = [chunk.pos.x, chunk.pos.z];
    let mut block = Vec::with_capacity(chunk.scheduled_block_ticks.len());
    for tick in &mut chunk.scheduled_block_ticks {
        if registry.blocks.block(&tick.block).is_none() {
            return Err(fail(format!(
                "chunk {key:?}: unknown scheduled block {}",
                tick.block
            )));
        }
        let sequence = manifest.block_ticks.next_sequence;
        manifest.block_ticks.next_sequence = sequence
            .checked_add(1)
            .ok_or_else(|| fail("block tick sequence overflow"))?;
        block.push(crate::block_ticks::BlockTickImage {
            pos: [tick.pos.x, tick.pos.y, tick.pos.z],
            block: tick.block.clone(),
            trigger_tick: tick.trigger_tick,
            priority: tick.priority,
            sequence,
        });
        *tick = mc_world::ScheduledBlockTick::from_storage(
            tick.pos,
            tick.block.clone(),
            tick.trigger_tick,
            tick.priority,
            sequence,
        );
    }
    manifest
        .block_ticks
        .chunks
        .push(crate::block_ticks::BlockTickChunkImage {
            chunk: key,
            ticks: block,
        });
    manifest.fluid_ticks.covered.push(key);
    for tick in &mut chunk.scheduled_fluid_ticks {
        if mc_world::ScheduledFluidTick::fluid_kind(&tick.fluid).is_none() {
            return Err(fail(format!("unknown scheduled fluid {}", tick.fluid)));
        }
        let sequence = manifest.fluid_ticks.next_sequence;
        manifest.fluid_ticks.next_sequence = sequence
            .checked_add(1)
            .ok_or_else(|| fail("fluid tick sequence overflow"))?;
        manifest
            .fluid_ticks
            .due
            .push(crate::fluids::FluidTickImage {
                pos: [tick.pos.x, tick.pos.y, tick.pos.z],
                fluid: tick.fluid.clone(),
                deadline: tick.trigger_tick,
                priority: tick.priority,
                sequence,
            });
        *tick = mc_world::ScheduledFluidTick::from_storage(
            tick.pos,
            tick.fluid.clone(),
            tick.trigger_tick,
            tick.priority,
            sequence,
        );
    }
    Ok(())
}

fn legacy_root(dimension: Dimension) -> &'static Path {
    Path::new(match dimension {
        Dimension::Overworld => "",
        Dimension::Nether => "DIM-1",
        Dimension::End => "DIM1",
    })
}

/// Layout admission is read-only: mappings never rename or write source files.
fn dimension_root(paths: &[PathBuf], dimension: Dimension) -> Result<PathBuf, CoreError> {
    let modern = dimension.directory(Path::new(""));
    let legacy = legacy_root(dimension);
    let active = |root: &Path, old: bool| {
        paths.iter().any(|path| {
            ["region", "entities", "poi", "data"]
                .iter()
                .filter(|domain| !(old && dimension == Dimension::Overworld && **domain == "data"))
                .any(|domain| path.starts_with(root.join(domain)))
        })
    };
    match (active(&modern, false), active(legacy, true)) {
        (true, true) => Err(fail(format!(
            "{}: ambiguous old/new dimension locations {} and {}",
            dimension.name(),
            legacy.display(),
            modern.display()
        ))),
        (_, true) => Ok(legacy.into()),
        _ => Ok(modern),
    }
}

fn player_path(path: &Path) -> bool {
    matches!(path.parent(), Some(parent) if parent == Path::new("players/data") || parent == Path::new("playerdata"))
        && path.extension().and_then(|value| value.to_str()) == Some("dat")
}

fn typed_saved_data(path: &Path) -> bool {
    path == Path::new("data/minecraft/scoreboard.dat")
        || (matches!(
            path.parent().and_then(Path::to_str),
            Some(
                "dimensions/minecraft/overworld/data/minecraft"
                    | "dimensions/minecraft/the_nether/data/minecraft"
                    | "dimensions/minecraft/the_end/data/minecraft"
            )
        ) && matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("raids.dat" | "ender_dragon_fight.dat" | "world_border.dat" | "chunk_tickets.dat")
        ))
}

/// Retired vanilla stores have no native owner or executed DFU path. Preserve
/// their entire file bytes without allocating a historical feature tree merely
/// to invent an effective source version. Unknown saved-data names stay refused.
pub(super) fn retired_saved_data(path: &Path) -> bool {
    matches!(
        path.to_str(),
        Some(
            "data/Mineshaft.dat"
                | "data/Monument.dat"
                | "data/Stronghold.dat"
                | "data/Temple.dat"
                | "data/Village.dat"
                | "data/villages.dat"
                | "data/villages_end.dat"
                | "data/villages_nether.dat"
        )
    )
}

fn opaque_saved_data(path: &Path) -> bool {
    let Some(name) = path.to_str() else {
        return false;
    };
    let maps = name.strip_prefix("data/minecraft/maps/");
    let map = maps.and_then(|name| name.strip_suffix(".dat"));
    let numbered_map =
        map.is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()));
    let command_storage = name
        .strip_prefix("data/")
        .and_then(|name| name.strip_suffix("/command_storage.dat"))
        .is_some_and(|namespace| {
            !namespace.is_empty()
                && namespace.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'.' | b'-')
                })
        });
    numbered_map
        || command_storage
        || matches!(
            name,
            "data/minecraft/maps/last_id.dat"
                | "data/minecraft/custom_boss_events.dat"
                | "data/minecraft/random_sequences.dat"
                | "data/minecraft/scheduled_events.dat"
                | "data/minecraft/stopwatches.dat"
        )
}

fn record_deferred(
    stats: &mut ConversionStats,
    record: String,
    destination: &Path,
    kind: &str,
    version: i32,
) {
    stats.upgrades.push(RecordUpgradeReceipt {
        record,
        kind: kind.into(),
        from: version,
        to: version,
        steps: Vec::new(),
        preserved: vec![super::UpgradePreservationReceipt {
            path: destination.to_string_lossy().into_owned(),
            reason: "original file bytes and DataVersion retained without execution; saved-data NBT transformation is deferred to vanilla load, not a vanilla DFU execution or gameplay-parity claim".into(),
        }],
        saved_data: Vec::new(),
    });
}

fn record_upgrade(
    stats: &mut ConversionStats,
    record: String,
    kind: RecordKind,
    report: &UpgradeReport,
    mapping: Option<String>,
) {
    let mut receipt = RecordUpgradeReceipt::from_report(record, kind, report);
    if let Some(reason) = mapping {
        receipt.preserved.push(super::UpgradePreservationReceipt {
            path: receipt.record.clone(),
            reason,
        });
    }
    stats.upgrades.push(receipt);
}

fn player_uuid(tag: &Tag, path: &str) -> Result<Option<uuid::Uuid>, CoreError> {
    match field(tag, "UUID")? {
        None => Ok(None),
        Some(Tag::IntArray(words)) if words.len() == 4 => {
            let bits = words
                .iter()
                .fold(0_u128, |bits, word| (bits << 32) | u128::from(*word as u32));
            let uuid = uuid::Uuid::from_u128(bits);
            if uuid.is_nil() {
                return Err(fail(format!("{path}: nil player UUID")));
            }
            Ok(Some(uuid))
        }
        _ => Err(fail(format!("{path}: expected four-int player UUID"))),
    }
}

fn missing_player_dimension(tag: Option<&Tag>) -> bool {
    matches!(tag, Some(Tag::String(name))
        if Dimension::parse(name).is_none() && mc_data::Identifier::parse(name).is_ok())
}

pub(super) fn player_missing_dimension_source(tag: &Tag) -> Result<Option<String>, CoreError> {
    let respawn = field(tag, "respawn")?;
    if !missing_player_dimension(field(tag, "Dimension")?)
        && !missing_player_dimension(field(tag, "SpawnDimension")?)
        && !missing_player_dimension(
            respawn
                .map(|respawn| field(respawn, "dimension"))
                .transpose()?
                .flatten(),
        )
    {
        return Ok(None);
    }
    Ok(Some(format!(
        "Dimension={:?}, Pos={:?}, SpawnDimension={:?}, SpawnX={:?}, SpawnY={:?}, SpawnZ={:?}, respawn={respawn:?}",
        field(tag, "Dimension")?,
        field(tag, "Pos")?,
        field(tag, "SpawnDimension")?,
        field(tag, "SpawnX")?,
        field(tag, "SpawnY")?,
        field(tag, "SpawnZ")?,
    )))
}

pub(super) fn normalize_missing_player_dimensions(
    tag: &mut Tag,
    source_location: Option<String>,
    manifest: &CheckpointManifest,
    path: &str,
    losses: &mut Vec<String>,
) -> Result<(), CoreError> {
    let Some(source_location) = source_location else {
        return Ok(());
    };
    let relocate = missing_player_dimension(field(tag, "Dimension")?);
    let clear_respawn = missing_player_dimension(
        field(tag, "respawn")?
            .map(|respawn| field(respawn, "dimension"))
            .transpose()?
            .flatten(),
    );
    if !relocate && !clear_respawn {
        return Ok(());
    }
    let spawn = manifest
        .world_spawn
        .ok_or_else(|| fail(format!("{path}: missing overworld world spawn")))?;
    let Tag::Compound(fields) = tag else {
        return Err(fail(format!("{path}: expected player compound")));
    };
    if relocate {
        fields.retain(|(key, _)| !matches!(key.as_str(), "Dimension" | "Pos"));
        fields.push((
            "Dimension".into(),
            Tag::String("minecraft:overworld".into()),
        ));
        fields.push((
            "Pos".into(),
            Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::DOUBLE,
                elements: spawn
                    .into_iter()
                    .map(|value| Tag::Double(f64::from(value)))
                    .collect(),
            }),
        ));
    }
    if clear_respawn {
        fields.retain(|(key, _)| key != "respawn");
    }
    losses.push(format!(
        "PRESERVATION LOSS: {path}: owner-approved absent-dimension login fallback; relocated_to_overworld_world_spawn={relocate}, cleared_missing_dimension_respawn={clear_respawn}, world_spawn={spawn:?}; original source {source_location}; all other player fields retained"
    ));
    Ok(())
}

fn admit_player(
    mut tag: Tag,
    uuid: uuid::Uuid,
    path: &str,
    tick: u64,
    items: &mc_data::items::ItemRegistry,
    manifest: &mut CheckpointManifest,
    players: &mut Vec<NativePlayerSnapshot>,
) -> Result<(), CoreError> {
    if player_uuid(&tag, path)?.is_some_and(|actual| actual != uuid) {
        return Err(fail(format!(
            "{path}: player UUID disagrees with source identity"
        )));
    }
    let player = crate::journal::player_from_tag(&tag, items)
        .map_err(|error| fail(format!("{path}: {error}")))?;
    if player.pos.iter().any(|value| !value.is_finite())
        || !player.yaw.is_finite()
        || !player.pitch.is_finite()
        || !player.health.is_finite()
    {
        return Err(fail(format!("{path}: non-finite player")));
    }
    let Tag::Compound(fields) = &mut tag else {
        return Err(fail(format!("{path}: player is not compound")));
    };
    fields.retain(|(key, _)| !matches!(key.as_str(), "solaris:Tick" | "SolarisUnlockedRecipes"));
    if let Some((_, Tag::Compound(book))) = fields.iter_mut().find(|(key, _)| key == "recipeBook") {
        book.retain(|(key, _)| key != "recipes");
    }
    fields.push((
        "SolarisUnlockedRecipes".into(),
        Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::STRING,
            elements: player
                .unlocked_recipes
                .iter()
                .cloned()
                .map(Tag::String)
                .collect(),
        }),
    ));
    fields.push((
        "solaris:Tick".into(),
        Tag::Long(i64::try_from(tick).map_err(fail)?),
    ));
    manifest.players.insert(uuid.to_string(), tick);
    players.push(NativePlayerSnapshot {
        uuid: *uuid.as_bytes(),
        record: bridge::tag_record(tick, &tag)?,
    });
    Ok(())
}

pub(super) fn convert(
    config: &CoreConfig,
    source: &Path,
    stage: &Path,
) -> Result<ConversionStats, CoreError> {
    let paths = files(source)?;
    let mut consumed = BTreeSet::new();
    let mut registry = crate::registry::CoreRegistry::load(config.vanilla_data_dir.as_deref())?;
    // S1 native settlement startup is world policy: `Core::open` always
    // materializes it before admission, so converted worlds must persist the
    // same applied identity or reopen refuses the checkpoint.
    crate::plugin_startup::apply(
        &mut registry,
        &mc_plugin_host::DeploymentContribution::default(),
        None,
        &[],
    )
    .map_err(fail)?;
    let mut stats = ConversionStats::default();
    let roots = DIMENSIONS
        .iter()
        .map(|&dimension| Ok((dimension, dimension_root(&paths, dimension)?)))
        .collect::<Result<BTreeMap<_, _>, CoreError>>()?;
    if paths
        .iter()
        .any(|path| player_path(path) && path.parent() == Some(Path::new("playerdata")))
        && paths
            .iter()
            .any(|path| player_path(path) && path.parent() == Some(Path::new("players/data")))
    {
        return Err(fail(
            "ambiguous old/new active player locations playerdata and players/data",
        ));
    }
    if paths.contains(&PathBuf::from("resources.zip"))
        && paths.contains(&PathBuf::from("resourcepacks/resources.zip"))
    {
        return Err(fail(
            "resources.zip: ResourcePackLocationFileFix at DataVersion 4772: ambiguous source and destination resourcepacks/resources.zip",
        ));
    }
    // Resolve auxiliary custody before level splitting or opening a native DB.
    // Multiple source spellings must never silently share a canonical output.
    let mut auxiliary_sources = BTreeMap::new();
    for relative in &paths {
        let destination = super::auxiliary_destination(relative)?;
        if !super::safe_extra_path(&destination) || player_path(relative) {
            continue;
        }
        if destination.starts_with("dimensions")
            && !DIMENSIONS
                .iter()
                .any(|dimension| destination.starts_with(dimension.directory(Path::new(""))))
        {
            return Err(fail(format!(
                "{}: unsupported dimension",
                relative.display()
            )));
        }
        if let Some(previous) = auxiliary_sources.insert(destination.clone(), relative.clone()) {
            return Err(fail(format!(
                "{} and {}: duplicate auxiliary destination {}",
                previous.display(),
                relative.display(),
                destination.display()
            )));
        }
    }
    let mut metadata = crate::level_metadata::MetadataTags::new();
    let level_path = source.join("level.dat");
    let mut level = read_nbt(&level_path)?;
    let level_version = source_version(&level, &level_path)?;
    let data = field(&level, "Data")?.ok_or_else(|| fail("level.dat: missing Data"))?;
    // Extract the complete original player before the level walker, so its own
    // record version and receipt govern the upgrade exactly once.
    field(data, "Player")?;
    let Tag::Compound(root_fields) = &mut level else {
        return Err(fail("level.dat: expected compound"));
    };
    let data = root_fields
        .iter_mut()
        .find_map(|(name, tag)| (name == "Data").then_some(tag))
        .expect("validated Data");
    let Tag::Compound(data_fields) = data else {
        return Err(fail("level.dat: Data: expected compound"));
    };
    let mut embedded_player = data_fields
        .iter()
        .position(|(name, _)| name == "Player")
        .map(|index| data_fields.remove(index).1);
    let embedded_player_location = embedded_player
        .as_ref()
        .map(player_missing_dimension_source)
        .transpose()?
        .flatten();
    if let Some(player) = embedded_player.as_mut() {
        let path = "level.dat#Data.Player";
        // Embedded players use the level's DataVersion old chain for the
        // typed Player walker when they carry no stamp of their own; the
        // old level root itself stays >= 2586 via source_version above.
        let from = if field(player, "DataVersion")?.is_some() {
            player_source_version(player, &source.join(path))?
        } else {
            level_version
        };
        let report = upgrade::upgrade_player(player, from)
            .map_err(|error| fail(format!("{path}: {error}")))?;
        let uuid = player_uuid(player, path)?.ok_or_else(|| {
            fail(format!(
                "{path}: missing UUID; cannot extract embedded player"
            ))
        })?;
        let identity = field(player, "UUID")?.expect("validated UUID").clone();
        if field(data, "singleplayer_uuid")?.is_some_and(|value| value != &identity) {
            return Err(fail(
                "level.dat: Data.singleplayer_uuid disagrees with embedded player UUID",
            ));
        }
        let Tag::Compound(data_fields) = data else {
            unreachable!("validated Data")
        };
        if !data_fields
            .iter()
            .any(|(name, _)| name == "singleplayer_uuid")
        {
            data_fields.push(("singleplayer_uuid".into(), identity));
        }
        record_upgrade(
            &mut stats,
            path.into(),
            RecordKind::Player,
            &report,
            Some(format!(
                "embedded source player admitted as players/data/{uuid}.dat without modifying level.dat"
            )),
        );
    }
    let mut report = upgrade::upgrade_level(&mut level, level_version)
        .map_err(|error| fail(format!("{}: {error}", level_path.display())))?;
    if embedded_player.is_some() {
        report.preserved.push(PreservationReceipt {
            path: "Data.Player".into(),
            reason: "embedded player upgraded separately and extracted into native player record; Data.singleplayer_uuid retained".into(),
        });
    }
    if paths.contains(&PathBuf::from("resources.zip")) {
        report.preserved.push(PreservationReceipt {
            path: "resources.zip".into(),
            reason: "ResourcePackLocationFileFix4772: source archive retained verbatim in native conversion_extras as resourcepacks/resources.zip; no archive execution or source filesystem mutation".into(),
        });
    }
    record_upgrade(
        &mut stats,
        "level.dat".into(),
        RecordKind::Level,
        &report,
        None,
    );
    for saved in report.saved_data {
        crate::level_metadata::validate_relative_path(&saved.path)?;
        if auxiliary_sources.contains_key(Path::new(&saved.path))
            || metadata.contains_key(&saved.path)
        {
            return Err(fail(format!(
                "level.dat: LevelDatToSavedDataFileFix at DataVersion 4772: duplicate saved-data destination {}",
                saved.path
            )));
        }
        metadata.insert(saved.path, saved.tag);
    }
    metadata.insert("level.dat".into(), level);
    consumed.insert(PathBuf::from("level.dat"));
    for path in METADATA.into_iter().filter(|path| *path != "level.dat") {
        let relative = PathBuf::from(path);
        if paths.contains(&relative) {
            let tag = read_nbt(&source.join(path))?;
            check_version(&tag, &source.join(path))?;
            metadata.insert(path.into(), tag);
            consumed.insert(relative);
        }
    }
    // Latest saved data keeps its existing typed owner. Older envelopes whose
    // payload no DFU fixer rewrote are upgraded in place (raids 2586 renames);
    // anything else stays byte-identical in conversion_extras for vanilla load.
    for (destination, relative) in &auxiliary_sources {
        if !typed_saved_data(destination) {
            continue;
        }
        let key = destination
            .to_str()
            .ok_or_else(|| fail("non-UTF8 saved-data path"))?;
        if metadata.contains_key(key) {
            return Err(fail(format!(
                "{}: duplicate level split saved-data destination {key}",
                relative.display()
            )));
        }
        let path = source.join(relative);
        let mut tag = read_nbt(&path)?;
        let version = saved_data_source_version(&tag, &path)?;
        if version == mc_world::anvil::TARGET_DATA_VERSION {
            metadata.insert(key.into(), tag);
            consumed.insert(relative.clone());
        } else if key.ends_with("raids.dat") && (2586..4309).contains(&version) {
            let Tag::Compound(root) = &mut tag else {
                return Err(fail(format!("{}: expected compound", path.display())));
            };
            let Some(data_index) = root.iter().position(|(name, _)| name == "data") else {
                return Err(fail(format!("{}: missing data", path.display())));
            };
            let Tag::Compound(data) = &mut root[data_index].1 else {
                return Err(fail(format!("{}: invalid data", path.display())));
            };
            // SavedDataUUIDFix2514 rewrites only raid-member UUID payloads; an
            // empty raid list has no UUIDs to migrate, so it is vacuous here.
            // Non-empty raid entries keep their stored UUID shape for the
            // typed reader, which rejects unknown forms fail-closed.
            let level_receipt = stats
                .upgrades
                .iter()
                .position(|record| record.record == "level.dat")
                .expect("recorded level upgrade");
            let mut steps = Vec::new();
            upgrade_raids_payload(data, version, &mut steps);
            // Stamp current after the 4309 rename so the typed reader sees the
            // codec fields; the receipt preserves the original version.
            if let Some((_, Tag::Int(stamp))) =
                root.iter_mut().find(|(name, _)| name == "DataVersion")
            {
                *stamp = mc_world::anvil::TARGET_DATA_VERSION;
            }
            stats.upgrades[level_receipt].steps.extend(steps);
            stats.upgrades[level_receipt].preserved.push(super::UpgradePreservationReceipt {
                path: key.into(),
                reason: format!(
                    "RaidRenamesDataFix4309 applied to {key} from DataVersion {version}; SavedDataUUIDFix2514 vacuous on empty raid list"
                ),
            });
            metadata.insert(key.into(), tag);
            consumed.insert(relative.clone());
        } else {
            record_deferred(
                &mut stats,
                relative.to_string_lossy().into_owned(),
                destination,
                "saved_data",
                version,
            );
        }
    }
    let level = metadata
        .get("level.dat")
        .ok_or_else(|| fail("missing level.dat"))?;
    let data = field(level, "Data")?.ok_or_else(|| fail("level.dat: missing Data"))?;
    let source_tick = match field(data, "Time")? {
        Some(Tag::Long(time)) if *time >= 0 => *time as u64,
        _ => return Err(fail("level.dat: missing or negative Time")),
    };
    let worldgen = metadata
        .get("data/minecraft/world_gen_settings.dat")
        .ok_or_else(|| fail("missing modern world_gen_settings.dat"))?;
    let settings = field(worldgen, "data")?.ok_or_else(|| fail("worldgen: missing data"))?;
    if field(settings, "seed")? != Some(&Tag::Long(config.seed)) {
        return Err(fail("worldgen: configured seed disagrees with source"));
    }
    let dimensions =
        field(settings, "dimensions")?.ok_or_else(|| fail("worldgen: missing dimensions"))?;
    let Tag::Compound(entries) = dimensions else {
        return Err(fail("worldgen: invalid dimensions"));
    };
    if entries.len() != 3
        || DIMENSIONS
            .iter()
            .any(|dimension| !entries.iter().any(|(name, _)| name == dimension.name()))
    {
        return Err(fail("worldgen: unsupported dimension set"));
    }
    // Only the vanilla normal noise descriptors have a native generation policy.
    for dimension in DIMENSIONS {
        let descriptor = field(dimensions, dimension.name())?
            .ok_or_else(|| fail("missing dimension descriptor"))?;
        let generator = field(descriptor, "generator")?.ok_or_else(|| fail("missing generator"))?;
        let setting = match dimension {
            Dimension::Overworld => "minecraft:overworld",
            Dimension::Nether => "minecraft:nether",
            Dimension::End => "minecraft:end",
        };
        if field(descriptor, "type")? != Some(&Tag::String(dimension.name().into()))
            || field(generator, "type")? != Some(&Tag::String("minecraft:noise".into()))
            || field(generator, "settings")? != Some(&Tag::String(setting.into()))
        {
            return Err(fail(format!(
                "{}: unsupported worldgen descriptor",
                dimension.name()
            )));
        }
        let biome = field(generator, "biome_source")?
            .ok_or_else(|| fail("worldgen: missing biome_source"))?;
        let expected_type = if dimension == Dimension::End {
            "minecraft:the_end"
        } else {
            "minecraft:multi_noise"
        };
        if field(biome, "type")? != Some(&Tag::String(expected_type.into())) {
            return Err(fail("worldgen: unsupported biome source"));
        }
        if dimension != Dimension::End {
            let preset = if dimension == Dimension::Overworld {
                "minecraft:overworld"
            } else {
                "minecraft:nether"
            };
            if field(biome, "preset")? != Some(&Tag::String(preset.into())) {
                return Err(fail("worldgen: unsupported biome preset"));
            }
        }
    }
    let mut manifest = CheckpointManifest {
        tick: source_tick,
        domain_versions: bridge::domain_versions(),
        ..Default::default()
    };
    for dimension in [Dimension::Nether, Dimension::End] {
        manifest.dimensions.insert(
            dimension,
            Box::new(CheckpointManifest {
                tick: source_tick,
                domain_versions: bridge::domain_versions(),
                ..Default::default()
            }),
        );
    }
    crate::level_metadata::import_tags(&metadata, &mut manifest)?;
    let tick = manifest.tick;
    for child in manifest.dimensions.values_mut() {
        child.tick = tick;
        child.difficulty = manifest.difficulty;
        child.gamerules.clone_from(&manifest.gamerules);
    }
    // Statistics own their JSON; do not retain an obsolete opaque second copy.
    // Resolve before extras accounting so offline/statistics-only UUIDs are
    // admitted through the same native player custody as online players.
    let mut imported_statistics = BTreeMap::new();
    for (destination, relative) in &auxiliary_sources {
        if destination.parent() != Some(Path::new("players/stats"))
            || destination.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let name = destination
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| fail("invalid statistics filename"))?;
        let uuid = uuid::Uuid::parse_str(name).map_err(fail)?;
        if uuid.is_nil() || name != uuid.to_string() {
            return Err(fail("unsupported statistics filename"));
        }
        let input = std::fs::read(source.join(relative)).map_err(fail)?;
        let value = serde_json::from_slice(&input).map_err(fail)?;
        let statistics = bridge::statistics_from_json(value, &registry)
            .map_err(|error| fail(format!("{}: {error}", relative.display())))?;
        if imported_statistics.insert(uuid, statistics).is_some() {
            return Err(fail(format!("duplicate statistics UUID {uuid}")));
        }
        consumed.insert(relative.clone());
    }
    let mut saved_maps = BTreeMap::new();
    let mut map_sequence = 0_i64;
    // Only stat regular auxiliary files; session.lock must never be opened.
    let mut extras_bytes = 4_u64;
    for relative in paths.iter().filter(|path| !consumed.contains(*path)) {
        if player_path(relative) {
            continue;
        }
        let destination = super::auxiliary_destination(relative)?;
        let name = relative
            .to_str()
            .ok_or_else(|| fail("non-UTF8 extra path"))?;
        if name == "session.lock" || !super::safe_extra_path(&destination) {
            continue;
        }
        let size = std::fs::metadata(source.join(relative))
            .map_err(fail)?
            .len();
        let destination_name = destination
            .to_str()
            .ok_or_else(|| fail("non-UTF8 extra destination"))?;
        let encoded_name_bytes: u64 = destination_name
            .encode_utf16()
            .map(|unit| match unit {
                1..=0x7f => 1_u64,
                0..=0x7ff => 2,
                _ => 3,
            })
            .sum();
        if encoded_name_bytes > u64::from(u16::MAX) {
            return Err(fail(format!(
                "conversion_extras: NBT path exceeds name budget at {name}"
            )));
        }
        extras_bytes = extras_bytes
            .checked_add(size)
            .and_then(|bytes| bytes.checked_add(7 + encoded_name_bytes))
            .ok_or_else(|| fail(format!("conversion_extras: budget overflow at {name}")))?;
        if extras_bytes > EXTRAS_BUDGET {
            return Err(fail(format!(
                "conversion_extras: 16 MiB record budget exceeded at {name} ({extras_bytes} encoded bytes)"
            )));
        }
    }
    // Saved-data records are version-validated before terrain import. POI files
    // are intentionally opaque: only custody paths and filenames are validated.
    let level_receipt = stats
        .upgrades
        .iter()
        .position(|record| record.record == "level.dat")
        .expect("recorded level upgrade");
    for (destination, relative) in &auxiliary_sources {
        if consumed.contains(relative) || typed_saved_data(destination) {
            continue;
        }
        let name = destination
            .to_str()
            .ok_or_else(|| fail("non-UTF8 extra destination"))?;
        if retired_saved_data(destination) {
            stats.upgrades[level_receipt].preserved.push(super::UpgradePreservationReceipt {
                path: name.into(),
                reason: format!(
                    "source {} retained byte-exact as {name}: retired vanilla structure/village store is opaque and not decoded, version-inferred, transformed or activated as native state",
                    relative.display()
                ),
            });
            continue;
        }
        let path = source.join(relative);
        if name.ends_with(".dat") {
            if !opaque_saved_data(destination) {
                return Err(fail(format!("{name}: unsupported saved-data semantics")));
            }
            let mut tag = read_nbt(&path)?;
            if name == "data/minecraft/maps/last_id.dat" {
                map_sequence = crate::maps::nbt::sequence_from_tag(&tag).map_err(fail)?;
                consumed.insert(relative.clone());
                continue;
            }
            // The common saved-data loader defaults every absent version to
            // 1343, without imposing the region/level floor. Map upgrades own
            // their transformation; other old envelopes remain explicitly deferred.
            let is_map = name.starts_with("data/minecraft/maps/")
                && name != "data/minecraft/maps/last_id.dat";
            if is_map {
                let version = saved_data_source_version(&tag, &path)?;
                let report = upgrade::upgrade_map_data(&mut tag, version)
                    .map_err(|error| fail(format!("{}: {error}", path.display())))?;
                // Upgrade the source envelope, then activate its native map row.
                let mut receipt = RecordUpgradeReceipt::from_report(
                    relative.to_string_lossy().into_owned(),
                    RecordKind::Item,
                    &report,
                );
                receipt.kind = "saved_data".into();
                stats.upgrades.push(receipt);
                let id: i32 = destination
                    .file_stem()
                    .and_then(|id| id.to_str())
                    .ok_or_else(|| fail("invalid saved map filename"))?
                    .parse()
                    .map_err(|_| fail("invalid saved map id"))?;
                if id < 0 {
                    return Err(fail("negative saved map id"));
                }
                let map = crate::maps::nbt::from_tag(&tag).map_err(fail)?;
                saved_maps.insert(id, Arc::new(map));
                consumed.insert(relative.clone());
                continue;
            }
            let version = saved_data_source_version(&tag, &path)?;
            record_deferred(
                &mut stats,
                relative.to_string_lossy().into_owned(),
                destination,
                "saved_data",
                version,
            );
        } else if name.contains("/poi/") {
            validate_poi_path(destination)?;
            stats.upgrades[level_receipt].preserved.push(super::UpgradePreservationReceipt {
                path: name.into(),
                reason: format!(
                    "source {} retained as {name}: complete POI file bytes copied unchanged without decoding, DataVersion validation, native activation or DFU execution; vanilla load owns any POI upgrade",
                    relative.display()
                ),
            });
        } else if name.ends_with(".json") {
            let input = std::fs::read(&path).map_err(fail)?;
            serde_json::from_slice::<serde_json::Value>(&input).map_err(fail)?;
        }
    }
    let mut globals = bridge::generation_records(config)?;
    globals.insert(
        "map_sequence".into(),
        bridge::record(tick, serde_json::to_vec(&map_sequence).map_err(fail)?),
    );
    crate::maps::checkpoint(&saved_maps, tick, &mut globals)?;
    std::fs::create_dir(stage.join("native")).map_err(fail)?;
    let store = NativeStore::create(
        &stage.join("native/state.redb"),
        bridge::native_identity(config),
    )
    .map_err(fail)?;
    store
        .initialize_dimensions(&globals["dimensions"])
        .map_err(fail)?;
    stats.tick = tick;
    stats.durable_tick = tick;
    let mut entity_chunks = BTreeMap::new();
    // First-wins order is Overworld/Nether/End, inline region files then
    // external entity files in source path order, Anvil slot, entity index.
    // One set spans all dimensions; neither identities nor references change.
    let mut uuids = BTreeSet::new();
    let mut claims = Vec::new();
    for dimension in DIMENSIONS {
        let root = &roots[&dimension];
        let canonical_root = dimension.directory(Path::new(""));
        let region = root.join("region");
        let entities = root.join("entities");
        // Inline entities taken from upgraded region chunks this pass (at
        // most one region chunk per position).
        let mut chunk_entities: BTreeMap<(i32, i32), mc_world::anvil::EntityChunk> =
            BTreeMap::new();
        // Keep exact upgraded bodies only for inline chunks whose region also
        // has external entity storage; UUID filtering must not hide a conflict.
        let mut inline_entity_bodies: BTreeMap<(i32, i32), Vec<u8>> = BTreeMap::new();
        for relative in paths
            .iter()
            .filter(|path| path.parent() == Some(region.as_path()))
        {
            let path = source.join(relative);
            // files() traverses sorted relative path components.
            let has_entity_region = paths
                .binary_search(&entities.join(relative.file_name().expect("region filename")))
                .is_ok();
            visit(&path, |pos, mut tag, _| {
                let from = source_version(&tag, &path)?;
                // `__context.generator` is the worldgen generator type for this
                // dimension (absent stays absent: vanilla defaults to empty and
                // only exact `minecraft:noise` triggers noise retrogen).
                let generator = metadata
                    .get("data/minecraft/world_gen_settings.dat")
                    .and_then(|worldgen| field(worldgen, "data").ok().flatten())
                    .and_then(|settings| field(settings, "dimensions").ok().flatten())
                    .and_then(|dimensions| field(dimensions, dimension.name()).ok().flatten())
                    .and_then(|descriptor| field(descriptor, "generator").ok().flatten())
                    .and_then(|generator| field(generator, "type").ok().flatten())
                    .and_then(|kind| match kind {
                        Tag::String(kind) => Some(kind.as_str()),
                        _ => None,
                    });
                let report = upgrade::upgrade_chunk_in_dimension(
                    &mut tag,
                    from,
                    dimension.name(),
                    generator,
                )
                .map_err(|error| {
                    fail(format!(
                        "{} chunk ({}, {}): {error}",
                        path.display(),
                        pos.x,
                        pos.z
                    ))
                })?;
                record_upgrade(
                    &mut stats,
                    format!("{}#chunk({}, {})", relative.display(), pos.x, pos.z),
                    RecordKind::Chunk,
                    &report,
                    (root != &canonical_root).then(|| {
                        format!(
                            "source {} admitted as {}/{} without filesystem mutation",
                            relative.display(),
                            canonical_root.display(),
                            relative
                                .strip_prefix(root)
                                .expect("dimension source root")
                                .display()
                        )
                    }),
                );
                let mut chunk = mc_world::anvil::chunk_from_nbt_with_items_at_position_at_tick(
                    &tag,
                    pos,
                    &registry.blocks,
                    Some(&registry.items),
                    &registry.chest_loot,
                    dimension.geometry(),
                    source_tick,
                )
                .map_err(|error| {
                    fail(format!(
                        "{} chunk ({}, {}): {error}",
                        path.display(),
                        pos.x,
                        pos.z
                    ))
                })?;
                // SavedTick.filterTickListForChunk discards work outside its
                // serialized owner, even when the containing chunk exists.
                // Leave unknown identities to the existing strict validation.
                let owns_tick = |point: mc_world::BlockPos| {
                    point.x.div_euclid(16) == pos.x && point.z.div_euclid(16) == pos.z
                };
                chunk.scheduled_block_ticks.retain(|tick| {
                    if owns_tick(tick.pos) || registry.blocks.block(&tick.block).is_none() {
                        true
                    } else {
                        stats.dropped_foreign_scheduled_ticks += 1;
                        false
                    }
                });
                chunk.scheduled_fluid_ticks.retain(|tick| {
                    if owns_tick(tick.pos)
                        || (tick.fluid.as_str() != "minecraft:empty"
                            && mc_world::ScheduledFluidTick::fluid_kind(&tick.fluid).is_none())
                    {
                        true
                    } else {
                        stats.dropped_foreign_scheduled_ticks += 1;
                        false
                    }
                });
                // Vanilla EmptyFluid ticks have no effect. Discard them before
                // deciding whether the chunk has an owned tick payload.
                chunk.scheduled_fluid_ticks.retain(|tick| {
                    if tick.fluid.as_str() == "minecraft:empty" {
                        stats.dropped_noop_fluid_ticks += 1;
                        false
                    } else {
                        true
                    }
                });
                // Normalize only exact duplicate native requests, retaining
                // the first source sequence and all distinct deadlines/priorities.
                // Sort/dedup in place: no cloned identifiers or dedup allocation.
                let before = chunk.scheduled_block_ticks.len();
                chunk.scheduled_block_ticks.sort_unstable_by(|a, b| {
                    (
                        a.pos.x,
                        a.pos.y,
                        a.pos.z,
                        a.block.as_str(),
                        a.trigger_tick,
                        a.priority,
                        a.sequence(),
                    )
                        .cmp(&(
                            b.pos.x,
                            b.pos.y,
                            b.pos.z,
                            b.block.as_str(),
                            b.trigger_tick,
                            b.priority,
                            b.sequence(),
                        ))
                });
                chunk.scheduled_block_ticks.dedup_by(|a, b| {
                    a.pos == b.pos
                        && a.block == b.block
                        && a.trigger_tick == b.trigger_tick
                        && a.priority == b.priority
                });
                stats.deduplicated_scheduled_ticks +=
                    (before - chunk.scheduled_block_ticks.len()) as u64;
                chunk.scheduled_block_ticks.sort_unstable_by_key(|tick| {
                    (tick.trigger_tick, tick.priority, tick.sequence())
                });
                let before = chunk.scheduled_fluid_ticks.len();
                chunk.scheduled_fluid_ticks.sort_unstable_by(|a, b| {
                    (
                        a.pos.x,
                        a.pos.y,
                        a.pos.z,
                        a.fluid.as_str(),
                        a.trigger_tick,
                        a.priority,
                        a.sequence(),
                    )
                        .cmp(&(
                            b.pos.x,
                            b.pos.y,
                            b.pos.z,
                            b.fluid.as_str(),
                            b.trigger_tick,
                            b.priority,
                            b.sequence(),
                        ))
                });
                chunk.scheduled_fluid_ticks.dedup_by(|a, b| {
                    a.pos == b.pos
                        && a.fluid == b.fluid
                        && a.trigger_tick == b.trigger_tick
                        && a.priority == b.priority
                });
                stats.deduplicated_scheduled_ticks +=
                    (before - chunk.scheduled_fluid_ticks.len()) as u64;
                chunk.scheduled_fluid_ticks.sort_unstable_by_key(|tick| {
                    (tick.trigger_tick, tick.priority, tick.sequence())
                });
                // 1.16.5-era full chunks (and modern region files) carry
                // their entities inline: pre-2842 `Level.Entities` is
                // promoted to root lowercase `entities` by the upgrade
                // chain (ChunkRenamesFix; nested records already fully
                // upgraded by walk, so no second
                // `upgrade_entity_region` from the old version). Vanilla
                // `WorldUpgrader` routes those into ENTITY_CHUNK storage,
                // so the importer does the same: move/take the list,
                // wrap it in the modern ENTITY_CHUNK container
                // ({DataVersion, Position IntArray, Entities compound list})
                // and run the existing entity decode path. The list is
                // removed from the chunk tag so it does not linger in
                // `Chunk.extras` as a duplicate modern-full-chunk
                // remainder. Overlapping external storage is compared before
                // either source can be hidden by first-wins UUID filtering.
                if let Tag::Compound(root) = &mut tag
                    && let Some(index) = root.iter().position(|(key, _)| key == "entities")
                {
                    let (_, mut list) = root.remove(index);
                    let empty = matches!(&list, Tag::List(list) if list.elements.is_empty());
                    if !empty {
                        if has_entity_region {
                            let body = entity_body_bytes(&mut list)?;
                            inline_entity_bodies.insert((pos.x, pos.z), body);
                        }
                        admit_entity_uuids(
                            &mut list,
                            relative,
                            [pos.x, pos.z],
                            &mut uuids,
                            &mut stats.duplicate_uuid_drops,
                        )?;
                        let wrapper = Tag::Compound(vec![
                            (
                                "DataVersion".into(),
                                Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
                            ),
                            ("Position".into(), Tag::IntArray(vec![pos.x, pos.z])),
                            ("Entities".into(), list),
                        ]);
                        let entity_chunk = mc_world::anvil::entity_chunk_from_nbt(
                            &registry.items,
                            &wrapper,
                            &path,
                        )
                        .map_err(fail)?;
                        if entity_chunk.x != pos.x || entity_chunk.z != pos.z {
                            return Err(fail(format!(
                                "{}: inline entity Position mismatch",
                                path.display()
                            )));
                        }
                        if chunk_entities
                            .insert((pos.x, pos.z), entity_chunk)
                            .is_some()
                        {
                            return Err(fail(format!(
                                "{}: duplicate inline entity chunk ({}, {})",
                                path.display(),
                                pos.x,
                                pos.z
                            )));
                        }
                    }
                }
                // An offline import starts a new native journal lineage, even
                // when Java retained metadata from an earlier Solaris export.
                chunk.extras.retain(|(key, _)| key != "SolarisJournalLsn");
                // The inline list was taken above: chunk_from_nbt never saw
                // it, but drop any same-named remainder defensively so a
                // future decoder change cannot resurrect the duplicate.
                chunk.extras.retain(|(key, _)| key != "entities");
                if !matches!(chunk.status.as_str(), "full" | "minecraft:full") {
                    let is_air = |state| {
                        registry.blocks.by_id(state).is_some_and(|state| {
                            state.block.id.namespace() == "minecraft"
                                && matches!(state.block.id.path(), "air" | "cave_air" | "void_air")
                        })
                    };
                    let all_air = chunk.sections.iter().all(|section| {
                        section.is_empty()
                            || match (section.palette(), section.indices()) {
                                (Some(palette), Some(indices)) => (0..mc_world::SECTION_VOLUME)
                                    .all(|i| is_air(palette[indices.get(i) as usize])),
                                _ => is_air(section.get(0, 0, 0)),
                            }
                    });
                    let owned_payload = !chunk.block_entities.is_empty()
                        || !chunk.furnaces.is_empty()
                        || !chunk.chests.is_empty()
                        || !chunk.hoppers.is_empty()
                        || !chunk.brewing_stands.is_empty()
                        || !chunk.scheduled_block_ticks.is_empty()
                        || !chunk.scheduled_fluid_ticks.is_empty();
                    if all_air && !owned_payload {
                        let Tag::Compound(fields) = &tag else {
                            unreachable!("decoded chunk root")
                        };
                        for (name, value) in fields {
                            let loss = match name.as_str() {
                                "DataVersion" | "xPos" | "yPos" | "zPos" | "Status" => continue,
                                "sections"
                                | "HeightMap"
                                | "Heightmaps"
                                | "PostProcessing"
                                | "UpgradeData"
                                | "LastUpdate"
                                | "LightPopulated"
                                | "isLightOn"
                                | "TerrainPopulated"
                                | "hasLegacyStructureData" => false,
                                "InhabitedTime" if matches!(value, Tag::Long(0) | Tag::Int(0)) => false,
                                "fluid_ticks" => continue,
                                "block_entities" | "block_ticks" if matches!(value, Tag::List(list) if list.elements.is_empty()) =>
                                {
                                    continue;
                                }
                                "structures" => match value {
                                    Tag::Compound(fields) => fields.iter().any(|(_, value)| {
                                        !matches!(value, Tag::Compound(entries) if entries.is_empty())
                                    }),
                                    _ => true,
                                },
                                _ => true,
                            };
                            if loss {
                                if name == "structures" {
                                    stats.regenerated_structure_losses += 1;
                                }
                                if let Some(count) = stats.regenerated_chunk_loss_tags.get_mut(name)
                                {
                                    *count += 1;
                                } else {
                                    stats.regenerated_chunk_loss_tags.insert(name.clone(), 1);
                                }
                                stats.known_gaps.push(format!(
                                    "PRESERVATION LOSS: {}#chunk({}, {}) status {}: field {name} omitted for empty-chunk regeneration; original stays in source, Solaris worldgen is not seed-equivalent",
                                    relative.display(), pos.x, pos.z, chunk.status,
                                ));
                            }
                            if let Some(count) = stats.regenerated_chunk_tags.get_mut(name) {
                                *count += 1;
                            } else {
                                stats.regenerated_chunk_tags.insert(name.clone(), 1);
                            }
                        }
                        stats.non_full_empty_chunks += 1;
                        // No native coverage: the existing loader generates on demand.
                        // Inline/external entities retain their independent custody.
                        return Ok(());
                    }
                    if all_air {
                        // Block-entity inventories and scheduled ticks keep their
                        // normal native custody, even on an all-air source.
                        stats.non_full_air_with_payload_chunks += 1;
                    } else {
                        stats.non_full_non_air_chunks += 1;
                    }
                }
                for marker in chunk.generated_entities() {
                    claims.push((dimension, marker.claim.clone()));
                }
                ticks(&mut chunk, image(&mut manifest, dimension), &registry)?;
                store
                    .import_staged_chunk(
                        bridge::dimension_id(dimension),
                        &chunk,
                        &registry.blocks,
                        &registry.items,
                    )
                    .map_err(fail)?;
                stats.chunks += 1;
                Ok(())
            })?;
            consumed.insert(relative.clone());
        }
        let mut decoded_entities: BTreeMap<(i32, i32), mc_world::anvil::EntityChunk> =
            BTreeMap::new();
        for ((x, z), inline) in chunk_entities {
            if decoded_entities.insert((x, z), inline).is_some() {
                return Err(fail(format!(
                    "{}: duplicate inline entity chunk ({x}, {z})",
                    entities.display()
                )));
            }
        }
        for relative in paths
            .iter()
            .filter(|path| path.parent() == Some(entities.as_path()))
        {
            let path = source.join(relative);
            visit(&path, |pos, mut tag, _| {
                let from = source_version(&tag, &path)?;
                let report = upgrade::upgrade_entity_region(&mut tag, from).map_err(|error| {
                    fail(format!(
                        "{} chunk ({}, {}): {error}",
                        path.display(),
                        pos.x,
                        pos.z
                    ))
                })?;
                record_upgrade(
                    &mut stats,
                    format!("{}#chunk({}, {})", relative.display(), pos.x, pos.z),
                    RecordKind::EntityRegion,
                    &report,
                    (root != &canonical_root).then(|| {
                        format!(
                            "source {} admitted as {}/{} without filesystem mutation",
                            relative.display(),
                            canonical_root.display(),
                            relative
                                .strip_prefix(root)
                                .expect("dimension source root")
                                .display()
                        )
                    }),
                );
                if field(&tag, "Position")? != Some(&Tag::IntArray(vec![pos.x, pos.z])) {
                    return Err(fail(format!(
                        "{}: entity slot Position mismatch",
                        relative.display()
                    )));
                }
                if !matches!(field(&tag, "Entities")?, Some(Tag::List(list)) if list.elements.iter().all(|entry| matches!(entry, Tag::Compound(_))))
                {
                    return Err(fail(format!(
                        "{}: invalid Entities list",
                        relative.display()
                    )));
                }
                let Tag::Compound(root) = &mut tag else {
                    unreachable!("validated entity chunk root")
                };
                let list = root
                    .iter_mut()
                    .find_map(|(name, value)| (name == "Entities").then_some(value))
                    .expect("validated Entities list");
                let inline_body = inline_entity_bodies.remove(&(pos.x, pos.z));
                if let Some(expected) = &inline_body {
                    let body = entity_body_bytes(list)?;
                    if body != *expected {
                        return Err(fail(format!(
                            "{} chunk ({}, {}): conflicting inline region and entities/ records",
                            relative.display(),
                            pos.x,
                            pos.z
                        )));
                    }
                }
                admit_entity_uuids(
                    list,
                    relative,
                    [pos.x, pos.z],
                    &mut uuids,
                    &mut stats.duplicate_uuid_drops,
                )?;
                if inline_body.is_some() {
                    // The identical inline copy was already admitted; retain
                    // the normal duplicate-body receipts without decoding again.
                    return Ok(());
                }
                let chunk = mc_world::anvil::entity_chunk_from_nbt(&registry.items, &tag, &path)
                    .map_err(fail)?;
                // An occupied slot without an identical source body is a
                // storage-domain conflict, never permission to merge bodies.
                match decoded_entities.entry((chunk.x, chunk.z)) {
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        slot.insert(chunk);
                    }
                    std::collections::btree_map::Entry::Occupied(_) => {
                        return Err(fail(format!(
                            "{} chunk ({}, {}): conflicting inline region and entities/ records",
                            relative.display(),
                            pos.x,
                            pos.z
                        )));
                    }
                }
                Ok(())
            })?;
            consumed.insert(relative.clone());
        }
        entity_chunks.insert(dimension, decoded_entities);
        for (name, fight) in [("raids.dat", false), ("ender_dragon_fight.dat", true)] {
            let canonical = dimension
                .directory(Path::new(""))
                .join("data/minecraft")
                .join(name);
            let key = canonical
                .to_str()
                .ok_or_else(|| fail("non-UTF8 saved-data path"))?;
            if let Some(tag) = metadata.get(key) {
                if fight {
                    if dimension != Dimension::End {
                        return Err(fail(format!("{key}: dragon fight outside End")));
                    }
                    image(&mut manifest, dimension).end_fight = Some(
                        crate::end_fight::from_nbt(tag)
                            .map_err(|error| fail(format!("{key}: {error}")))?,
                    );
                } else {
                    image(&mut manifest, dimension).raids = Some(
                        crate::raids::from_nbt(tag)
                            .map_err(|error| fail(format!("{key}: {error}")))?,
                    );
                }
            }
        }
    }
    let mut ids = BTreeSet::new();
    stage_duplicate_uuid_drops(source, stage, &mut stats.duplicate_uuid_drops)?;
    let mut high = 0_i32;
    for chunks in entity_chunks.values() {
        for entity in chunks.values().flat_map(|chunk| &chunk.entities) {
            if entity.uuid.is_nil() {
                return Err(fail(format!("invalid entity UUID {}", entity.uuid)));
            }
            if entity.wire_id < 0 {
                return Err(fail("negative entity WireId"));
            }
            if entity.wire_id > 0 {
                let end = entity
                    .wire_id
                    .checked_add(if entity.id == "minecraft:ender_dragon" {
                        8
                    } else {
                        0
                    })
                    .ok_or_else(|| fail("dragon WireId overflow"))?;
                for id in entity.wire_id..=end {
                    if !ids.insert(id) {
                        return Err(fail(format!("duplicate entity/dragon WireId {id}")));
                    }
                }
                high = high.max(end);
            }
        }
    }
    let mut snapshots = Vec::new();
    for dimension in DIMENSIONS {
        let chunks = entity_chunks
            .get_mut(&dimension)
            .expect("dimension collected");
        for (&(x, z), chunk) in chunks.iter_mut() {
            for (index, entity) in chunk.entities.iter_mut().enumerate() {
                if let Some(raids) = image(&mut manifest, dimension).raids.as_mut() {
                    crate::raids::hydrate_native_entity(raids, entity).map_err(|error| {
                        let path = source
                            .join(&roots[&dimension])
                            .join("entities")
                            .join(format!("r.{}.{}.mca", x.div_euclid(32), z.div_euclid(32)));
                        let type_id = mc_data::Identifier::parse(&entity.id)
                            .ok()
                            .and_then(|name| registry.entity_types.id_of(&name));
                        fail(format!(
                        "{} chunk ({x}, {z}) entity-index {index} UUID {} type-id {} ({}): {error}",
                        path.display(), entity.uuid,
                        type_id.map_or_else(|| "unknown".into(), |id| id.to_string()), entity.id
                    ))
                    })?;
                }
                if entity.wire_id == 0 {
                    high = high
                        .checked_add(1)
                        .ok_or_else(|| fail("WireId space exhausted"))?;
                    entity.wire_id = high;
                    if entity.id == "minecraft:ender_dragon" {
                        high = high
                            .checked_add(8)
                            .ok_or_else(|| fail("dragon WireId space exhausted"))?;
                    }
                    stats.reminted_ids += 1;
                }
            }
        }
        let encoded = bridge::encode_entities(dimension, chunks, &registry.items, tick)?;
        let mut replay = Replay {
            end_fight: image(&mut manifest, dimension).end_fight.clone(),
            raids: image(&mut manifest, dimension).raids.clone(),
            wandering_trader: image(&mut manifest, dimension).wandering_trader,
            ..Default::default()
        };
        for entity in &encoded {
            crate::dimensions::recovery::hydrate_entity(
                &registry,
                &mut replay,
                entity.payload.as_ref().expect("live entity"),
                tick,
            )
            .map_err(|error| {
                let position = entity.position;
                let original = &chunks[&(position.x, position.z)];
                let index = original
                    .entities
                    .iter()
                    .position(|record| record.uuid.as_bytes() == &entity.identity)
                    .expect("encoded entity belongs to source chunk");
                let record = &original.entities[index];
                let type_id = mc_data::Identifier::parse(&record.id)
                    .ok()
                    .and_then(|name| registry.entity_types.id_of(&name));
                let path = source
                    .join(&roots[&dimension])
                    .join("entities")
                    .join(format!(
                        "r.{}.{}.mca",
                        position.x.div_euclid(32),
                        position.z.div_euclid(32)
                    ));
                fail(format!(
                    "{} chunk ({}, {}) entity-index {index} UUID {} type-id {} ({}): {error}",
                    path.display(),
                    position.x,
                    position.z,
                    record.uuid,
                    type_id.map_or_else(|| "unknown".into(), |id| id.to_string()),
                    record.id
                ))
            })?;
        }
        image(&mut manifest, dimension).end_fight = replay.end_fight;
        snapshots.extend(encoded);
    }
    for (dimension, claim) in claims {
        let hash = |seed: u64| {
            claim.bytes().fold(seed, |value, byte| {
                (value ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3)
            })
        };
        let uuid = uuid::Uuid::from_u128(
            (u128::from(hash(0xCBF2_9CE4_8422_2325)) << 64)
                | u128::from(hash(0x8422_2325_CBF2_9CE4)),
        );
        if uuids.contains(&uuid) {
            image(&mut manifest, dimension)
                .generated_entity_claims
                .insert(claim);
        }
    }
    stats.entities = snapshots.len() as u64;
    let mut players = Vec::new();
    let mut player_ids = BTreeSet::new();
    let mut embedded_player_id = None;
    if let Some(mut tag) = embedded_player {
        let path = "level.dat#Data.Player";
        let uuid = player_uuid(&tag, path)?.expect("validated embedded UUID");
        if uuids.contains(&uuid) {
            return Err(fail(format!(
                "{path}: player UUID {uuid} collides with entity UUID"
            )));
        }
        player_ids.insert(uuid);
        embedded_player_id = Some(uuid);
        normalize_missing_player_dimensions(
            &mut tag,
            embedded_player_location,
            &manifest,
            path,
            &mut stats.known_gaps,
        )?;
        admit_player(
            tag,
            uuid,
            path,
            tick,
            &registry.items,
            &mut manifest,
            &mut players,
        )?;
    }
    for relative in paths.iter().filter(|path| player_path(path)) {
        let name = relative
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| fail("invalid player filename"))?;
        let uuid = uuid::Uuid::parse_str(name).map_err(fail)?;
        if relative.extension().and_then(|value| value.to_str()) != Some("dat")
            || name != uuid.to_string()
            || uuid.is_nil()
        {
            return Err(fail("unsupported player filename"));
        }
        if !player_ids.insert(uuid) {
            if embedded_player_id == Some(uuid)
                && relative.parent() == Some(Path::new("playerdata"))
            {
                let external = read_nbt(&source.join(relative))?;
                if player_uuid(&external, &relative.to_string_lossy())?
                    .is_some_and(|actual| actual != uuid)
                {
                    return Err(fail(format!(
                        "{}: player UUID disagrees with source identity",
                        relative.display()
                    )));
                }
                // A legacy singleplayer save keeps both owner records. Its
                // embedded Player is authoritative, not a second native player.
                stats
                    .upgrades
                    .iter_mut()
                    .find(|entry| entry.record == "level.dat#Data.Player")
                    .expect("embedded player upgrade receipt")
                    .preserved
                    .push(super::UpgradePreservationReceipt {
                        path: relative.to_string_lossy().into_owned(),
                        reason: format!(
                            "external player record omitted; authoritative level.dat#Data.Player for UUID {uuid} retained"
                        ),
                    });
                consumed.insert(relative.clone());
                continue;
            }
            return Err(fail(format!(
                "{}: duplicate player UUID {uuid}",
                relative.display()
            )));
        }
        if uuids.contains(&uuid) {
            return Err(fail(format!(
                "{}: player UUID {uuid} collides with entity UUID",
                relative.display()
            )));
        }
        let path = source.join(relative);
        let mut tag = read_nbt(&path)?;
        let source_location = player_missing_dimension_source(&tag)?;
        // Versionless players report -1 (vanilla PlayerDataStorage/NbtUtils
        // default), NOT 1343: -1 runs the full legacy fixer chain, while
        // 1343 would skip most of it. The per-kind floor
        // (players/maps accept -1) lives in mc-world's upgrade module.
        let from = player_source_version(&tag, &path)?;
        let report = upgrade::upgrade_player(&mut tag, from)
            .map_err(|error| fail(format!("{}: {error}", path.display())))?;
        record_upgrade(
            &mut stats,
            relative
                .to_str()
                .ok_or_else(|| fail("non-UTF8 player path"))?
                .into(),
            RecordKind::Player,
            &report,
            (relative.parent() == Some(Path::new("playerdata"))).then(|| {
                format!(
                    "source {} admitted as players/data/{uuid}.dat without filesystem mutation",
                    relative.display()
                )
            }),
        );
        normalize_missing_player_dimensions(
            &mut tag,
            source_location,
            &manifest,
            &relative.to_string_lossy(),
            &mut stats.known_gaps,
        )?;
        admit_player(
            tag,
            uuid,
            relative
                .to_str()
                .ok_or_else(|| fail("non-UTF8 player path"))?,
            tick,
            &registry.items,
            &mut manifest,
            &mut players,
        )?;
        consumed.insert(relative.clone());
    }
    for (uuid, statistics) in imported_statistics {
        if uuids.contains(&uuid) {
            return Err(fail(format!(
                "statistics UUID {uuid} collides with entity UUID"
            )));
        }
        if let Some(player) = players
            .iter_mut()
            .find(|player| player.uuid == *uuid.as_bytes())
        {
            let mut tag = bridge::decode_tag(&player.record, "player")?;
            let Tag::Compound(fields) = &mut tag else {
                unreachable!("validated player compound")
            };
            fields.retain(|(name, _)| name != "SolarisStatistics");
            let encoded = serde_json::to_vec(&statistics)
                .map_err(fail)?
                .into_iter()
                .map(|byte| byte as i8)
                .collect();
            fields.push(("SolarisStatistics".into(), Tag::ByteArray(encoded)));
            player.record = bridge::tag_record(tick, &tag)?;
        } else {
            let mut player =
                crate::journal::player_from_tag(&Tag::Compound(Vec::new()), &registry.items)?;
            player.statistics = statistics;
            player.pos = manifest.world_spawn.unwrap_or([0, 80, 0]).map(f64::from);
            player.yaw = manifest.world_spawn_rotation[0];
            player.pitch = manifest.world_spawn_rotation[1];
            player.tick = tick;
            let tag = crate::journal::player_to_tag(&player, &registry.items)?;
            admit_player(
                tag,
                uuid,
                "statistics-only player",
                tick,
                &registry.items,
                &mut manifest,
                &mut players,
            )?;
        }
    }
    // Advancement documents belong to existing players only. Orphan files
    // remain byte-for-byte extras rather than creating fictitious players.
    for player in &mut players {
        let uuid = uuid::Uuid::from_bytes(player.uuid);
        let destination = PathBuf::from(format!("players/advancements/{uuid}.json"));
        let Some(relative) = auxiliary_sources.get(&destination) else {
            continue;
        };
        let bytes = std::fs::read(source.join(relative)).map_err(fail)?;
        let progress: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| fail(format!("{}: {error}", relative.display())))?;
        if !progress.is_object() {
            return Err(fail(format!(
                "{}: expected advancement document object",
                relative.display()
            )));
        }
        let mut tag = bridge::decode_tag(&player.record, "player")?;
        let Tag::Compound(fields) = &mut tag else {
            return Err(fail("player: expected compound"));
        };
        fields.retain(|(key, _)| key != "SolarisAdvancements");
        fields.push((
            "SolarisAdvancements".into(),
            Tag::ByteArray(
                serde_json::to_vec(&progress)
                    .map_err(fail)?
                    .into_iter()
                    .map(|byte| byte as i8)
                    .collect(),
            ),
        ));
        player.record = bridge::tag_record(tick, &tag)?;
        consumed.insert(relative.clone());
    }
    stats.players = players.len() as u64;
    let scoreboard = if let Some(tag) = metadata.get("data/minecraft/scoreboard.dat") {
        crate::scoreboard::Scoreboard::from_nbt(tag).map_err(fail)?;
        tag.clone()
    } else {
        crate::scoreboard::Scoreboard::default().to_nbt()
    };
    let mut extras: Vec<(String, Tag)> = Vec::new();
    for relative in paths.iter().filter(|path| !consumed.contains(*path)) {
        let name = relative
            .to_str()
            .ok_or_else(|| fail("non-UTF8 extra path"))?;
        crate::level_metadata::validate_relative_path(name)?;
        if name == "session.lock" {
            continue;
        }
        let destination = super::auxiliary_destination(relative)?;
        if !super::safe_extra_path(&destination) {
            return Err(fail(format!(
                "{name}: unsupported active or unsafe source file"
            )));
        }
        let destination_name = destination
            .to_str()
            .ok_or_else(|| fail("non-UTF8 extra destination"))?;
        if metadata.contains_key(destination_name) {
            return Err(fail(format!(
                "{name}: duplicate owned saved-data destination {destination_name}"
            )));
        }
        let bytes = std::fs::read(source.join(relative)).map_err(fail)?;
        extras.push((
            destination_name.into(),
            Tag::ByteArray(bytes.into_iter().map(|byte| byte as i8).collect()),
        ));
    }
    stats.opaque_files = extras.len() as u64;
    stats.opaque_retention = extras.iter().map(|(path, _)| path.clone()).collect();
    for dimension in DIMENSIONS {
        let image = image(&mut manifest, dimension);
        image.block_ticks.chunks.sort_by_key(|chunk| chunk.chunk);
        image.fluid_ticks.covered.sort_unstable();
        image
            .fluid_ticks
            .due
            .sort_by_key(|tick| (tick.deadline, tick.priority, tick.sequence));
    }
    bridge::validate_manifest(&manifest)?;
    for record in globals.values_mut() {
        record.captured_tick = tick;
    }
    globals.insert(
        "checkpoint".into(),
        bridge::record(tick, serde_json::to_vec(&manifest).map_err(fail)?),
    );
    globals.insert("scoreboard".into(), bridge::tag_record(tick, &scoreboard)?);
    globals.insert(
        "bossbars".into(),
        bridge::tag_record(tick, &crate::bossbar::BossBars::default().to_nbt())?,
    );
    globals.insert(
        "level_metadata".into(),
        bridge::tag_record(tick, &crate::level_metadata::to_tag(&metadata))?,
    );
    let extras_record = bridge::tag_record(tick, &Tag::Compound(extras))?;
    if extras_record.payload.len() as u64 > EXTRAS_BUDGET {
        return Err(fail(
            "conversion_extras: encoded NBT exceeds 16 MiB record budget",
        ));
    }
    globals.insert("conversion_extras".into(), extras_record);
    globals.insert(
        "plugins".into(),
        bridge::record(
            tick,
            serde_json::to_vec(&crate::plugin::PluginState::default()).map_err(fail)?,
        ),
    );
    globals.insert(
        "plugin_startup".into(),
        bridge::record(
            tick,
            crate::plugin_startup::world_identity(&registry).map_err(fail)?,
        ),
    );
    // This Immediate checkpoint is the durability fence for every staged
    // baseline transaction. The outer converter publishes only after it returns.
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick,
                previous_tick: 0,
                chunks: &[],
                entities: &snapshots,
                players: &players,
                globals: &globals,
            },
            &registry.blocks,
            &registry.items,
        )
        .map_err(fail)?;
    drop(store);
    Ok(stats)
}
