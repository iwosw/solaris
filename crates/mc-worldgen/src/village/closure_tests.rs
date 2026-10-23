use super::*;

#[test]
fn absent_jigsaw_template_has_vanilla_empty_semantics() {
    let root = tempfile::tempdir().unwrap();
    let registry = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap();
    let id = Identifier::parse("minecraft:missing_piece").unwrap();
    let template = load_piece(root.path(), &id, &id, &registry).unwrap();
    assert_eq!(template.size(), [0; 3]);
    assert!(template.blocks().is_empty());
    assert!(template.jigsaws().is_empty());
    assert!(template.entities().is_empty());
    assert!(template.block_entities().is_empty());
    assert!(template.chests().is_empty());
}

#[test]
fn malformed_jigsaw_template_still_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("data/minecraft/structure");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("malformed.nbt"), b"invalid NBT").unwrap();
    let registry = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap();
    let id = Identifier::parse("minecraft:malformed").unwrap();
    assert!(matches!(
        load_piece(root.path(), &id, &id, &registry),
        Err(ClosureError::Template(crate::structures::StructureError::Nbt { .. }))
    ));
}
