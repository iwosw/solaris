use super::VillagerProfession;

#[test]
fn builder_profession_has_native_identity_and_mason_appearance() {
    let builder = VillagerProfession::from_name("solaris:builder")
        .expect("settlement builders have a native persisted profession");
    assert_eq!(builder.name(), "builder");
    assert_eq!(
        builder.profession_id(),
        VillagerProfession::Mason.profession_id()
    );
    assert_eq!(builder.title_name(), "mason");
    assert!(VillagerProfession::from_name("minecraft:builder").is_none());
}

#[test]
fn farmer_delivers_surplus_but_keeps_breeding_ration() {
    let (mut state, slot, mut v) =
        super::mining_tests::fixture(crate::settlements::buildings::BuildingKind::Mine);
    super::mining_tests::storage(&mut state, &v, [4, 201, 3]);
    v.data.profession = VillagerProfession::Farmer;
    let bread = super::mining::item_id(&state, "bread").unwrap();
    v.inventory[0] = Some(mc_data::ItemStack::new(bread, 7));
    assert!(super::harvest::deposit(&mut state, slot, &mut v).is_none());
    assert_eq!(
        v.inventory
            .iter()
            .flatten()
            .map(|stack| stack.count)
            .sum::<i32>(),
        3
    );
    assert_eq!(
        state.block_entities[&(4, 201, 3)]
            .slots
            .iter()
            .map(|stack| stack.count)
            .sum::<i32>(),
        4
    );
    assert!(super::harvest::deposit(&mut state, slot, &mut v).is_none());
    assert_eq!(
        state.block_entities[&(4, 201, 3)]
            .slots
            .iter()
            .map(|stack| stack.count)
            .sum::<i32>(),
        4
    );
}

#[test]
fn bound_farmer_keeps_raw_wheat_once_food_reserve_is_full() {
    let (mut state, slot, mut v) = super::mining_tests::fixture(
        crate::settlements::buildings::BuildingKind::Workshop(VillagerProfession::Farmer),
    );
    v.data.profession = VillagerProfession::Farmer;
    let bread = super::mining::item_id(&state, "bread").unwrap();
    let wheat = super::mining::item_id(&state, "wheat").unwrap();
    v.inventory[0] = Some(mc_data::ItemStack::new(bread, 3));
    v.inventory[1] = Some(mc_data::ItemStack::new(wheat, 2));
    let crop =
        crate::settlements::allocator::tests::block(&state, "minecraft:wheat", &[("age", "7")]);
    state.world.set_block(4, 201, 3, crop);
    let mut changes = crate::boundary::ChangeSet::default();
    for _ in 0..8 {
        super::harvest::work(&mut state, slot, &mut v, &mut changes);
        state.tick += 40;
    }
    assert_eq!(
        v.inventory
            .iter()
            .flatten()
            .filter(|stack| stack.item_id == bread)
            .map(|stack| stack.count)
            .sum::<i32>(),
        3
    );
    assert_eq!(
        v.inventory
            .iter()
            .flatten()
            .filter(|stack| stack.item_id == wheat)
            .map(|stack| stack.count)
            .sum::<i32>(),
        3
    );
    let seeds = super::mining::item_id(&state, "wheat_seeds").unwrap();
    assert_eq!(
        v.inventory
            .iter()
            .flatten()
            .filter(|stack| stack.item_id == seeds)
            .map(|stack| stack.count)
            .sum::<i32>(),
        1
    );
}

#[test]
fn farmer_refuses_harvest_if_net_seed_and_grain_do_not_both_fit() {
    let (mut state, slot, mut v) = super::mining_tests::fixture(
        crate::settlements::buildings::BuildingKind::Workshop(VillagerProfession::Farmer),
    );
    v.data.profession = VillagerProfession::Farmer;
    let cobble = super::mining::item_id(&state, "cobblestone").unwrap();
    let wheat = super::mining::item_id(&state, "wheat").unwrap();
    v.inventory = std::array::from_fn(|_| Some(mc_data::ItemStack::new(cobble, 64)));
    v.inventory[7] = Some(mc_data::ItemStack::new(wheat, 63));
    let before = v.inventory.clone();
    let crop =
        crate::settlements::allocator::tests::block(&state, "minecraft:wheat", &[("age", "7")]);
    state.world.set_block(4, 201, 3, crop);
    let mut changes = crate::boundary::ChangeSet::default();
    super::harvest::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(v.inventory, before);
    assert_eq!(state.world.block_at(4, 201, 3), Some(crop));
    assert!(changes.blocks.is_empty());
}
