use super::mining_tests::{fixture, storage};
use super::{VillagerProfession, VillagerState, mining, shepherd};
use crate::boundary::ChangeSet;
use crate::entities::EntitySlot;
use crate::settlements::buildings::BuildingKind;
use crate::tick::TickState;
use mc_data::ItemStack;

fn setup() -> (TickState, EntitySlot, Box<VillagerState>) {
    let (state, slot, mut v) = fixture(BuildingKind::Mine);
    v.data.profession = VillagerProfession::Shepherd;
    (state, slot, v)
}

fn sheep(state: &mut TickState, pos: [f64; 3], wool: u8) -> EntitySlot {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 8.0);
    let slot = state.entities.slot_of(id).unwrap();
    crate::tick::set_wool(
        std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
        wool,
    );
    slot
}

fn carried(v: &VillagerState, item: u32) -> i32 {
    v.inventory
        .iter()
        .flatten()
        .filter(|stack| stack.item_id == item)
        .map(|stack| stack.count)
        .sum()
}

#[test]
fn shepherd_shears_adult_sheep_for_matching_wool_once() {
    let (mut state, slot, mut v) = setup();
    let sheep_slot = sheep(&mut state, [3.5, 201.0, 3.5], 14);
    state.entities.set_pos(slot, [3.5, 201.0, 3.5]);
    let mut changes = ChangeSet::default();
    state.tick = 20;
    assert_eq!(shepherd::work(&mut state, slot, &mut v, &mut changes), None);
    let red = mining::item_id(&state, "red_wool").unwrap();
    let yield_count = carried(&v, red);
    assert!(
        (1..=3).contains(&yield_count),
        "vanilla 1..=3 colored yield, got {yield_count}"
    );
    assert_eq!(
        crate::tick::wool_of(&state.entities.row(sheep_slot).appearance),
        0x10 | 14,
        "color kept, sheared bit set"
    );
    assert!(
        state
            .appearance_changed
            .contains(&state.entities.body(sheep_slot).id),
        "shear journals the appearance change"
    );
    // A sheared sheep pays nothing until native grazing regrows the wool.
    state.tick = 120;
    assert_eq!(shepherd::work(&mut state, slot, &mut v, &mut changes), None);
    assert_eq!(carried(&v, red), yield_count);
    assert_eq!(
        crate::tick::wool_of(&state.entities.row(sheep_slot).appearance),
        0x10 | 14
    );
}

#[test]
fn shepherd_refuses_baby_foreign_and_distant_sheep() {
    let (mut state, slot, mut v) = setup();
    state.entities.set_pos(slot, [3.5, 201.0, 3.5]);
    let baby = sheep(&mut state, [4.5, 201.0, 4.5], 0);
    state.entities.row_mut(baby).entity_life.child_ticks = 200;
    let cow_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    state.spawn_mob(cow_type, [3.5, 201.0, 4.5], 10.0);
    let far = sheep(&mut state, [108.5, 201.0, 108.5], 0);
    let appearances = state.appearance_changed.clone();
    let mut changes = ChangeSet::default();
    state.tick = 20;
    assert_eq!(shepherd::work(&mut state, slot, &mut v, &mut changes), None);
    assert!(v.inventory.iter().all(Option::is_none));
    assert_eq!(
        crate::tick::wool_of(&state.entities.row(baby).appearance),
        0,
        "baby keeps its wool"
    );
    assert_eq!(
        crate::tick::wool_of(&state.entities.row(far).appearance),
        0,
        "distant sheep keeps its wool"
    );
    assert_eq!(state.appearance_changed, appearances);
}

#[test]
fn shepherd_full_carry_keeps_sheep_wool() {
    let (mut state, slot, mut v) = setup();
    let sheep_slot = sheep(&mut state, [3.5, 201.0, 3.5], 0);
    state.entities.set_pos(slot, [3.5, 201.0, 3.5]);
    let cobble = mining::item_id(&state, "cobblestone").unwrap();
    v.inventory = std::array::from_fn(|_| Some(ItemStack::new(cobble, 64)));
    let before = v.inventory.clone();
    let mut changes = ChangeSet::default();
    state.tick = 20;
    // No storage certified: the deposit has nowhere to go.
    assert_eq!(shepherd::work(&mut state, slot, &mut v, &mut changes), None);
    assert_eq!(
        crate::tick::wool_of(&state.entities.row(sheep_slot).appearance),
        0,
        "no room means no shearing"
    );
    assert_eq!(v.inventory, before, "carry preserved without storage");
}

#[test]
fn shepherd_stock_deposit_conserves_wool() {
    let (mut state, slot, mut v) = setup();
    storage(&mut state, &v, [4, 201, 3]);
    let white = mining::item_id(&state, "white_wool").unwrap();
    state.block_entities.get_mut(&(4, 201, 3)).unwrap().slots[0] = ItemStack::new(white, 10);
    v.inventory = std::array::from_fn(|_| Some(ItemStack::new(white, 64)));
    state.entities.set_pos(slot, [4.5, 201.0, 3.5]);
    let mut changes = ChangeSet::default();
    state.tick = 20;
    assert_eq!(shepherd::work(&mut state, slot, &mut v, &mut changes), None);
    assert!(v.inventory.iter().all(Option::is_none));
    let chest = state.block_entities.get(&(4, 201, 3)).unwrap();
    let total: i32 = chest.slots.iter().map(|stack| stack.count).sum();
    assert_eq!(total, 10 + 8 * 64, "stock plus carry, nothing duplicated");
}
