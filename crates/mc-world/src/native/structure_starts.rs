use super::{NativeCheckpointReader, NativeError, codec, db};
use crate::{ChunkPos, chunk::GeneratedStructureSite};
use mc_nbt::Tag;


impl NativeCheckpointReader {
    /// Read coverage and start metadata from one pinned snapshot, without block
    /// palette expansion, procedural generation or mutable world admission.
    /// Includes covered chunks with no starts, which suppress planned sites.
    pub fn structure_starts_in_region(
        &self,
        dimension: u32,
        min: (i32, i32),
        max: (i32, i32),
        skip_referenced: bool,
    ) -> Result<Vec<(ChunkPos, Vec<GeneratedStructureSite>)>, NativeError> {
        let metadata = db(self.tx.open_table(super::METADATA))?;
        let imported = db(self.tx.open_table(super::IMPORTED))?;
        let mut result = Vec::new();
        self.visit_chunk_positions(|dim, pos| {
            if dim != dimension || pos.x < min.0 || pos.z < min.1 || pos.x > max.0 || pos.z > max.1
            {
                return Ok(());
            }
            let key = super::chunk_key(dim, pos);
            let bytes = match db(metadata.get(key.as_slice()))? {
                Some(bytes) => Some(bytes),
                None => db(imported.get(key.as_slice()))?,
            };
            let starts = if let Some(bytes) = bytes {
                let root = codec::decode(bytes.value())?;
                let Tag::Compound(fields) = root else {
                    return Err(NativeError::Invalid("structure metadata root"));
                };
                crate::structure_starts::read(&fields, pos, skip_referenced)
                    .map_err(NativeError::Source)?
            } else {
                Vec::new()
            };
            result.push((pos, starts));
            Ok(())
        })?;
        Ok(result)
    }
}
