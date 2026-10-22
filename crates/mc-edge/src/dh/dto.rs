//! DH protocol-16 full-data LOD DTO, format 1 with Zstd block compression.
//!
//! Independent implementation written from the protocol description in
//! `.analysis/DHCompat/design.md`; no upstream code is copied or translated.
//! The parent service owns session/config/split framing: this module only
//! converts already-filtered immutable native chunks into one wire-ready DTO.
//!
//! Wire shape (all big-endian): `i64` packed section pos, `i32` checksum
//! (unused, 0), eight `i32` length + bytes blobs (main, north/south/east/west
//! edges, generation steps, world compression, block/biome mapping), `u8`
//! format (1), `u8` compression (4 = one Zstd frame), three flag bytes
//! (apply-to-parent, apply-to-children, regenerate), `i64` modified-ms and
//! `i64` created-ms. Format 1 carries all 4096 columns in the main blob; the
//! four edge blobs are zero-length.
//!
//! Column `x * 64 + z` holds top-down packed runs:
//! `id | height<<32 | rel_min_y<<44 | sky<<56 | block<<60`, where `rel_min_y`
//! is measured from the chunks' minimum Y. Runs merge only on equal mapping
//! id *and* equal light nibbles, so air gaps, water, canopy and light
//! gradients survive as separate runs.
//!
//! Input contract: at least the 16 chunks covering the detail-6 section
//! (extra halo chunks are ignored); all must share one geometry. Chunks are
//! expected pre-filtered (anti-xray/visibility policy applied upstream).
//! Missing chunks, mixed geometries and unregistered states fail closed.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockState, BlockStateId, Chunk};

/// DH data-format version emitted here: full 64x64 column grid, no edges.
pub(crate) const DATA_FORMAT_V1: u8 = 1;
/// DH compression-mode value emitted here: Zstd block encoding (one frame).
pub(crate) const COMPRESSION_ZSTD_BLOCK: u8 = 4;
/// Only leaf sections (4x4 chunks, one block per column) are encodable.
pub(crate) const SECTION_DETAIL: u8 = 6;
/// Columns per section side; column index is `x * 64 + z` (Z fastest).
pub(crate) const SECTION_WIDTH: i32 = 64;
/// Hard cap on the finished DTO, matching the service byte budget.
pub(crate) const MAX_DTO_BYTES: usize = 4 * 1024 * 1024;
/// Generation step stamped per column. Callers must pass fully generated
/// chunks; this does not verify population or light-baked status itself.
const GEN_STEP_LIGHT: u8 = 9;
/// World compression stamped per column: plain same-block run merging.
const WORLD_COMPRESSION_MERGE: u8 = 0;
/// Zstd level for LOD blobs, matching the pinned stack's choice.
const ZSTD_LEVEL: i32 = 3;
/// Java `DataOutput.writeUTF` length ceiling in encoded bytes.
const MAX_JAVA_UTF_BYTES: usize = 65_535;
/// Packed-run field widths (12-bit height and relative Y).
const MAX_RUN_LEN: u32 = 0xFFF;

/// Presentation hook: map a native (block state, biome) pair to the canonical
/// DH mapping serial (`<biome>_DH-BSW_<block>_STATE_<sorted {name:value}>`).
/// Return `None` for the verbatim registry serial. Lets the service project
/// states the negotiated profile cannot name (e.g. newer blocks on an older
/// profile) through the established policy instead of a DTO-local fallback.
/// Overrides must be a pure function of the state content and biome text.
pub(crate) trait DhBlockProjector {
    /// Canonical serial override for `state` in `biome`, if any.
    fn project(&self, state: &BlockState, biome: &Identifier) -> Option<String>;
}

impl<F> DhBlockProjector for F
where
    F: Fn(&BlockState, &Identifier) -> Option<String>,
{
    fn project(&self, state: &BlockState, biome: &Identifier) -> Option<String> {
        self(state, biome)
    }
}

/// Identity projector: every pair keeps its verbatim registry serial.
pub(crate) struct VerbatimProjector;

impl DhBlockProjector for VerbatimProjector {
    fn project(&self, _state: &BlockState, _biome: &Identifier) -> Option<String> {
        None
    }
}

/// Pure conversion output: 4096 top-down packed columns plus mapping table.
pub(crate) struct DhColumns {
    /// 4096 entries in `x * 64 + z` order, each top-down (highest first).
    pub columns: Vec<Vec<u64>>,
    /// Mapping table in first-appearance order; run ids index into it.
    pub mapping: Vec<String>,
}

/// Service entry point: verbatim registry serials, `created` mirrors
/// `modified` (the service owns creation tracking if it needs it).
pub(crate) fn encode(
    position: i64,
    chunks: &[Arc<Chunk>],
    registry: &BlockRegistry,
    modified: i64,
) -> Result<Vec<u8>> {
    encode_projected(position, chunks, registry, &VerbatimProjector, modified)
}

/// Service entry point with a presentation hook for profile projection.
pub(crate) fn encode_projected(
    position: i64,
    chunks: &[Arc<Chunk>],
    registry: &BlockRegistry,
    project: &dyn DhBlockProjector,
    modified: i64,
) -> Result<Vec<u8>> {
    let (section_x, section_z) = decode_section(position)?;
    let cols = columns_for_section(chunks, registry, project, section_x, section_z)?;
    encode_columns(position, &cols, modified)
}

/// Decode a packed section pos into section coordinates. Detail lives in the
/// low 7 bits; X sits at bit 8 and Z at bit 36 as signed 28-bit fields.
fn decode_section(position: i64) -> Result<(i32, i32)> {
    let detail = (position & 0x7F) as u8;
    if detail != SECTION_DETAIL {
        bail!("dh dto encodes only detail-6 leaf sections, got detail {detail}");
    }
    let section_x = ((((position >> 8) & 0x0FFF_FFFF) << 36) >> 36) as i32;
    let section_z = ((((position >> 36) & 0x0FFF_FFFF) << 36) >> 36) as i32;
    Ok((section_x, section_z))
}

/// Canonical mapping serial for one native pair: biome and block resource
/// locations joined by `_DH-BSW_`, then `_STATE_` plus `{name:value}` pairs
/// in alphabetical property-name order (empty suffix when property-less).
pub(crate) fn canonical_serial(state: &BlockState, biome: &Identifier) -> String {
    let mut props: Vec<(&str, &str)> = state
        .properties
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    props.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = String::with_capacity(
        biome.as_str().len() + state.block.id.as_str().len() + 16 + props.len() * 8,
    );
    out.push_str(biome.as_str());
    out.push_str("_DH-BSW_");
    out.push_str(state.block.id.as_str());
    out.push_str("_STATE_");
    for (name, value) in props {
        out.push('{');
        out.push_str(name);
        out.push(':');
        out.push_str(value);
        out.push('}');
    }
    out
}

/// Java `DataOutput.writeUTF`: u16 big-endian encoded-byte count followed by
/// modified UTF-8 (null as 0xC0 0x80, supplementary chars as surrogate pairs).
pub(crate) fn write_java_utf(buf: &mut Vec<u8>, text: &str) -> Result<()> {
    fn push3(out: &mut Vec<u8>, v: u32) {
        out.push((0xE0 | (v >> 12)) as u8);
        out.push((0x80 | ((v >> 6) & 0x3F)) as u8);
        out.push((0x80 | (v & 0x3F)) as u8);
    }
    let mut encoded = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let v = ch as u32;
        if v == 0 {
            encoded.extend_from_slice(&[0xC0, 0x80]);
        } else if v < 0x80 {
            encoded.push(v as u8);
        } else if v < 0x800 {
            encoded.push((0xC0 | (v >> 6)) as u8);
            encoded.push((0x80 | (v & 0x3F)) as u8);
        } else if v < 0x10000 {
            push3(&mut encoded, v);
        } else {
            let v = v - 0x10000;
            push3(&mut encoded, 0xD800 + (v >> 10));
            push3(&mut encoded, 0xDC00 + (v & 0x3FF));
        }
    }
    if encoded.len() > MAX_JAVA_UTF_BYTES {
        bail!("dh mapping entry exceeds writeUTF ceiling");
    }
    buf.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
    buf.extend_from_slice(&encoded);
    Ok(())
}

/// Run currently being extended while scanning one column top-down.
#[derive(Debug, Clone, Copy)]
struct Open<'a> {
    state: BlockStateId,
    biome: &'a str,
    sky: u8,
    block: u8,
    bottom: i32,
    height: u32,
}

fn push_packed(
    runs: &mut Vec<u64>,
    map: u32,
    height: u32,
    bottom: i32,
    min_y: i32,
    sky: u8,
    block: u8,
) {
    debug_assert!((1..=MAX_RUN_LEN).contains(&height));
    let rel = (bottom - min_y) as u64;
    debug_assert!(rel <= u64::from(MAX_RUN_LEN));
    runs.push(
        u64::from(map)
            | (u64::from(height) << 32)
            | (rel << 44)
            | (u64::from(sky) << 56)
            | (u64::from(block) << 60),
    );
}

/// Pure column conversion: one scan per chunk, no repeated chunk loads.
/// Selects the section's 4x4 chunks out of the (possibly larger halo) slice.
pub(crate) fn columns_for_section(
    chunks: &[Arc<Chunk>],
    registry: &BlockRegistry,
    project: &dyn DhBlockProjector,
    section_x: i32,
    section_z: i32,
) -> Result<DhColumns> {
    let base_cx = section_x
        .checked_mul(4)
        .context("dh section coordinate out of chunk range")?;
    let base_cz = section_z
        .checked_mul(4)
        .context("dh section coordinate out of chunk range")?;
    let mut by_pos: HashMap<(i32, i32), &Arc<Chunk>> = HashMap::with_capacity(chunks.len());
    for chunk in chunks {
        by_pos.insert((chunk.pos.x, chunk.pos.z), chunk);
    }
    let first = by_pos
        .get(&(base_cx, base_cz))
        .with_context(|| format!("dh section needs chunk ({base_cx},{base_cz})"))?;
    let geometry = first.geometry();
    let (min_y, max_y) = (geometry.min_y(), geometry.max_y());

    let mut mapping: Vec<String> = Vec::new();
    let mut map_index: HashMap<(BlockStateId, &str), u32> = HashMap::new();
    let mut columns: Vec<Vec<u64>> = Vec::with_capacity(4096);

    for rel_x in 0..SECTION_WIDTH {
        for rel_z in 0..SECTION_WIDTH {
            let cx = base_cx + rel_x / 16;
            let cz = base_cz + rel_z / 16;
            let chunk = by_pos
                .get(&(cx, cz))
                .with_context(|| format!("dh section missing chunk ({cx},{cz})"))?;
            if chunk.geometry() != geometry {
                bail!("dh section mixes chunk geometries at ({cx},{cz})");
            }
            let lx = (rel_x % 16) as u8;
            let lz = (rel_z % 16) as u8;

            let mut runs: Vec<u64> = Vec::new();
            let mut open: Option<(Open<'_>, u32)> = None;
            let flush = |runs: &mut Vec<u64>, open: &Option<(Open<'_>, u32)>| {
                if let Some((run, map)) = open {
                    push_packed(
                        runs, *map, run.height, run.bottom, min_y, run.sky, run.block,
                    );
                }
            };

            let mut y = max_y - 1;
            while y >= min_y {
                let section = ((y - min_y) / 16) as usize;
                let sub_y = ((y - min_y) % 16) as u8;
                let id = chunk
                    .get_block(lx, y, lz)
                    .with_context(|| format!("column y={y} outside chunk geometry"))?;
                let state = registry.by_id(id).with_context(|| {
                    format!("chunk ({cx},{cz}) references unregistered state {id:?}")
                })?;
                let biome = chunk
                    .biomes
                    .get(section)
                    .context("chunk biome sections shorter than block sections")?
                    .get(lx >> 2, sub_y >> 2, lz >> 2);
                let cell = (usize::from(sub_y) * 16 + usize::from(lz)) * 16 + usize::from(lx);
                let (sky, block) = chunk
                    .section_lights
                    .get(section)
                    .map(|layers| {
                        (
                            layers.sky.as_ref().map(|l| l.get(cell)).unwrap_or(0),
                            layers.block.as_ref().map(|l| l.get(cell)).unwrap_or(0),
                        )
                    })
                    .unwrap_or((0, 0));

                let extend = match &open {
                    Some((run, _)) => {
                        run.state == id
                            && run.biome == biome.as_str()
                            && run.sky == sky
                            && run.block == block
                    }
                    None => false,
                };
                if extend {
                    let (run, _) = open.as_mut().expect("matched open run");
                    if run.height == MAX_RUN_LEN {
                        flush(&mut runs, &open);
                        let map = intern(&mut mapping, &mut map_index, project, state, biome, id);
                        open = Some((
                            Open {
                                state: id,
                                biome: biome.as_str(),
                                sky,
                                block,
                                bottom: y,
                                height: 1,
                            },
                            map,
                        ));
                    } else {
                        run.height += 1;
                        run.bottom = y;
                    }
                } else {
                    flush(&mut runs, &open);
                    let map = intern(&mut mapping, &mut map_index, project, state, biome, id);
                    open = Some((
                        Open {
                            state: id,
                            biome: biome.as_str(),
                            sky,
                            block,
                            bottom: y,
                            height: 1,
                        },
                        map,
                    ));
                }
                y -= 1;
            }
            flush(&mut runs, &open);
            debug_assert!(!runs.is_empty());
            columns.push(runs);
        }
    }

    Ok(DhColumns { columns, mapping })
}

/// Mapping-table intern keyed by (state id, biome text); builds the serial
/// only on first appearance so steady columns cost one lookup per cell.
fn intern<'a>(
    mapping: &mut Vec<String>,
    map_index: &mut HashMap<(BlockStateId, &'a str), u32>,
    project: &dyn DhBlockProjector,
    state: &BlockState,
    biome: &'a Identifier,
    id: BlockStateId,
) -> u32 {
    if let Some(map) = map_index.get(&(id, biome.as_str())) {
        return *map;
    }
    let serial = project
        .project(state, biome)
        .unwrap_or_else(|| canonical_serial(state, biome));
    let map = mapping.len() as u32;
    mapping.push(serial);
    map_index.insert((id, biome.as_str()), map);
    map
}

/// Envelope + compression: format-1 main blob, zero edges, LIGHT steps,
/// MERGE world compression, mapping table, flags and timestamps.
pub(crate) fn encode_columns(position: i64, cols: &DhColumns, modified: i64) -> Result<Vec<u8>> {
    debug_assert_eq!(cols.columns.len(), 4096);
    let mut main_raw = Vec::with_capacity(4096 * 16);
    for column in &cols.columns {
        let count = i16::try_from(column.len()).context("dh column exceeds i16 run count")?;
        main_raw.extend_from_slice(&count.to_be_bytes());
        for run in column {
            main_raw.extend_from_slice(&run.to_be_bytes());
        }
    }
    let main = zstd::bulk::compress(&main_raw, ZSTD_LEVEL).context("compress dh main blob")?;
    let steps = zstd::bulk::compress(&[GEN_STEP_LIGHT; 4096], ZSTD_LEVEL)
        .context("compress dh generation steps")?;
    let world = zstd::bulk::compress(&[WORLD_COMPRESSION_MERGE; 4096], ZSTD_LEVEL)
        .context("compress dh world compression")?;
    let mut mapping_raw = Vec::with_capacity(1024);
    mapping_raw.extend_from_slice(&(cols.mapping.len() as i32).to_be_bytes());
    for entry in &cols.mapping {
        write_java_utf(&mut mapping_raw, entry)?;
    }
    let mapping =
        zstd::bulk::compress(&mapping_raw, ZSTD_LEVEL).context("compress dh mapping blob")?;

    // Envelope order: main, north/south/east/west edges (zero-length for
    // format 1), generation steps, world compression, mapping.
    let empty: &[u8] = &[];
    let mut out = Vec::with_capacity(main.len() + mapping.len() + 4096);
    out.extend_from_slice(&position.to_be_bytes());
    out.extend_from_slice(&0_i32.to_be_bytes());
    for blob in [
        main.as_slice(),
        empty,
        empty,
        empty,
        empty,
        steps.as_slice(),
        world.as_slice(),
        mapping.as_slice(),
    ] {
        out.extend_from_slice(
            &i32::try_from(blob.len())
                .context("dh blob too large")?
                .to_be_bytes(),
        );
        out.extend_from_slice(blob);
    }
    out.push(DATA_FORMAT_V1);
    out.push(COMPRESSION_ZSTD_BLOCK);
    out.extend_from_slice(&[1, 0, 0]);
    out.extend_from_slice(&modified.to_be_bytes());
    out.extend_from_slice(&modified.to_be_bytes());
    ensure_size(out.len())?;
    Ok(out)
}

/// Hard DTO byte budget on the finished envelope.
fn ensure_size(len: usize) -> Result<()> {
    if len > MAX_DTO_BYTES {
        bail!("dh dto is {len} bytes, over the {MAX_DTO_BYTES} budget");
    }
    Ok(())
}

#[cfg(test)]
#[path = "dto_tests.rs"]
mod dto_tests;
