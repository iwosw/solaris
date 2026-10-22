//! Read-only probe of a stopped native world: exact blocks, their typed
//! container contents, stored player inventories and stored item entities.
//!
//! Scope matches the census: the pinned native checkpoint only, the journal is
//! never replayed, and a world with complete journal batches after its
//! checkpoint tick is refused instead of probed from a stale image. Nothing is
//! started or ticked; the only write is the receipt, which must lie outside the
//! source.
use super::{fail, hashes, write_receipt};
use crate::dimensions::Dimension;
use crate::journal::{CheckpointManifest, Replay, player_from_tag, visit_complete_batches_after};
use crate::native_checkpoint::{
    decode_tag, dimension_id, generation_records, generator, hydrate_visited_entity,
    native_identity, validate_manifest,
};
use crate::{CoreConfig, CoreError};
use mc_world::native::{NativeEntitySnapshot, NativeReadOnlyStore};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

const SCOPE: &str = "pinned native checkpoint only: requested block states and their typed container \
contents, stored player inventories (inventory, cursor, crafting grid) and stored item entities; no \
journal replay, no Core load, no tick; refused when journal batches follow the checkpoint";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeItem {
    /// Contract resource id of the stack, as the registry names it.
    pub item: String,
    pub count: i32,
    /// Where the stack sits: `container`, `inventory`, `cursor` or `grid`.
    pub place: String,
    /// Slot inside its owner.
    pub slot: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeBlock {
    pub dimension: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Stored block state, absent when the chunk holds no such position.
    pub state: Option<u32>,
    pub block: Option<String>,
    /// Non-empty container slots at this cell, in slot order.
    pub items: Vec<ProbeItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProbePlayer {
    pub uuid: String,
    pub dimension: String,
    pub position: [f64; 3],
    /// Every non-empty stored stack, inventory then cursor then grid.
    pub items: Vec<ProbeItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProbeEntity {
    pub type_id: u32,
    pub position: [f64; 3],
    pub item: Option<String>,
    pub count: i32,
}

/// Durable operator evidence; only a fully completed probe reports PASS.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeReceipt {
    pub status: String,
    pub operation: String,
    pub read_only: bool,
    pub source: PathBuf,
    pub scope: String,
    pub native_schema: u32,
    pub checkpoint_tick: Option<u64>,
    pub blocks: Vec<ProbeBlock>,
    pub players: Vec<ProbePlayer>,
    pub item_entities: Vec<ProbeEntity>,
    pub source_unchanged: Option<bool>,
    pub hash_algorithm: String,
    pub input_hashes: BTreeMap<String, String>,
    pub error: Option<String>,
}

/// Probe a stopped native world. `blocks` are absolute overworld positions.
pub fn probe_native(
    config: &CoreConfig,
    source: &Path,
    blocks: &[[i32; 3]],
    receipt_path: &Path,
) -> Result<ProbeReceipt, CoreError> {
    let receipt_parent = receipt_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let receipt_absolute = fs::canonicalize(receipt_parent).map_err(fail)?.join(
        receipt_path
            .file_name()
            .ok_or_else(|| fail("receipt has no filename"))?,
    );
    if let Ok(root) = fs::canonicalize(source)
        && receipt_absolute.starts_with(root)
    {
        return Err(fail("receipt must be outside source"));
    }
    let mut receipt = ProbeReceipt {
        status: "FAIL".into(),
        operation: "probe-native".into(),
        read_only: true,
        source: source.into(),
        scope: SCOPE.into(),
        native_schema: mc_world::native::NATIVE_PROTOTYPE_SCHEMA,
        hash_algorithm: "CRC32".into(),
        ..Default::default()
    };
    let result = probe(config, source, blocks, &mut receipt);
    if let Err(error) = &result {
        receipt.status = "FAIL".into();
        receipt.error = Some(error.to_string());
    }
    write_receipt(receipt_path, &receipt)?;
    result?;
    Ok(receipt)
}

fn probe(
    config: &CoreConfig,
    source: &Path,
    blocks: &[[i32; 3]],
    receipt: &mut ProbeReceipt,
) -> Result<(), CoreError> {
    let source = fs::canonicalize(source).map_err(fail)?;
    if !source.is_dir() {
        return Err(fail("source must be a directory"));
    }
    let lease = File::open(&source).map_err(fail)?;
    lease.try_lock().map_err(fail)?;
    receipt.input_hashes = hashes(&source)?;
    let mut config = config.clone();
    config.world_dir = source.clone();
    let path = source.join("native/state.redb");
    if !path.is_file() {
        return Err(crate::native_checkpoint::import_required(&config));
    }
    let registry = crate::registry::CoreRegistry::load(config.vanilla_data_dir.as_deref())?;
    let storage = |error: mc_world::native::NativeError| CoreError::Storage(error.to_string());
    let store =
        NativeReadOnlyStore::open_with_cache(&path, native_identity(&config), 16 * 1024 * 1024)
            .map_err(storage)?;
    let reader = store.checkpoint_reader().map_err(storage)?;
    let image = reader.read_checkpoint_metadata().map_err(storage)?;
    let manifest: CheckpointManifest = serde_json::from_slice(&image.globals["checkpoint"].payload)
        .map_err(|e| CoreError::Storage(format!("native checkpoint: {e}")))?;
    validate_manifest(&manifest)?;
    let expected = generation_records(&config)?;
    for key in ["generation", "dimensions"] {
        if image.globals[key].payload != expected[key].payload {
            return Err(CoreError::Worldgen(format!(
                "native {key}: configured generation identity mismatch"
            )));
        }
    }
    receipt.checkpoint_tick = Some(manifest.tick);
    let mut tail = 0u64;
    visit_complete_batches_after(&source.join("journal"), manifest.tick, |_| {
        tail += 1;
        Ok(())
    })?;
    if tail != 0 {
        return Err(fail(format!(
            "journal holds {tail} complete batches after checkpoint tick {}; a probe would not \
             match the reopened world (start and stop the server to fold the journal first)",
            manifest.tick
        )));
    }

    // Requested blocks: the composed chunk carries the current block state and
    // the typed container projection (the checkpoint persists block-entity
    // slots in the chunk metadata, not just the opaque client NBT image).
    let dimension = Dimension::Overworld;
    let chunk_generator = generator(&config, &registry, dimension)?;
    for &[x, y, z] in blocks {
        let chunk_pos = mc_world::ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        let chunk = reader
            .read_chunk(
                dimension_id(dimension),
                chunk_pos,
                &registry.blocks,
                &registry.items,
                &registry.chest_loot,
                chunk_generator.as_ref(),
            )
            .map_err(storage)?;
        let mut probe = ProbeBlock {
            dimension: dimension.name().into(),
            x,
            y,
            z,
            ..ProbeBlock::default()
        };
        if let Some(chunk) = chunk {
            probe.state = chunk
                .get_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8)
                .map(|state| state.0);
            probe.block = probe.state.and_then(|state| {
                registry
                    .blocks
                    .by_id(mc_world::BlockStateId(state))
                    .map(|block| block.block.id.as_str().to_owned())
            });
            let cell = mc_world::BlockPos { x, y, z };
            if let Some(chest) = chunk.chests.get(&cell) {
                for (slot, stack) in chest.slots.iter().enumerate() {
                    push_item(
                        &registry,
                        &mut probe.items,
                        "container",
                        slot as u32,
                        stack.count,
                        stack.item_id,
                    );
                }
            }
            if let Some(furnace) = chunk.furnaces.get(&cell) {
                for (slot, stack) in furnace.slots.iter().enumerate() {
                    push_item(
                        &registry,
                        &mut probe.items,
                        "container",
                        slot as u32,
                        stack.count,
                        stack.item_id,
                    );
                }
            }
            if let Some(hopper) = chunk.hoppers.get(&cell) {
                for (slot, stack) in hopper.slots.iter().enumerate() {
                    push_item(
                        &registry,
                        &mut probe.items,
                        "container",
                        slot as u32,
                        stack.count,
                        stack.item_id,
                    );
                }
            }
        }
        receipt.blocks.push(probe);
    }

    // Stored player post-images carry the exact inventory, cursor and grid.
    for player in &image.players {
        let tag = decode_tag(&player.record, "player")?;
        let record = match player_from_tag(&tag, &registry.items) {
            Ok(record) => record,
            Err(error) => {
                return Err(fail(format!(
                    "player {}: {error}",
                    uuid::Uuid::from_bytes(player.uuid)
                )));
            }
        };
        let mut probe = ProbePlayer {
            uuid: uuid::Uuid::from_bytes(player.uuid).to_string(),
            dimension: record.dimension.name().into(),
            position: record.pos,
            items: Vec::new(),
        };
        for (slot, stack) in record.inventory.iter().enumerate() {
            push_item(
                &registry,
                &mut probe.items,
                "inventory",
                slot as u32,
                stack.count,
                stack.item_id,
            );
        }
        push_item(
            &registry,
            &mut probe.items,
            "cursor",
            u32::from(record.selected),
            record.carried.count,
            record.carried.item_id,
        );
        for (slot, stack) in record.grid.iter().enumerate() {
            push_item(
                &registry,
                &mut probe.items,
                "grid",
                slot as u32,
                stack.count,
                stack.item_id,
            );
        }
        receipt.players.push(probe);
    }

    // Stored item entities, hydrated through the same validator recovery uses.
    let mut scratch = Replay::default();
    reader
        .visit_entities(|entity: NativeEntitySnapshot| {
            let Some(payload) = &entity.payload else {
                return Ok(());
            };
            let entity_dimension = match entity.dimension {
                0 => Dimension::Overworld,
                1 => Dimension::Nether,
                2 => Dimension::End,
                _ => unreachable!("native reader validates dimension"),
            };
            hydrate_visited_entity(
                &registry,
                &config,
                &mut scratch,
                entity_dimension,
                &entity,
                payload,
                manifest.tick,
            )?;
            for record in scratch.entities.values() {
                let Some(stack) = record.item.as_ref().filter(|stack| !stack.is_empty()) else {
                    continue;
                };
                receipt.item_entities.push(ProbeEntity {
                    type_id: record.type_id,
                    position: record.pos,
                    item: registry
                        .items
                        .name_of(stack.item_id)
                        .map(|id| id.as_str().to_owned()),
                    count: stack.count,
                });
            }
            scratch.entities.clear();
            Ok(())
        })
        .map_err(storage)?;

    drop(reader);
    drop(store);
    if receipt.input_hashes != hashes(&source)? {
        receipt.source_unchanged = Some(false);
        return Err(fail("source changed during probe"));
    }
    receipt.source_unchanged = Some(true);
    receipt.status = "PASS".into();
    Ok(())
}

fn push_item(
    registry: &crate::registry::CoreRegistry,
    items: &mut Vec<ProbeItem>,
    place: &str,
    slot: u32,
    count: i32,
    item_id: u32,
) {
    if count <= 0 {
        return;
    }
    let Some(item) = registry.items.name_of(item_id) else {
        return;
    };
    items.push(ProbeItem {
        item: item.as_str().to_owned(),
        count,
        place: place.to_owned(),
        slot,
    });
}
