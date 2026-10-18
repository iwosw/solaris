use super::*;
use crate::dimensions::Dimension;
use crate::registry::CoreRegistry;
use mc_data::Identifier;

#[test]
fn malformed_falling_import_reports_location_and_leaves_region_bytes_unchanged() {
    let registry = CoreRegistry::load(None).unwrap();
    let uuid = uuid::Uuid::from_u128(0xfa11);
    let type_id = registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:falling_block").unwrap())
        .unwrap();
    for (name, block, expected) in [
        (
            "nongravity",
            "minecraft:stone",
            "BlockState.Name: not a gravity block",
        ),
        (
            "unknown-property",
            "minecraft:sand",
            "BlockState.Properties.unknown",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir(&source).unwrap();
        let mut manifest = crate::journal::CheckpointManifest {
            domain_versions: crate::native_checkpoint::domain_versions(),
            world_spawn: Some([0, 80, 0]),
            ..Default::default()
        };
        for dimension in [Dimension::Nether, Dimension::End] {
            manifest.dimensions.insert(
                dimension,
                Box::new(crate::journal::CheckpointManifest {
                    domain_versions: crate::native_checkpoint::domain_versions(),
                    world_spawn: Some([0, 80, 0]),
                    ..Default::default()
                }),
            );
        }
        let mut metadata = crate::level_metadata::MetadataTags::new();
        crate::level_metadata::checkpoint_tags(&mut metadata, 42, &manifest).unwrap();
        for (path, tag) in metadata {
            write_nbt(&source.join(path), &tag).unwrap();
        }
        let mut fields = vec![("Name".into(), Tag::String(block.into()))];
        if name == "unknown-property" {
            fields.push((
                "Properties".into(),
                Tag::Compound(vec![("unknown".into(), Tag::String("bad".into()))]),
            ));
        }
        let malformed = mc_data::entity_nbt::VanillaEntity {
            id: "minecraft:falling_block".into(),
            uuid,
            wire_id: 8,
            pos: [-520.5, 75.0, 1040.5],
            extra_fields: vec![("BlockState".into(), Tag::Compound(fields))],
            ..Default::default()
        };
        let chunks = BTreeMap::from([(
            (-33, 65),
            mc_world::anvil::EntityChunk {
                x: -33,
                z: 65,
                entities: vec![
                    mc_data::entity_nbt::VanillaEntity {
                        id: "minecraft:cow".into(),
                        uuid: uuid::Uuid::from_u128(7),
                        wire_id: 7,
                        pos: [-520.5, 75.0, 1040.5],
                        ..Default::default()
                    },
                    malformed,
                ],
            },
        )]);
        let regions = Dimension::Overworld.directory(&source).join("entities");
        mc_world::anvil::write_entity_regions(&regions, &registry.items, &chunks, 1).unwrap();
        let path = regions.join("r.-2.2.mca");
        let before = fs::read(&path).unwrap();
        let source_hashes = hashes(&source).unwrap();
        let destination = dir.path().join("native");
        let receipt_path = dir.path().join("receipt.json");
        let error = import_anvil(
            &CoreConfig::new(source.clone(), 42),
            &source,
            &destination,
            &receipt_path,
        )
        .unwrap_err()
        .to_string();
        let receipt: ConversionReceipt =
            serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
        assert_eq!(receipt.status, "FAIL");
        for detail in [
            path.display().to_string(),
            "chunk (-33, 65)".into(),
            "entity-index 1".into(),
            format!("UUID {uuid}"),
            format!("type-id {type_id} (minecraft:falling_block)"),
            expected.into(),
        ] {
            assert!(
                error.contains(&detail),
                "{name}: missing {detail:?} in {error}"
            );
            assert!(
                receipt.error.as_ref().unwrap().contains(&detail),
                "receipt missing {detail}"
            );
        }
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "failed import cannot rewrite custody"
        );
        assert_eq!(hashes(&source).unwrap(), source_hashes);
        assert!(!destination.exists());
    }
}
