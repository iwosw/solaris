//! Loader for the vanilla block-entity type registry slice of `registries.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::Identifier;
use crate::sidecar::RawRegistries;

#[derive(Debug, Error)]
pub enum BlockEntityTypesReportError {
    #[error("registries.json not found at {0}")]
    Missing(PathBuf),
    #[error("registries.json io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("registries.json parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("registries.json does not contain minecraft:block_entity_type")]
    MissingBlockEntityTypeRegistry,
    #[error("invalid block entity type identifier {0:?} in registries.json")]
    InvalidIdentifier(String),
}

pub fn load_block_entity_types_report(
    path: impl AsRef<Path>,
) -> Result<Vec<BlockEntityTypeReport>, BlockEntityTypesReportError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(BlockEntityTypesReportError::Missing(path.to_path_buf()));
    }
    let bytes = crate::sidecar::read_file(path)?;
    let raw: RawRegistries = serde_json::from_slice(&bytes)?;
    let entries = raw
        .registries
        .get("minecraft:block_entity_type")
        .ok_or(BlockEntityTypesReportError::MissingBlockEntityTypeRegistry)?;
    entries
        .entries
        .iter()
        .map(|(name, body)| {
            let id = Identifier::parse(name.clone())
                .map_err(|_| BlockEntityTypesReportError::InvalidIdentifier(name.clone()))?;
            Ok(BlockEntityTypeReport {
                id,
                protocol_id: body.protocol_id,
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct BlockEntityTypeReport {
    pub id: Identifier,
    pub protocol_id: u32,
}

#[derive(Debug, Clone, Default)]
pub struct BlockEntityTypeRegistry {
    by_name: BTreeMap<Identifier, u32>,
}

impl BlockEntityTypeRegistry {
    #[must_use]
    pub fn from_report(report: &[BlockEntityTypeReport]) -> Self {
        let by_name = report
            .iter()
            .map(|entry| (entry.id.clone(), entry.protocol_id))
            .collect();
        Self { by_name }
    }

    #[must_use]
    pub fn id_of(&self, name: &Identifier) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

#[must_use]
pub fn solaris_required_block_entity_types() -> BlockEntityTypeRegistry {
    let by_name = crate::block_entity_kinds::BlockEntityKind::ALL
        .iter()
        .map(|kind| {
            (
                Identifier::parse(kind.vanilla_id()).expect("static block entity type id is valid"),
                kind.protocol_id(),
            )
        })
        .collect();
    BlockEntityTypeRegistry { by_name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_block_entity_type_ids_cover_runtime_emitters() {
        let registry = solaris_required_block_entity_types();
        for id in [
            "minecraft:furnace",
            "minecraft:chest",
            "minecraft:barrel",
            "minecraft:smoker",
            "minecraft:blast_furnace",
            "minecraft:campfire",
            "minecraft:sign",
        ] {
            assert!(registry.id_of(&Identifier::parse(id).unwrap()).is_some());
        }
    }

    #[test]
    fn bed_has_no_block_entity_type_in_26_3() {
        let registry = solaris_required_block_entity_types();
        assert_eq!(
            registry.id_of(&Identifier::parse("minecraft:bed").unwrap()),
            None,
            "26.3 registries.json block_entity_type drops minecraft:bed"
        );
    }

    #[test]
    fn shrieker_block_entity_type_matches_vanilla_protocol_id() {
        let registry = solaris_required_block_entity_types();
        assert_eq!(
            registry.id_of(&Identifier::parse("minecraft:sculk_shrieker").unwrap()),
            Some(37)
        );
    }
}
