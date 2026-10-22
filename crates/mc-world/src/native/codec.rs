use std::collections::BTreeMap;
use std::io::Write;

use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use mc_data::Identifier;
use mc_data::items::ItemRegistry;
use mc_nbt::{Tag, tag_type};

use crate::anvil::chunk_nbt::{chunk_metadata_to_nbt_with_items_at_tick, encode_block_section};
use crate::{BlockRegistry, Chunk, ChunkSection, SECTION_VOLUME};

use super::NativeError;
use super::dictionary::{Resolve, Writer};

const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
const SPARSE: u8 = 1;
const DENSE: u8 = 2;

pub(super) fn encode(tag: &Tag) -> Result<Vec<u8>, NativeError> {
    super::checkpoint_validation::nbt(tag)?;
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", tag)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(NativeError::Invalid("native record exceeds byte budget"));
    }
    Ok(bytes)
}

pub(super) fn decode(bytes: &[u8]) -> Result<Tag, NativeError> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(NativeError::Invalid("native record exceeds byte budget"));
    }
    let plain = super::pack::unpack(bytes, MAX_RECORD_BYTES)?;
    let mut cursor = plain.as_ref();
    let (_, tag) = mc_nbt::read_named(&mut cursor)?;
    if !cursor.is_empty() {
        return Err(NativeError::Invalid("trailing native NBT bytes"));
    }
    super::checkpoint_validation::nbt(&tag)?;
    Ok(tag)
}

pub(super) fn compound_mut(tag: &mut Tag) -> Result<&mut Vec<(String, Tag)>, NativeError> {
    match tag {
        Tag::Compound(fields) => Ok(fields),
        _ => Err(NativeError::Invalid("expected compound")),
    }
}

pub(super) fn sections_mut(tag: &mut Tag) -> Result<&mut Vec<Tag>, NativeError> {
    match compound_mut(tag)?
        .iter_mut()
        .find(|(key, _)| key == "sections")
    {
        Some((_, Tag::List(list))) if list.element_type == tag_type::COMPOUND => {
            Ok(&mut list.elements)
        }
        _ => Err(NativeError::Invalid("missing sections")),
    }
}

pub(super) fn section_y(tag: &Tag) -> Result<i32, NativeError> {
    let Tag::Compound(fields) = tag else {
        return Err(NativeError::Invalid("invalid section compound"));
    };
    match fields.iter().find(|(key, _)| key == "Y") {
        Some((_, Tag::Byte(y))) => Ok(i32::from(*y)),
        _ => Err(NativeError::Invalid("invalid section Y")),
    }
}

/// Uses the existing side-state codec: inventories, typed/opaque block
/// entities, ticks, initialization markers, biomes, light and root extras.
/// It neither encodes nor stores procedural block payload.
pub(super) fn metadata(
    chunk: &Chunk,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
) -> Result<Vec<u8>, NativeError> {
    super::pack::pack(encode(&metadata_root(chunk, blocks, items)?)?)
}

pub(super) fn metadata_root(
    chunk: &Chunk,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
) -> Result<Tag, NativeError> {
    // Native metadata is the owner after-image, not Anvil's flush-time LastUpdate.
    // Absolute scheduled deadlines likewise do not depend on when IO executes.
    let mut root = chunk_metadata_to_nbt_with_items_at_tick(chunk, blocks, Some(items), 0)?;
    super::ticks::append(&mut root, chunk)?;
    Ok(root)
}

pub(super) fn metadata_at_lsn(
    chunk: &Chunk,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    lsn: u64,
) -> Result<Vec<u8>, NativeError> {
    let mut root = metadata_root(chunk, blocks, items)?;
    let fields = compound_mut(&mut root)?;
    fields.retain(|(key, _)| key != "SolarisJournalLsn");
    fields.push(("SolarisJournalLsn".into(), Tag::Long(lsn as i64)));
    super::pack::pack(encode(&root)?)
}

/// Full immutable base assembled without Anvil relative-delay tick encoding.
pub(super) fn baseline(
    chunk: &Chunk,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
) -> Result<Vec<u8>, NativeError> {
    let mut root = metadata_root(chunk, blocks, items)?;
    let min_section = chunk.geometry().min_y() / 16;
    for section in sections_mut(&mut root)? {
        let index = section_y(section)? - min_section;
        if let Ok(index) = usize::try_from(index)
            && let Some(blocks_section) = chunk.sections.get(index)
        {
            compound_mut(section)?.push((
                "block_states".into(),
                encode_block_section(blocks_section, blocks)?,
            ));
        }
    }
    super::pack::pack(encode(&root)?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Representation {
    Sparse,
    Dense,
}

pub(super) fn representation(bytes: &[u8]) -> Result<Representation, NativeError> {
    match bytes.first() {
        Some(&SPARSE) => Ok(Representation::Sparse),
        Some(&DENSE) => Ok(Representation::Dense),
        _ => Err(NativeError::Invalid("unknown section representation")),
    }
}

/// New IDs and the returned overlay must be committed in the same transaction.
/// Side metadata remains in its existing separate codec, irrespective of mode.
pub(super) fn overlay(
    base: &ChunkSection,
    effective: &ChunkSection,
    blocks: &BlockRegistry,
    dictionary: &mut Writer<'_>,
    previous: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, NativeError> {
    let mut changed = Vec::new();
    let mut states = Vec::with_capacity(SECTION_VOLUME);
    let mut palette = Vec::new();
    let mut by_state = BTreeMap::new();
    for index in 0..SECTION_VOLUME {
        let (x, y, z) = coordinates(index);
        let state = effective.get(x, y, z);
        let palette_index = match by_state.get(&state) {
            Some(index) => *index,
            None => {
                let index = palette.len() as u16;
                palette.push(state);
                by_state.insert(state, index);
                index
            }
        };
        states.push(palette_index);
        if state != base.get(x, y, z) {
            changed.push((index as u16, palette_index));
        }
    }
    if changed.is_empty() {
        return Ok(None);
    }
    let previous = previous.map(representation).transpose()?;
    let air = blocks
        .block(&Identifier::parse("minecraft:air").expect("static identifier"))
        .ok_or(NativeError::Invalid("air missing from registry"))?
        .default;
    let all_air = palette.as_slice() == [air];
    let ids = palette
        .iter()
        .map(|state| dictionary.intern(*state, blocks))
        .collect::<Result<Vec<_>, _>>()?;
    let mut dense = Vec::new();
    append_palette(&mut dense, &ids);
    if ids.len() > 1 {
        let bits = palette_bits(ids.len());
        let per_word = 64 / bits;
        for indices in states.chunks(per_word) {
            let mut word = 0u64;
            for (offset, index) in indices.iter().enumerate() {
                word |= u64::from(*index) << (offset * bits);
            }
            dense.extend_from_slice(&word.to_be_bytes());
        }
    }
    let dense = compress(DENSE, &dense)?;
    // AllAir is always a replacement, never an absent terrain record.
    if all_air {
        return Ok(Some(dense));
    }
    let mut sparse_palette = Vec::new();
    let mut sparse_indices = BTreeMap::new();
    let mut sparse = Vec::with_capacity(2 + changed.len() * 4);
    sparse.extend_from_slice(&(changed.len() as u16).to_be_bytes());
    for (index, state) in changed {
        let palette_index = *sparse_indices.entry(state).or_insert_with(|| {
            let next = sparse_palette.len() as u16;
            sparse_palette.push(ids[usize::from(state)]);
            next
        });
        sparse.extend_from_slice(&index.to_be_bytes());
        sparse.extend_from_slice(&palette_index.to_be_bytes());
    }
    append_palette(&mut sparse, &sparse_palette);
    let sparse = compress(SPARSE, &sparse)?;
    Ok(Some(
        match select_representation(sparse.len(), dense.len(), previous) {
            Representation::Sparse => sparse,
            Representation::Dense => dense,
        },
    ))
}

/// Compare complete compressed records, including tag and zlib checksum.
/// Switch only when saving at least 32 bytes AND 5% of the prior mode's size.
/// This modest hysteresis is storage tuning, not part of WorldIdentity.
fn select_representation(
    sparse: usize,
    dense: usize,
    previous: Option<Representation>,
) -> Representation {
    let worthwhile = |old: usize, new: usize| old.saturating_sub(new) >= 32.max(old.div_ceil(20));
    match previous {
        Some(Representation::Sparse) if !worthwhile(sparse, dense) => Representation::Sparse,
        Some(Representation::Dense) if !worthwhile(dense, sparse) => Representation::Dense,
        _ if sparse <= dense => Representation::Sparse,
        _ => Representation::Dense,
    }
}

fn append_palette(bytes: &mut Vec<u8>, ids: &[u32]) {
    bytes.extend_from_slice(&(ids.len() as u16).to_be_bytes());
    for id in ids {
        bytes.extend_from_slice(&id.to_be_bytes());
    }
}

fn palette_bits(count: usize) -> usize {
    (usize::BITS - (count - 1).leading_zeros()) as usize
}

fn compress(mode: u8, bytes: &[u8]) -> Result<Vec<u8>, NativeError> {
    let mut header = vec![mode];
    header.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    let mut encoder = ZlibEncoder::new(header, Compression::fast());
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}

// Maximum sparse body: count, 4096 index/reference pairs, palette count,
// 4096 native IDs. Dense packed bodies are smaller.
const MAX_OVERLAY_BODY: usize = 4 + SECTION_VOLUME * 8;

fn decompress(bytes: &[u8]) -> Result<Vec<u8>, NativeError> {
    if bytes.len() < 6 || bytes.len() > MAX_OVERLAY_BODY + 1024 {
        return Err(NativeError::Invalid("invalid compressed overlay size"));
    }
    let length = u32::from_be_bytes(bytes[1..5].try_into().expect("length")) as usize;
    if length > MAX_OVERLAY_BODY {
        return Err(NativeError::Invalid("oversized overlay body"));
    }
    let mut body = vec![0; length];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(&bytes[5..], &mut body, FlushDecompress::Finish)
        .map_err(|_| NativeError::Invalid("invalid compressed overlay"))?;
    if status != Status::StreamEnd
        || decoder.total_in() != (bytes.len() - 5) as u64
        || decoder.total_out() != length as u64
    {
        return Err(NativeError::Invalid(
            "truncated or trailing compressed overlay",
        ));
    }
    Ok(body)
}

pub(super) fn compose_section(
    base: &ChunkSection,
    bytes: &[u8],
    blocks: &BlockRegistry,
    dictionary: &impl Resolve,
) -> Result<Tag, NativeError> {
    let mode = representation(bytes)?;
    let air = blocks
        .block(&Identifier::parse("minecraft:air").expect("static identifier"))
        .ok_or(NativeError::Invalid("air missing from registry"))?
        .default;
    let mut section = match mode {
        Representation::Sparse => base.clone(),
        Representation::Dense => ChunkSection::filled(air, air),
    };
    visit_overlay(
        bytes,
        &mut |id| dictionary.resolve(id, blocks),
        |index, count, state| {
            if mode == Representation::Dense && index == 0 {
                section = ChunkSection::filled(state, air);
                return;
            }
            debug_assert_eq!(count, 1);
            let (x, y, z) = coordinates(index);
            section.set(x, y, z, state);
        },
    )?;
    Ok(encode_block_section(&section, blocks)?)
}

/// Inspect native references without interpreting runtime IDs. Sparse cells not
/// mentioned in the record are baseline cells. Validate the entire record,
/// including cells after the requested one, before returning an origin.
pub(super) fn inspect_state(bytes: &[u8], index: usize) -> Result<Option<(u8, u32)>, NativeError> {
    if index >= SECTION_VOLUME {
        return Err(NativeError::Invalid("invalid inspected local index"));
    }
    let mut state = None;
    visit_overlay(bytes, &mut Ok, |start, count, id| {
        if index >= start && index - start < count {
            state = Some(id);
        }
    })?;
    Ok(state.map(|id| (bytes[0], id)))
}

// Resolve each palette entry once. A single-state dense record is emitted as
// one span, preserving the O(1) AllAir/full-uniform decode path.
fn visit_overlay<T: Copy>(
    bytes: &[u8],
    resolve: &mut impl FnMut(u32) -> Result<T, NativeError>,
    mut visit: impl FnMut(usize, usize, T),
) -> Result<(), NativeError> {
    let mode = representation(bytes)?;
    let body = decompress(bytes)?;
    let mut cursor = body.as_slice();
    match mode {
        Representation::Sparse => {
            let count = usize::from(take_u16(&mut cursor)?);
            if count == 0 || count > SECTION_VOLUME || cursor.len() < count * 4 {
                return Err(NativeError::Invalid("invalid sparse count"));
            }
            let (entries, rest) = cursor.split_at(count * 4);
            cursor = rest;
            let palette = read_palette(&mut cursor, count, resolve)?;
            if !cursor.is_empty() {
                return Err(NativeError::Invalid("trailing sparse bytes"));
            }
            let mut previous = None;
            for entry in entries.as_chunks::<4>().0 {
                let index = usize::from(u16::from_be_bytes([entry[0], entry[1]]));
                let state = usize::from(u16::from_be_bytes([entry[2], entry[3]]));
                if index >= SECTION_VOLUME || previous.is_some_and(|last| index <= last) {
                    return Err(NativeError::Invalid("unsorted or invalid sparse index"));
                }
                let state = *palette
                    .get(state)
                    .ok_or(NativeError::Invalid("sparse palette index out of bounds"))?;
                visit(index, 1, state);
                previous = Some(index);
            }
        }
        Representation::Dense => {
            let palette = read_palette(&mut cursor, SECTION_VOLUME, resolve)?;
            if palette.len() == 1 {
                if !cursor.is_empty() {
                    return Err(NativeError::Invalid("trailing single-state dense bytes"));
                }
                visit(0, SECTION_VOLUME, palette[0]);
            } else {
                let bits = palette_bits(palette.len());
                let per_word = 64 / bits;
                if cursor.len() != SECTION_VOLUME.div_ceil(per_word) * 8 {
                    return Err(NativeError::Invalid("invalid dense packed length"));
                }
                let mask = (1u64 << bits) - 1;
                for (word_index, bytes) in cursor.as_chunks::<8>().0.iter().enumerate() {
                    let mut word = u64::from_be_bytes(*bytes);
                    let used = per_word.min(SECTION_VOLUME - word_index * per_word);
                    for offset in 0..used {
                        let state = *palette
                            .get((word & mask) as usize)
                            .ok_or(NativeError::Invalid("dense palette index out of bounds"))?;
                        visit(word_index * per_word + offset, 1, state);
                        word >>= bits;
                    }
                    if word != 0 {
                        return Err(NativeError::Invalid("nonzero dense padding"));
                    }
                }
            }
        }
    }
    Ok(())
}

fn read_palette<T: Copy>(
    cursor: &mut &[u8],
    maximum: usize,
    resolve: &mut impl FnMut(u32) -> Result<T, NativeError>,
) -> Result<Vec<T>, NativeError> {
    let count = usize::from(take_u16(cursor)?);
    if count == 0 || count > maximum || cursor.len() < count * 4 {
        return Err(NativeError::Invalid("invalid native palette length"));
    }
    let (entries, rest) = cursor.split_at(count * 4);
    *cursor = rest;
    let mut seen = std::collections::BTreeSet::new();
    entries
        .as_chunks::<4>()
        .0
        .iter()
        .map(|entry| {
            let id = u32::from_be_bytes(*entry);
            if !seen.insert(id) {
                return Err(NativeError::Invalid("duplicate native palette ID"));
            }
            resolve(id)
        })
        .collect()
}

fn take_u16(cursor: &mut &[u8]) -> Result<u16, NativeError> {
    if cursor.len() < 2 {
        return Err(NativeError::Invalid("truncated overlay"));
    }
    let value = u16::from_be_bytes([cursor[0], cursor[1]]);
    *cursor = &cursor[2..];
    Ok(value)
}

fn coordinates(index: usize) -> (u8, u8, u8) {
    (
        (index & 15) as u8,
        (index >> 8) as u8,
        ((index >> 4) & 15) as u8,
    )
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
