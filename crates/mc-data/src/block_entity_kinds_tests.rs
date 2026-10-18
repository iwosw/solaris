use super::*;
use crate::Identifier;
use crate::block_entity_types::{
    load_block_entity_types_report, solaris_required_block_entity_types,
};
use std::collections::BTreeSet;

#[test]
fn every_native_kind_matches_the_26_1_2_registry_exactly() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/reports/registries.json");
    let report = load_block_entity_types_report(path).unwrap();
    let registry = solaris_required_block_entity_types();
    assert_eq!(BlockEntityKind::ALL.len(), report.len());
    assert_eq!(registry.len(), report.len());
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for kind in BlockEntityKind::ALL {
        let name = Identifier::parse(kind.vanilla_id()).unwrap();
        let entry = report.iter().find(|entry| entry.id == name).unwrap();
        assert_eq!(kind.protocol_id(), entry.protocol_id, "{name}");
        assert_eq!(registry.id_of(&name), Some(entry.protocol_id), "{name}");
        assert!(ids.insert(kind.protocol_id()));
        assert!(names.insert(name));
    }
}

#[test]
fn storage_blocks_use_native_kinds_not_inventory_size() {
    for (path, kind) in [
        ("furnace", BlockEntityKind::Furnace),
        ("smoker", BlockEntityKind::Smoker),
        ("blast_furnace", BlockEntityKind::BlastFurnace),
        ("chest", BlockEntityKind::Chest),
        ("trapped_chest", BlockEntityKind::TrappedChest),
        ("barrel", BlockEntityKind::Barrel),
        ("hopper", BlockEntityKind::Hopper),
        ("dispenser", BlockEntityKind::Dispenser),
        ("dropper", BlockEntityKind::Dropper),
        ("brewing_stand", BlockEntityKind::BrewingStand),
        ("crafter", BlockEntityKind::Crafter),
    ] {
        assert_eq!(BlockEntityKind::for_block(path), Some(kind), "{path}");
    }
    for wood in [
        "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "bamboo",
        "pale_oak", "crimson", "warped",
    ] {
        let path = format!("{wood}_shelf");
        assert_eq!(
            BlockEntityKind::for_block(&path),
            Some(BlockEntityKind::Shelf)
        );
    }
    for path in [
        "copper_chest",
        "exposed_copper_chest",
        "weathered_copper_chest",
        "oxidized_copper_chest",
        "waxed_copper_chest",
        "waxed_exposed_copper_chest",
        "waxed_weathered_copper_chest",
        "waxed_oxidized_copper_chest",
    ] {
        assert_eq!(
            BlockEntityKind::for_block(path),
            Some(BlockEntityKind::Chest)
        );
    }
    assert_eq!(BlockEntityKind::for_block("stone"), None);
}

#[test]
fn all_201_native_block_aliases_have_one_owner() {
    let mut blocks = BTreeSet::new();
    for kind in BlockEntityKind::ALL {
        for path in kind.block_paths() {
            assert!(blocks.insert(*path), "duplicate owner for {path}");
            assert_eq!(BlockEntityKind::for_block(path), Some(*kind));
        }
    }
    assert_eq!(blocks.len(), 201, "BlockEntityType registration, 26.1.2");
    assert_eq!(BlockEntityKind::ShulkerBox.block_paths().len(), 17);
    assert_eq!(BlockEntityKind::Sign.block_paths().len(), 24);
    assert_eq!(BlockEntityKind::HangingSign.block_paths().len(), 24);
    assert_eq!(BlockEntityKind::Banner.block_paths().len(), 32);
}
