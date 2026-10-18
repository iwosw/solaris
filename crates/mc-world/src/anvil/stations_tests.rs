use super::*;
use mc_data::loot::chest_26_1_2::ChestLootCatalog;

#[test]
fn brewing_reload_preserves_slots_components_progress_and_fuel() {
    let items = mc_data::items::solaris_required_items();
    let blocks = BlockRegistry::from_report(&[mc_data::blocks::BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: Default::default(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: Default::default(),
        }],
    }])
    .unwrap();
    let pos = BlockPos { x: 2, y: 64, z: 3 };
    let mut stand = BrewingStandBlockEntity::default();
    stand.slots[0].count = 1;
    stand.slots[0].item_id = items
        .id_of(&Identifier::parse("minecraft:splash_potion").unwrap())
        .unwrap();
    stand.slots[0].custom_name = Some("Saved potion".into());
    stand.slots[0].extended_mut().potion = Some(Identifier::parse("minecraft:strength").unwrap());
    stand.slots[3].count = 3;
    stand.slots[3].item_id = items
        .id_of(&Identifier::parse("minecraft:dragon_breath").unwrap())
        .unwrap();
    stand.ingredient = Some(stand.slots[3].item_id);
    stand.brew_time = 213;
    stand.fuel = 17;
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.brewing_stands.insert(pos, stand.clone());
    let encoded = chunk_to_nbt_with_items(&chunk, &blocks, Some(&items)).unwrap();
    let decoded = chunk_from_nbt_with_items(
        &encoded,
        &blocks,
        Some(&items),
        &ChestLootCatalog::new(),
        crate::OVERWORLD_GEOMETRY,
    )
    .unwrap();
    assert_eq!(decoded.brewing_stands.get(&pos), Some(&stand));
    assert!(!decoded.block_entities.contains_key(&pos));
    let Tag::Compound(root) = encoded else {
        unreachable!()
    };
    let be = get_optional_list(&root, "block_entities").unwrap().unwrap();
    let fields = expect_compound(&be.elements[0], "block_entities[]").unwrap();
    assert_eq!(get_optional_short(fields, "BrewTime").unwrap(), Some(213));
    assert!(
        fields
            .iter()
            .any(|(name, tag)| name == "Fuel" && tag == &Tag::Byte(17))
    );
}
