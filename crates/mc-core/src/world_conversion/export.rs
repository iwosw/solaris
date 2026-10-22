//! Read-only native checkpoint/WAL projection into modern Anvil files.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use mc_nbt::{ListTag, Tag, tag_type};
use mc_world::anvil::{ChunkPayload, TARGET_DATA_VERSION};
use mc_world::{BlockPos, BlockStateId, Chunk, ChunkPos, ScheduledBlockTick, ScheduledFluidTick};

use super::{ConversionStats, fail, write_nbt};
use crate::config::{CoreConfig, CoreError};
use crate::dimensions::Dimension;
use crate::journal::{CheckpointManifest, ReplayBlockEntity};
use crate::native_checkpoint::{decode_tag, encode_checkpoint, generator, recover_offline};

type Cell = (i32, i32, i32);
type Column = (i32, i32);

pub(super) fn convert(
    config: &CoreConfig,
    source: &Path,
    stage: &Path,
) -> Result<ConversionStats, CoreError> {
    let mut config = config.clone();
    config.world_dir = source.to_path_buf();
    let world = recover_offline(&config)?;
    let mut checkpoint = encode_checkpoint(&world)?;
    let manifest: CheckpointManifest =
        serde_json::from_slice(&checkpoint.globals["checkpoint"].payload).map_err(fail)?;
    let mut stats = ConversionStats {
        tick: checkpoint.tick,
        durable_tick: world.checkpoint.tick,
        ..Default::default()
    };
    let mut extras = BTreeMap::new();
    if let Some(record) = checkpoint.globals.get("conversion_extras") {
        let Tag::Compound(retained) = decode_tag(record, "conversion_extras")? else {
            return Err(fail("conversion_extras: expected compound"));
        };
        for (path, bytes) in retained {
            crate::level_metadata::validate_relative_path(&path)?;
            if !super::safe_extra_path(Path::new(&path)) {
                return Err(fail(format!(
                    "conversion_extras {path}: unsafe auxiliary path"
                )));
            }
            if path.starts_with("dimensions/")
                && ![Dimension::Overworld, Dimension::Nether, Dimension::End]
                    .iter()
                    .any(|dimension| {
                        Path::new(&path).starts_with(dimension.directory(Path::new("")))
                    })
            {
                return Err(fail(format!(
                    "conversion_extras {path}: unsupported dimension"
                )));
            }
            let Tag::ByteArray(bytes) = bytes else {
                return Err(fail(format!(
                    "conversion_extras {path}: expected byte array"
                )));
            };
            if extras.insert(path.clone(), bytes).is_some() {
                return Err(fail(format!(
                    "conversion_extras {path}: duplicate destination"
                )));
            }
        }
    }
    let mut stored = BTreeMap::<u32, BTreeSet<Column>>::new();
    world
        .reader
        .visit_chunk_positions(|dimension, pos| {
            stored.entry(dimension).or_default().insert((pos.x, pos.z));
            Ok(())
        })
        .map_err(fail)?;

    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let dimension_id = crate::native_checkpoint::dimension_id(dimension);
        let replay = &world.dimensions[&dimension];
        let generator = generator(&config, &world.registry, dimension)?;
        let mut columns = stored.remove(&dimension_id).unwrap_or_default();
        let mut blocks = BTreeMap::<Column, Vec<(Cell, u32)>>::new();
        let mut opaque = BTreeMap::<Column, Vec<(Cell, &Option<Vec<u8>>)>>::new();
        let mut typed = BTreeMap::<Column, Vec<(Cell, &ReplayBlockEntity)>>::new();
        for (&cell, &state) in &replay.blocks {
            let column = column(cell);
            columns.insert(column);
            blocks.entry(column).or_default().push((cell, state));
        }
        for (&cell, image) in &replay.opaque_block_entities {
            let column = column(cell);
            columns.insert(column);
            opaque.entry(column).or_default().push((cell, image));
        }
        for (&cell, image) in &replay.block_entities {
            let column = column(cell);
            columns.insert(column);
            typed.entry(column).or_default().push((cell, image));
        }
        let block_ticks: BTreeMap<_, _> = replay
            .block_ticks
            .chunks
            .iter()
            .map(|image| ((image.chunk[0], image.chunk[1]), image))
            .collect();
        columns.extend(block_ticks.keys().copied());
        let mut fluid_ticks = BTreeMap::<Column, Vec<&crate::fluids::FluidTickImage>>::new();
        for image in &replay.fluid_ticks.due {
            fluid_ticks
                .entry(column((image.pos[0], image.pos[1], image.pos[2])))
                .or_default()
                .push(image);
        }
        let fluid_coverage: BTreeSet<_> = replay
            .fluid_ticks
            .covered
            .iter()
            .map(|position| (position[0], position[1]))
            .collect();
        columns.extend(fluid_coverage.iter().copied());
        columns.extend(replay.inhabited_time.keys().map(|[x, z]| (*x, *z)));
        let mut regions = BTreeMap::<Column, Vec<Column>>::new();
        for column in columns {
            regions
                .entry((column.0.div_euclid(32), column.1.div_euclid(32)))
                .or_default()
                .push(column);
        }
        let directory = dimension.directory(stage);
        let region_dir = directory.join("region");
        std::fs::create_dir_all(&region_dir).map_err(fail)?;
        for ((rx, rz), positions) in regions {
            let mut payloads = Vec::with_capacity(positions.len());
            for (x, z) in positions {
                let pos = ChunkPos { x, z };
                let mut chunk = world
                    .reader
                    .read_chunk(
                        dimension_id,
                        pos,
                        &world.registry.blocks,
                        &world.registry.items,
                        &world.registry.chest_loot,
                        generator.as_ref(),
                    )
                    .map_err(fail)?
                    .unwrap_or_else(|| generator.generate(pos));
                if chunk.pos != pos || chunk.geometry() != dimension.geometry() {
                    return Err(fail(format!(
                        "{} chunk ({x},{z}): geometry/position mismatch",
                        dimension.name()
                    )));
                }
                // Native journal decisions are not portable Anvil metadata.
                chunk.extras.retain(|(key, _)| key != "SolarisJournalLsn");
                if let Some(image) = replay.inhabited_time.get(&[x, z]) {
                    image.apply(&mut chunk, replay.tick);
                }
                if let Some(edits) = blocks.remove(&(x, z)) {
                    for (cell, state) in edits {
                        apply_block(&mut chunk, cell, state, &world.registry)?;
                    }
                }
                if let Some(images) = opaque.remove(&(x, z)) {
                    for ((x, y, z), image) in images {
                        let pos = BlockPos { x, y, z };
                        if !chunk.contains_block_pos(pos) {
                            return Err(fail(format!(
                                "{} block entity ({x},{y},{z}): outside geometry",
                                dimension.name()
                            )));
                        }
                        remove_block_entity(&mut chunk, pos);
                        if let Some(bytes) = image {
                            let tag = mc_nbt::read_network(&mut bytes.as_slice()).map_err(fail)?;
                            let Tag::Compound(fields) = tag else {
                                return Err(fail(format!(
                                    "block entity ({x},{y},{z}): expected compound"
                                )));
                            };
                            mc_data::storage_blocks::validate(&fields, &world.registry.items)
                                .map_err(fail)?;
                            chunk.block_entities.insert(pos, bytes.clone());
                        }
                    }
                }
                if let Some(image) = block_ticks.get(&(x, z)) {
                    chunk.load_scheduled_block_ticks(
                        image
                            .ticks
                            .iter()
                            .map(|tick| {
                                ScheduledBlockTick::from_storage(
                                    BlockPos {
                                        x: tick.pos[0],
                                        y: tick.pos[1],
                                        z: tick.pos[2],
                                    },
                                    tick.block.clone(),
                                    tick.trigger_tick,
                                    tick.priority,
                                    tick.sequence,
                                )
                            })
                            .collect(),
                    );
                }
                if fluid_coverage.contains(&(x, z)) {
                    chunk.load_scheduled_fluid_ticks(
                        fluid_ticks
                            .remove(&(x, z))
                            .unwrap_or_default()
                            .into_iter()
                            .map(|tick| {
                                ScheduledFluidTick::from_storage(
                                    BlockPos {
                                        x: tick.pos[0],
                                        y: tick.pos[1],
                                        z: tick.pos[2],
                                    },
                                    tick.fluid.clone(),
                                    tick.deadline,
                                    tick.priority,
                                    tick.sequence,
                                )
                            })
                            .collect(),
                    );
                }
                filter_claimed_generated_markers(&mut chunk, &replay.generated_entity_claims);
                let mut tag = mc_world::anvil::chunk_nbt::chunk_to_nbt_with_items_at_tick(
                    &chunk,
                    &world.registry.blocks,
                    Some(&world.registry.items),
                    checkpoint.tick,
                )
                .map_err(fail)?;
                // Tick zero is a codec preservation mode; export always stamps the recovered clock.
                put(
                    compound_mut(&mut tag)?,
                    "LastUpdate",
                    Tag::Long(i64::try_from(checkpoint.tick).map_err(fail)?),
                );
                if let Some(images) = typed.remove(&(x, z)) {
                    for (cell, image) in images {
                        apply_typed(&mut tag, &chunk, cell, image, &world.registry)?;
                    }
                }
                portable_hopper_cooldowns(&mut tag, checkpoint.tick)?;
                payloads.push(payload(x, z, &tag)?);
                stats.chunks += 1;
            }
            mc_world::anvil::write_region_create_new(
                region_dir.join(format!("r.{rx}.{rz}.mca")),
                &payloads,
            )
            .map_err(fail)?;
        }
        export_entities(
            &directory.join("entities"),
            dimension_id,
            &mut checkpoint.entities,
            &mut stats,
        )?;
    }
    if !stored.is_empty() {
        return Err(fail("native export: unsupported stored dimension"));
    }

    let mut metadata = crate::level_metadata::from_tag(&decode_tag(
        &checkpoint.globals["level_metadata"],
        "level_metadata",
    )?)?;
    crate::level_metadata::checkpoint_tags(&mut metadata, config.seed, &manifest)?;
    let scoreboard_path = "data/minecraft/scoreboard.dat";
    let scoreboard = decode_tag(&checkpoint.globals["scoreboard"], "scoreboard")?;
    if extras.contains_key(scoreboard_path) {
        let owned = crate::scoreboard::Scoreboard::from_nbt(&scoreboard).map_err(fail)?;
        if owned != crate::scoreboard::Scoreboard::default() {
            return Err(fail(format!(
                "{scoreboard_path}: active native scoreboard conflicts with deferred saved data"
            )));
        }
    } else {
        refresh_saved(&mut metadata, scoreboard_path.into(), scoreboard)?;
    }
    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let replay = &world.dimensions[&dimension];
        let root = dimension.directory(Path::new(""));
        let saved_path = |name: &str| format!("{}/data/minecraft/{name}", root.display());
        let raid_path = saved_path("raids.dat");
        let fight_path = saved_path("ender_dragon_fight.dat");
        let border_path = saved_path("world_border.dat");
        let ticket_path = saved_path("chunk_tickets.dat");
        if extras.contains_key(&border_path)
            && replay.border != crate::command_border::Border::default()
        {
            return Err(fail(format!(
                "{border_path}: active native border conflicts with deferred saved data"
            )));
        }
        if extras.contains_key(&ticket_path)
            && replay
                .tickets
                .keys()
                .any(|owner| crate::forceload::is_forced_owner(owner))
        {
            return Err(fail(format!(
                "{ticket_path}: active native forced tickets conflict with deferred saved data"
            )));
        }
        if let Some(raids) = &replay.raids {
            if extras.contains_key(&raid_path) {
                if raids != &crate::raids::Raids::default() {
                    return Err(fail(format!(
                        "{raid_path}: active native raids conflict with deferred saved data"
                    )));
                }
            } else {
                refresh_saved(&mut metadata, raid_path, crate::raids::to_nbt(raids))?;
            }
        }
        if let Some(fight) = &replay.end_fight {
            if extras.contains_key(&fight_path) {
                return Err(fail(format!(
                    "{fight_path}: active native dragon fight conflicts with deferred saved data"
                )));
            }
            refresh_saved(&mut metadata, fight_path, crate::end_fight::to_nbt(fight))?;
        }
    }
    // checkpoint_tags generates defaults even for domains deliberately deferred
    // at import. Those placeholders do not own the retained Java file.
    for path in extras.keys() {
        let relative = Path::new(path);
        let generated_domain = path == scoreboard_path
            || (matches!(
                relative.parent().and_then(Path::to_str),
                Some(
                    "dimensions/minecraft/overworld/data/minecraft"
                        | "dimensions/minecraft/the_nether/data/minecraft"
                        | "dimensions/minecraft/the_end/data/minecraft"
                )
            ) && matches!(
                relative.file_name().and_then(|name| name.to_str()),
                Some(
                    "raids.dat"
                        | "ender_dragon_fight.dat"
                        | "world_border.dat"
                        | "chunk_tickets.dat"
                )
            ));
        if metadata.contains_key(path) && !generated_domain {
            return Err(fail(format!(
                "{path}: retained auxiliary file conflicts with owned metadata"
            )));
        }
        metadata.remove(path);
    }
    for (path, tag) in metadata {
        crate::level_metadata::validate_relative_path(&path)?;
        write_nbt(&stage.join(path), &tag)?;
    }
    for player in &checkpoint.players {
        let uuid = uuid::Uuid::from_bytes(player.uuid);
        let mut tag = decode_tag(&player.record, "player")?;
        let record = crate::journal::player_from_tag(&tag, &world.registry.items)?;
        let statistics =
            crate::native_checkpoint::statistics_to_json(&record.statistics, &world.registry)?;
        let statistics_path = format!("players/stats/{uuid}.json");
        if extras.contains_key(&statistics_path)
            || extras.contains_key(&format!("stats/{uuid}.json"))
        {
            return Err(fail(format!(
                "{statistics_path}: native statistics conflict with opaque legacy custody"
            )));
        }
        let path = stage.join(&statistics_path);
        std::fs::create_dir_all(path.parent().expect("statistics directory")).map_err(fail)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(fail)?;
        serde_json::to_writer(&mut file, &statistics).map_err(fail)?;
        file.write_all(b"\n").map_err(fail)?;
        file.sync_all().map_err(fail)?;
        let progress_path = format!("players/advancements/{uuid}.json");
        // A modeled player owns this destination even if an old conversion
        // retained an orphan file before that player was admitted.
        extras.remove(&progress_path);
        extras.remove(&format!("advancements/{uuid}.json"));
        let path = stage.join(&progress_path);
        std::fs::create_dir_all(path.parent().expect("advancement directory")).map_err(fail)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(fail)?;
        file.write_all(&serde_json::to_vec(&record.advancement_progress).map_err(fail)?)
            .map_err(fail)?;
        file.sync_all().map_err(fail)?;
        let Tag::Compound(fields) = &mut tag else {
            return Err(fail("player: expected compound"));
        };
        fields.retain(|(key, _)| {
            !matches!(
                key.as_str(),
                "SolarisAdvancements"
                    | "SolarisUnlockedRecipes"
                    | "SolarisStatistics"
                    | "SolarisWardenWarning"
            )
        });
        let book_index = fields.iter().position(|(key, _)| key == "recipeBook");
        if book_index.is_some() || !record.unlocked_recipes.is_empty() {
            let index = book_index.unwrap_or_else(|| {
                fields.push(("recipeBook".into(), Tag::Compound(Vec::new())));
                fields.len() - 1
            });
            let Tag::Compound(book) = &mut fields[index].1 else {
                return Err(fail("recipeBook: expected compound"));
            };
            book.retain(|(key, _)| key != "recipes");
            book.push((
                "recipes".into(),
                Tag::List(ListTag {
                    element_type: tag_type::STRING,
                    elements: record
                        .unlocked_recipes
                        .iter()
                        .cloned()
                        .map(Tag::String)
                        .collect(),
                }),
            ));
            if !book.iter().any(|(key, _)| key == "toBeDisplayed") {
                book.push((
                    "toBeDisplayed".into(),
                    Tag::List(ListTag {
                        element_type: tag_type::STRING,
                        elements: Vec::new(),
                    }),
                ));
            }
        }
        write_nbt(&stage.join(format!("players/data/{uuid}.dat")), &tag)?;
        stats.players += 1;
    }
    for (relative, bytes) in extras {
        stats.opaque_retention.push(relative.clone());
        stats.opaque_retention.push(format!(
            "{relative}: original file bytes retained without execution; any saved-data/POI upgrade remains deferred to vanilla load"
        ));
        let path = stage.join(relative);
        std::fs::create_dir_all(path.parent().expect("validated relative path")).map_err(fail)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(fail)?;
        let bytes: Vec<_> = bytes.into_iter().map(|byte| byte as u8).collect();
        file.write_all(&bytes).map_err(fail)?;
        file.sync_all().map_err(fail)?;
        stats.opaque_files += 1;
    }
    Ok(stats)
}

// A consumed placement is not a pending Anvil inhabitant, even after its
// entity dies. Filter the original NBT list rather than re-encoding surviving
// markers, so their unknown fields remain intact.
fn filter_claimed_generated_markers(chunk: &mut Chunk, claims: &BTreeSet<String>) {
    if claims.is_empty() {
        return;
    }
    let consumed: BTreeSet<_> = chunk
        .generated_entities()
        .into_iter()
        .filter(|marker| claims.contains(&marker.claim))
        .map(|marker| marker.claim)
        .collect();
    if consumed.is_empty() {
        return;
    }
    for (key, tag) in &mut chunk.extras {
        if key != "SolarisGeneratedEntities" {
            continue;
        }
        if let Tag::List(list) = tag {
            list.elements.retain(|entry| {
                let Tag::Compound(fields) = entry else { return true; };
                !matches!(mc_data::item_nbt::field(fields, "Claim"), Some(Tag::String(claim)) if consumed.contains(claim))
            });
        }
    }
}

fn column((x, _, z): Cell) -> Column {
    (x.div_euclid(16), z.div_euclid(16))
}

fn remove_block_entity(chunk: &mut Chunk, pos: BlockPos) {
    chunk.block_entities.remove(&pos);
    chunk.chests.remove(&pos);
    chunk.furnaces.remove(&pos);
    chunk.hoppers.remove(&pos);
    chunk.brewing_stands.remove(&pos);
}

// Native hopper deadlines use the recovered server clock. Java and a later
// import own only the bounded remaining cooldown, never that absolute anchor.
fn portable_hopper_cooldowns(tag: &mut Tag, tick: u64) -> Result<(), CoreError> {
    let root = compound_mut(tag)?;
    let Some(Tag::List(list)) = root
        .iter_mut()
        .find(|(key, _)| key == "block_entities")
        .map(|(_, tag)| tag)
    else {
        return Ok(());
    };
    for tag in &mut list.elements {
        let fields = compound_mut(tag)?;
        let Some(Tag::Long(until)) =
            mc_data::item_nbt::field(fields, "solaris:TransferCooldownUntil")
        else {
            continue;
        };
        let remaining = (*until).max(0) as u64;
        let remaining = remaining.saturating_sub(tick).min(8) as i32;
        fields.retain(|(key, _)| key != "solaris:TransferCooldownUntil");
        put(fields, "TransferCooldown", Tag::Int(remaining));
    }
    Ok(())
}

fn apply_block(
    chunk: &mut Chunk,
    (x, y, z): Cell,
    state: u32,
    registry: &crate::registry::CoreRegistry,
) -> Result<(), CoreError> {
    let lx = x.rem_euclid(16) as u8;
    let lz = z.rem_euclid(16) as u8;
    let previous = chunk
        .get_block(lx, y, lz)
        .ok_or_else(|| fail(format!("block ({x},{y},{z}): outside geometry")))?;
    let new = registry
        .blocks
        .by_id(BlockStateId(state))
        .ok_or_else(|| fail(format!("block ({x},{y},{z}): unknown state {state}")))?;
    let old = registry
        .blocks
        .by_id(previous)
        .ok_or_else(|| fail(format!("block ({x},{y},{z}): unknown stored state")))?;
    let same_family =
        old.block.id == new.block.id || crate::copper::same_family(&old.block.id, &new.block.id);
    if previous.0 != state {
        if !same_family {
            remove_block_entity(chunk, BlockPos { x, y, z });
        }
        chunk.set_block_and_update(
            lx,
            y,
            lz,
            BlockStateId(state),
            BlockStateId(registry.states.air),
        );
    }
    Ok(())
}

fn compound_mut(tag: &mut Tag) -> Result<&mut Vec<(String, Tag)>, CoreError> {
    match tag {
        Tag::Compound(fields) => Ok(fields),
        _ => Err(fail("native export: expected compound")),
    }
}
fn put(fields: &mut Vec<(String, Tag)>, key: &str, tag: Tag) {
    fields.retain(|(name, _)| name != key);
    fields.push((key.into(), tag));
}
fn payload(x: i32, z: i32, tag: &Tag) -> Result<ChunkPayload, CoreError> {
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", tag).map_err(fail)?;
    Ok(ChunkPayload {
        local_x: x.rem_euclid(32) as u8,
        local_z: z.rem_euclid(32) as u8,
        timestamp: 0,
        uncompressed_nbt: bytes,
    })
}

fn apply_typed(
    tag: &mut Tag,
    chunk: &Chunk,
    (x, y, z): Cell,
    image: &ReplayBlockEntity,
    registry: &crate::registry::CoreRegistry,
) -> Result<(), CoreError> {
    use mc_data::block_entity_kinds::BlockEntityKind;
    let state = chunk
        .get_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8)
        .and_then(|state| registry.blocks.by_id(state))
        .ok_or_else(|| fail(format!("block entity ({x},{y},{z}): invalid block")))?;
    let kind = BlockEntityKind::for_block(state.block.id.path());
    let furnace = matches!(
        kind,
        Some(BlockEntityKind::Furnace | BlockEntityKind::Smoker | BlockEntityKind::BlastFurnace)
    );
    if !furnace
        && !matches!(
            kind,
            Some(BlockEntityKind::Chest | BlockEntityKind::TrappedChest | BlockEntityKind::Barrel)
        )
    {
        // Other storage families own opaque full post-images, not this legacy typed shadow.
        return Ok(());
    }
    let root = compound_mut(tag)?;
    let list = root
        .iter_mut()
        .find(|(key, _)| key == "block_entities")
        .ok_or_else(|| fail("chunk: missing block_entities"))?;
    let Tag::List(list) = &mut list.1 else {
        return Err(fail("chunk block_entities: expected list"));
    };
    let index = list.elements.iter().position(|tag| match tag {
        Tag::Compound(fields) => [ ("x", x), ("y", y), ("z", z) ].into_iter()
            .all(|(key, expected)| matches!(mc_data::item_nbt::field(fields, key), Some(Tag::Int(value)) if *value == expected)),
        _ => false,
    });
    if index.is_none() {
        list.elements.push(Tag::Compound(vec![
            (
                "id".into(),
                Tag::String(format!(
                    "minecraft:{}",
                    if state.block.id.path() == "trapped_chest" {
                        "chest"
                    } else {
                        state.block.id.path()
                    }
                )),
            ),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
        ]));
    }
    list.element_type = tag_type::COMPOUND;
    let index = index.unwrap_or(list.elements.len() - 1);
    let fields = compound_mut(&mut list.elements[index])?;
    let mut items = Vec::new();
    for (slot, stack) in image.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        let slot = i8::try_from(slot).map_err(fail)?;
        let mut fields =
            mc_data::item_nbt::stack_to_fields(&registry.items, stack).map_err(fail)?;
        fields.push(("Slot".into(), Tag::Byte(slot)));
        items.push(Tag::Compound(fields));
    }
    put(
        fields,
        "Items",
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: items,
        }),
    );
    fields.retain(|(key, _)| !matches!(key.as_str(), "LootTable" | "LootTableSeed"));
    if furnace {
        let clock = image.furnace.as_ref();
        for (key, value) in [
            (
                "lit_time_remaining",
                clock.map_or(0, |clock| clock.lit_remaining),
            ),
            ("lit_total_time", clock.map_or(0, |clock| clock.lit_total)),
            (
                "cooking_time_spent",
                clock.map_or(0, |clock| clock.cook_progress),
            ),
            (
                "cooking_total_time",
                clock.map_or(200, |clock| clock.cook_total),
            ),
        ] {
            put(fields, key, Tag::Short(i16::try_from(value).map_err(fail)?));
        }
    }
    Ok(())
}

fn export_entities(
    directory: &Path,
    dimension: u32,
    entities: &mut [mc_world::native::NativeEntitySnapshot],
    stats: &mut ConversionStats,
) -> Result<(), CoreError> {
    let mut regions = BTreeMap::<Column, BTreeMap<Column, Vec<Tag>>>::new();
    for entity in entities
        .iter_mut()
        .filter(|entity| entity.dimension == dimension)
    {
        if let Some(tag) = entity.payload.take() {
            let x = entity.position.x;
            let z = entity.position.z;
            regions
                .entry((x.div_euclid(32), z.div_euclid(32)))
                .or_default()
                .entry((x, z))
                .or_default()
                .push(tag);
            stats.entities += 1;
        }
    }
    std::fs::create_dir_all(directory).map_err(fail)?;
    for ((rx, rz), chunks) in regions {
        let mut payloads = Vec::with_capacity(chunks.len());
        for ((x, z), entities) in chunks {
            let root = Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("Position".into(), Tag::IntArray(vec![x, z])),
                (
                    "Entities".into(),
                    Tag::List(ListTag {
                        element_type: tag_type::COMPOUND,
                        elements: entities,
                    }),
                ),
            ]);
            payloads.push(payload(x, z, &root)?);
        }
        mc_world::anvil::write_region_create_new(
            directory.join(format!("r.{rx}.{rz}.mca")),
            &payloads,
        )
        .map_err(fail)?;
    }
    Ok(())
}

fn refresh_saved(
    tags: &mut crate::level_metadata::MetadataTags,
    path: String,
    mut fresh: Tag,
) -> Result<(), CoreError> {
    if let Some(old) = tags.remove(&path) {
        merge_unknown(&mut fresh, old)?;
    }
    tags.insert(path, fresh);
    Ok(())
}

// Match retained list entries by durable codec identity, never by list index:
// removed scores, raids and teams must not be resurrected from a template.
fn same_saved_identity(current: &Tag, retained: &Tag) -> bool {
    let (Tag::Compound(current), Tag::Compound(retained)) = (current, retained) else {
        return false;
    };
    let names: &[&str] = if mc_data::item_nbt::field(current, "Name").is_some() {
        &["Name", "Objective"]
    } else if mc_data::item_nbt::field(current, "UUID").is_some() {
        &["UUID"]
    } else if mc_data::item_nbt::field(current, "id").is_some() {
        &["id"]
    } else {
        return false;
    };
    names.iter().all(|name| {
        mc_data::item_nbt::field(current, name) == mc_data::item_nbt::field(retained, name)
    })
}

// Refresh codec-owned values while retaining unowned saved-data envelope fields.
fn merge_unknown(fresh: &mut Tag, old: Tag) -> Result<(), CoreError> {
    match (fresh, old) {
        (Tag::Compound(fresh), Tag::Compound(old)) => {
            for (key, value) in old {
                if let Some((_, current)) = fresh.iter_mut().find(|(name, _)| *name == key) {
                    if key != "DisplaySlots"
                        && matches!(
                            (&*current, &value),
                            (Tag::Compound(_), Tag::Compound(_)) | (Tag::List(_), Tag::List(_))
                        )
                    {
                        merge_unknown(current, value)?;
                    }
                } else if !matches!(key.as_str(), "dragon_uuid" | "respawn_stage" | "TeamColor") {
                    fresh.push((key, value));
                }
            }
        }
        (Tag::List(fresh), Tag::List(mut old)) => {
            for current in &mut fresh.elements {
                if let Some(index) = old
                    .elements
                    .iter()
                    .position(|retained| same_saved_identity(current, retained))
                {
                    merge_unknown(current, old.elements.remove(index))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
