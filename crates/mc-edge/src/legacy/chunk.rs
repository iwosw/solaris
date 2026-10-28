//! Chunk section rewriting: block-state and biome ids inside paletted
//! containers (`LevelChunkWithLight.data`, `ChunksBiomes`).
//!
//! Single-value and indirect containers only remap their palette; direct
//! (global) containers are repacked at the legacy client's global bit width,
//! which the client derives from its own registry size, not from the packet.

use anyhow::{Result, bail, ensure};
use bytes::Buf;
use mc_protocol::codec::{ReadMc, WriteMc};

use super::tables::IdMap;

const SECTION_BLOCKS: usize = 4096;
const SECTION_BIOMES: usize = 64;
/// Highest indirect bit width per container kind (above it: global palette).
const MAX_INDIRECT_BLOCK_BITS: u8 = 8;
const MAX_INDIRECT_BIOME_BITS: u8 = 3;

/// One container kind's legacy mapping.
pub struct Container<'a> {
    pub ids: &'a IdMap,
    /// Fallback legacy id for a canonical id with no mapping.
    pub fallback: i32,
    pub legacy_global_bits: u8,
    volume: usize,
    max_indirect_bits: u8,
}

impl<'a> Container<'a> {
    pub fn blocks(ids: &'a IdMap, legacy_global_bits: u8) -> Self {
        Self {
            ids,
            fallback: 1,
            legacy_global_bits,
            volume: SECTION_BLOCKS,
            max_indirect_bits: MAX_INDIRECT_BLOCK_BITS,
        }
    }

    pub fn biomes(ids: &'a IdMap, legacy_global_bits: u8) -> Self {
        Self {
            ids,
            fallback: 0,
            legacy_global_bits,
            volume: SECTION_BIOMES,
            max_indirect_bits: MAX_INDIRECT_BIOME_BITS,
        }
    }

    fn map(&self, id: i32) -> i32 {
        self.ids.get(id).unwrap_or(self.fallback)
    }

    /// Copy one container from `input` to `out` with ids remapped.
    pub fn rewrite(&self, input: &mut &[u8], out: &mut Vec<u8>) -> Result<()> {
        let bits = input.read_u8()?;
        if bits == 0 {
            out.write_u8(0);
            out.write_varint(self.map(input.read_varint()?));
            return Ok(());
        }
        if bits <= self.max_indirect_bits {
            out.write_u8(bits);
            let len = input.read_varint()?;
            ensure!((0..=4096).contains(&len), "palette length {len}");
            out.write_varint(len);
            for _ in 0..len {
                out.write_varint(self.map(input.read_varint()?));
            }
            let words = long_count(bits, self.volume);
            ensure!(input.remaining() >= words * 8, "truncated palette data");
            out.extend_from_slice(&input[..words * 8]);
            input.advance(words * 8);
            return Ok(());
        }
        ensure!(bits <= 32, "global palette bit width {bits}");
        let words = long_count(bits, self.volume);
        ensure!(
            input.remaining() >= words * 8,
            "truncated global palette data"
        );
        let mut values = Vec::with_capacity(self.volume);
        let per_word = 64 / usize::from(bits);
        let mask = (1u64 << bits) - 1;
        for word in 0..words {
            let long = input.read_u64()?;
            for slot in 0..per_word {
                if values.len() == self.volume {
                    break;
                }
                let raw = (long >> (slot * usize::from(bits))) & mask;
                values.push(self.map(raw as i32) as u64);
            }
            if word + 1 == words && values.len() != self.volume {
                bail!(
                    "global palette holds {} of {} entries",
                    values.len(),
                    self.volume
                );
            }
        }
        let bits = self.legacy_global_bits;
        out.write_u8(bits);
        let per_word = 64 / usize::from(bits);
        for chunk in values.chunks(per_word) {
            let mut long = 0u64;
            for (slot, value) in chunk.iter().enumerate() {
                long |= value << (slot * usize::from(bits));
            }
            out.write_u64(long);
        }
        Ok(())
    }
}

fn long_count(bits: u8, volume: usize) -> usize {
    volume.div_ceil(64 / usize::from(bits))
}

/// Rewrite a `LevelChunkWithLight.data` blob: every section's block and biome
/// container (the two i16 counters are copied).
pub fn rewrite_sections(
    mut input: &[u8],
    blocks: &Container<'_>,
    biomes: &Container<'_>,
    out: &mut Vec<u8>,
) -> Result<()> {
    while !input.is_empty() {
        ensure!(input.len() >= 4, "truncated section header");
        out.extend_from_slice(&input[..4]);
        input.advance(4);
        blocks.rewrite(&mut input, out)?;
        biomes.rewrite(&mut input, out)?;
    }
    Ok(())
}

/// Rewrite a `ChunksBiomes` entry blob: biome containers only.
pub fn rewrite_biome_sections(
    mut input: &[u8],
    biomes: &Container<'_>,
    out: &mut Vec<u8>,
) -> Result<()> {
    while !input.is_empty() {
        biomes.rewrite(&mut input, out)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "chunk_tests.rs"]
mod tests;
