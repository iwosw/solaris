use super::*;
use crate::entities::EntitySlot;
use crate::villager::{VillagerProfession, VillagerState};
use mc_data::{Identifier, ItemStack};

pub(crate) fn fixture() -> (TickState, SettlementId, EntitySlot) {
    let registry = std::sync::Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = std::sync::Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        registry.blocks.clone(),
    ));
    let mut world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 201, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(&rustc_hash::FxHashMap::from_iter([(
        crate::boundary::ChunkPos { x: 0, z: 0 },
        0,
    )]));
    for x in 0..16 {
        for z in 0..16 {
            world.set_block(x, 200, z, registry.states.stone);
            for y in 201..206 {
                world.set_block(x, y, z, registry.states.air);
            }
        }
    }
    world.fold_overlay();
    let mut state = TickState::new(registry, world, 2, 512);
    state.tickets.insert(
        "army-fixture".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.refresh_active_chunks(0);
    let id = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "army".into(),
            "owner".into(),
            [8, 201, 8],
            16,
        )
        .unwrap();
    let record = state.settlements.get_mut(id).unwrap();
    for (seq, kind, min, max, points) in [
        (1, BuildingKind::Barracks, [1, 200, 1], [5, 204, 5], vec![]),
        (
            2,
            BuildingKind::Storage,
            [6, 201, 6],
            [8, 203, 8],
            vec![[6, 201, 6]],
        ),
    ] {
        let bid = BuildingId((id.0 << 32) | seq);
        record.buildings.insert(
            bid,
            buildings::Building {
                id: bid,
                settlement: id,
                kind,
                level: 1,
                bounds: mc_script::ScriptWorldBounds::new(min, max).unwrap(),
                work_points: points,
                revision: 1,
                natural_zone: false,
            },
        );
    }
    record.building_sequence = 2;
    let chest = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:chest").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(6, 201, 6, chest);
    state.block_entities.insert(
        (6, 201, 6),
        crate::tick::BlockEntityState {
            slots: vec![ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let entity = state.spawn_mob(type_id, [3.5, 201.0, 3.5], 20.0);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.row_mut(slot).villager = Some(Box::new(VillagerState {
        settlement: Some(id),
        ..Default::default()
    }));
    (state, id, slot)
}
fn item(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
}

#[test]
fn recruit_fills_barracks_slots_and_equipment_is_debited_exactly_once() {
    let (mut state, id, slot) = fixture();
    let sword = item(&state, "diamond_sword");
    let helmet = item(&state, "iron_helmet");
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[0] = ItemStack::new(sword, 1);
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[1] = ItemStack::new(helmet, 1);
    let first = recruit(&mut state, id).unwrap();
    assert_eq!(
        state.entities.row(slot).appearance.equipment[0],
        Some(ItemStack::new(sword, 1))
    );
    assert_eq!(
        state.entities.row(slot).appearance.equipment[5],
        Some(ItemStack::new(helmet, 1))
    );
    assert!(
        state.block_entities[&(6, 201, 6)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    assert!(recruit(&mut state, id).is_err());
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    for _ in 0..2 {
        let entity = state.spawn_mob(type_id, [4.5, 201.0, 4.5], 20.0);
        let s = state.entities.slot_of(entity).unwrap();
        state.entities.row_mut(s).villager = Some(Box::new(VillagerState {
            settlement: Some(id),
            ..Default::default()
        }));
    }
    recruit(&mut state, id).unwrap();
    assert!(
        recruit(&mut state, id).is_err(),
        "level one barracks has two beds/slots"
    );
    dismiss(&mut state, id, first).unwrap();
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .data
            .profession,
        VillagerProfession::None
    );
    assert!(
        state
            .entities
            .row(slot)
            .appearance
            .equipment
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        state.block_entities[&(6, 201, 6)]
            .slots
            .iter()
            .filter(|s| s.item_id == sword && !s.is_empty())
            .map(|s| s.count)
            .sum::<i32>(),
        1
    );
    assert_eq!(
        state.block_entities[&(6, 201, 6)]
            .slots
            .iter()
            .filter(|s| s.item_id == helmet && !s.is_empty())
            .map(|s| s.count)
            .sum::<i32>(),
        1
    );
}

#[test]
fn dismissal_with_full_storage_is_atomic() {
    let (mut state, id, slot) = fixture();
    let sword = item(&state, "iron_sword");
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[0] = ItemStack::new(sword, 1);
    let uuid = recruit(&mut state, id).unwrap();
    let dirt = item(&state, "dirt");
    state
        .block_entities
        .get_mut(&(6, 201, 6))
        .unwrap()
        .slots
        .fill(ItemStack::new(dirt, 64));
    assert!(dismiss(&mut state, id, uuid).is_err());
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .soldier
            .is_some()
    );
    assert_eq!(
        state.entities.row(slot).appearance.equipment[0],
        Some(ItemStack::new(sword, 1))
    );
}

#[test]
fn army_orders_survive_villager_nbt_roundtrip() {
    let (mut state, id, slot) = fixture();
    let uuid = recruit(&mut state, id).unwrap();
    order(&mut state, id, Some(uuid), Order::Guard([4, 201, 5])).unwrap();
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    let encoded =
        crate::villager::persistence::villager_to_fields(&state.registry.items, v, state.tick)
            .unwrap();
    let restored = crate::villager::persistence::villager_from_fields(
        &state.registry.items,
        &encoded,
        state.tick,
    )
    .unwrap();
    assert_eq!(restored.soldier, v.soldier);
}

#[test]
fn recruit_chooses_best_available_weapon_and_guard_rejects_outside_radius() {
    let (mut state, id, slot) = fixture();
    let wooden = item(&state, "wooden_sword");
    let diamond = item(&state, "diamond_sword");
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[0] = ItemStack::new(wooden, 1);
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[1] = ItemStack::new(diamond, 1);
    let uuid = recruit(&mut state, id).unwrap();
    assert_eq!(
        state.entities.row(slot).appearance.equipment[0],
        Some(ItemStack::new(diamond, 1))
    );
    assert_eq!(
        state.block_entities[&(6, 201, 6)].slots[0],
        ItemStack::new(wooden, 1)
    );
    assert!(order(&mut state, id, Some(uuid), Order::Guard([100, 201, 100])).is_err());
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .soldier
            .as_ref()
            .unwrap()
            .order,
        Order::Patrol
    );
}

#[test]
fn bootstrap_storage_is_canonical_deduplicated_equipment_source() {
    let (mut state, id, slot) = fixture();
    state.settlements.get_mut(id).unwrap().bootstrap_storage = Some([6, 201, 6]);
    assert_eq!(buildings::storage_points(&state, id), vec![[6, 201, 6]]);
    state
        .settlements
        .get_mut(id)
        .unwrap()
        .buildings
        .retain(|_, b| b.kind != BuildingKind::Storage);
    let sword = item(&state, "iron_sword");
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[0] = ItemStack::new(sword, 1);
    recruit(&mut state, id).unwrap();
    assert_eq!(
        state.entities.row(slot).appearance.equipment[0],
        Some(ItemStack::new(sword, 1))
    );
    assert!(state.block_entities[&(6, 201, 6)].slots[0].is_empty());
}

#[test]
fn both_double_chest_halves_are_debited_as_one_physical_inventory() {
    let (mut state, id, slot) = fixture();
    for (x, kind) in [(6, "left"), (7, "right")] {
        let chest = crate::settlements::allocator::tests::block(
            &state,
            "minecraft:chest",
            &[("facing", "north"), ("type", kind)],
        );
        state.world.set_block(x, 201, 6, chest);
        state.block_entities.insert(
            (x, 201, 6),
            crate::tick::BlockEntityState {
                slots: vec![ItemStack::EMPTY; 27],
                furnace: None,
            },
        );
    }
    let building = state
        .settlements
        .get_mut(id)
        .unwrap()
        .buildings
        .get_mut(&BuildingId((id.0 << 32) | 2))
        .unwrap();
    building.work_points.push([7, 201, 6]);
    let sword = item(&state, "diamond_sword");
    let helmet = item(&state, "iron_helmet");
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[0] = ItemStack::new(sword, 1);
    state.block_entities.get_mut(&(7, 201, 6)).unwrap().slots[0] = ItemStack::new(helmet, 1);
    let uuid = recruit(&mut state, id).unwrap();
    assert_eq!(
        state.entities.row(slot).appearance.equipment[0],
        Some(ItemStack::new(sword, 1))
    );
    assert_eq!(
        state.entities.row(slot).appearance.equipment[5],
        Some(ItemStack::new(helmet, 1))
    );
    assert!(state.block_entities[&(6, 201, 6)].slots[0].is_empty());
    assert!(state.block_entities[&(7, 201, 6)].slots[0].is_empty());
    dismiss(&mut state, id, uuid).unwrap();
    let stock = inventory::load(
        &state,
        &inventory::Endpoint::Container {
            dimension: state.dimension.name().into(),
            position: [6, 201, 6],
        },
    )
    .unwrap();
    assert_eq!(
        stock
            .slots
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.count)
            .sum::<i32>(),
        2
    );
}

#[test]
fn loaded_autonomous_settlement_recruits_on_bounded_cadence() {
    let (mut state, id, slot) = fixture();
    state.settlements.get_mut(id).unwrap().owner = None;
    state.tick = 99;
    tick(&mut state);
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .soldier
            .is_none()
    );
    state.tick = 100;
    tick(&mut state);
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .soldier
            .is_some()
    );
    assert_eq!(list(&state, id).len(), 1);
}

#[test]
fn barracks_slots_equal_certified_bed_requirement() {
    for (level, expected) in [(0, 0), (1, 2), (2, 4), (3, 6), (4, 0)] {
        assert_eq!(
            buildings::job_slots(BuildingKind::Barracks, level),
            expected
        );
    }
}

#[test]
fn owned_settlement_keeps_soldier_dismissed_after_automatic_staffing_cycles() {
    let (mut state, id, slot) = fixture();
    let sword = item(&state, "iron_sword");
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[0] = ItemStack::new(sword, 1);
    let soldier = recruit(&mut state, id).unwrap();
    dismiss(&mut state, id, soldier).unwrap();
    for _ in 0..201 {
        crate::tick::run_tick(&mut state, &[]);
    }
    let resident = state.entities.row(slot).villager.as_ref().unwrap();
    assert!(
        resident.soldier.is_none(),
        "owned settlements must not undo an owner's dismissal"
    );
    assert_ne!(resident.data.profession, VillagerProfession::Soldier);
    assert!(state.entities.row(slot).appearance.equipment[0].is_none());
    assert_eq!(
        state.block_entities[&(6, 201, 6)]
            .slots
            .iter()
            .filter(|stack| !stack.is_empty() && stack.item_id == sword)
            .map(|stack| stack.count)
            .sum::<i32>(),
        1
    );
    recruit(&mut state, id).expect("the owner can explicitly recruit again");
}

#[test]
fn recruited_soldier_gear_drops_on_death_and_dismissal_restores_default_chance() {
    use crate::survival::DamageKind;
    use crate::tick::DamagePiece;
    let (mut state, id, slot) = fixture();
    let sword = item(&state, "diamond_sword");
    let helmet = item(&state, "iron_helmet");
    let default_chances = crate::entity_loot::EntityLootState::default().drop_chances;
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[0] = ItemStack::new(sword, 1);
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[1] = ItemStack::new(helmet, 1);
    let soldier = recruit(&mut state, id).unwrap();
    assert_eq!(state.entities.row(slot).loot.drop_chances[0], 2.0);
    assert_eq!(state.entities.row(slot).loot.drop_chances[5], 2.0);
    assert_eq!(
        state.entities.row(slot).loot.drop_chances[2],
        default_chances[2]
    );
    dismiss(&mut state, id, soldier).unwrap();
    assert_eq!(state.entities.row(slot).loot.drop_chances, default_chances);
    recruit(&mut state, id).unwrap();
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (_, _, dropped, _) = state.commit_merge_effects();
    let mut items: Vec<_> = dropped
        .iter()
        .filter_map(|entity| state.entities.slot_of(*entity))
        .filter_map(|slot| state.entities.row(slot).item.as_ref().map(|s| s.item_id))
        .collect();
    items.sort_unstable();
    let mut expected = vec![sword, helmet];
    expected.sort_unstable();
    assert_eq!(items, expected);
}
