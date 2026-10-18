use super::*;

#[test]
fn all_vanilla_wall_variants_share_the_standing_loot_table() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/data/minecraft/loot_table");
    let tables = crate::loot::load_vanilla_subset(root).unwrap();
    assert_eq!(PAIRS.len(), 61);
    for &(alias, source) in PAIRS {
        let alias = Identifier::parse(alias).unwrap();
        let source = Identifier::parse(source).unwrap();
        assert_eq!(canonical(&alias), &source);
        assert!(
            tables.block_loot(&alias).is_some(),
            "{alias}: absent loot table"
        );
        assert_eq!(
            tables.block_loot(&alias),
            tables.block_loot(&source),
            "{alias}"
        );
        assert_eq!(
            tables.block_random_sequence(&alias),
            tables.block_random_sequence(&source)
        );
    }
    for name in [
        "example:oak_wall_sign",
        "minecraft:unknown_wall_sign",
        "minecraft:stone",
    ] {
        let id = Identifier::parse(name).unwrap();
        assert_eq!(canonical(&id), &id);
    }
}
