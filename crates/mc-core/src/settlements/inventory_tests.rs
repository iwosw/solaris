use super::*;
use crate::boundary::ChunkPos;
use std::sync::Arc;

fn state() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 200, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn body(state: &mut TickState) -> Endpoint {
    let kind = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let entity = state.spawn_mob(kind, [8.0, 201.0, 8.0], 20.0);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState::default()));
    Endpoint::VillagerCarry {
        uuid: state.entities.row(slot).uuid,
    }
}

fn emeralds(state: &TickState, count: i32) -> ItemStack {
    let id = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    ItemStack::new(id, count)
}

#[test]
fn native_uuid_carry_preserves_components_and_uses_canonical_fences() {
    let mut state = state();
    let source = body(&mut state);
    let destination = body(&mut state);
    let before = load(&state, &source).unwrap();
    let mut stock = before.slots.clone();
    stock[0] = emeralds(&state, 5);
    stock[0].custom_name = Some("native payroll".into());
    commit(
        &mut state,
        vec![Change {
            before,
            after: stock,
        }],
    )
    .unwrap();
    let source_image = load(&state, &source).unwrap();
    let destination_image = load(&state, &destination).unwrap();
    let old_source = source_image.fence.clone();
    let old_destination = destination_image.fence.clone();
    let mut source_slots = source_image.slots.clone();
    let mut destination_slots = destination_image.slots.clone();
    destination_slots[0] = source_slots[0].clone();
    destination_slots[0].count = 3;
    source_slots[0].count = 2;
    let fences = commit(
        &mut state,
        vec![
            Change {
                before: source_image,
                after: source_slots,
            },
            Change {
                before: destination_image,
                after: destination_slots,
            },
        ],
    )
    .unwrap();
    assert!(fences[0].revision > old_source.revision);
    assert!(fences[1].revision > old_destination.revision);
    let after = load(&state, &destination).unwrap();
    assert_eq!(after.fence, fences[1]);
    assert_eq!(
        after.slots[0].custom_name.as_deref(),
        Some("native payroll")
    );
    assert_eq!(
        after.slots[0].count + load(&state, &source).unwrap().slots[0].count,
        5
    );
    assert!(
        state
            .plugins
            .domain_entries("__native_npcs")
            .next()
            .is_none()
    );
    assert_eq!(
        state.plugins.domain_entries("@native-inventories").count(),
        3
    );
}

#[test]
fn stale_or_over_capacity_endpoint_refuses_entire_native_transaction() {
    let mut state = state();
    let source = body(&mut state);
    let destination = body(&mut state);
    let old_source = load(&state, &source).unwrap();
    let old_destination = load(&state, &destination).unwrap();
    let mut initial = old_source.slots.clone();
    initial[0] = emeralds(&state, 5);
    commit(
        &mut state,
        vec![Change {
            before: old_source.clone(),
            after: initial,
        }],
    )
    .unwrap();
    let source_now = load(&state, &source).unwrap();
    let mut imagined = old_destination.slots.clone();
    imagined[0] = emeralds(&state, 3);
    assert_eq!(
        commit(
            &mut state,
            vec![
                Change {
                    before: old_destination.clone(),
                    after: imagined
                },
                Change {
                    before: old_source.clone(),
                    after: old_source.slots.clone()
                },
            ]
        )
        .unwrap_err(),
        Failure::StaleRevision
    );
    assert_eq!(load(&state, &source).unwrap().slots, source_now.slots);
    assert_eq!(
        load(&state, &destination).unwrap().slots,
        old_destination.slots
    );
    let mut removed = source_now.slots.clone();
    removed[0] = ItemStack::EMPTY;
    let mut overflow = old_destination.slots.clone();
    overflow[0] = emeralds(&state, 65);
    assert_eq!(
        commit(
            &mut state,
            vec![
                Change {
                    before: source_now.clone(),
                    after: removed
                },
                Change {
                    before: old_destination.clone(),
                    after: overflow
                },
            ]
        )
        .unwrap_err(),
        Failure::Capacity
    );
    assert_eq!(load(&state, &source).unwrap().fence, source_now.fence);
    assert_eq!(
        load(&state, &destination).unwrap().fence,
        old_destination.fence
    );
}

fn chest(state: &mut TickState, x: i32, kind: &str) -> Endpoint {
    state
        .tickets
        .insert("native-inventory".into(), (ChunkPos { x: 0, z: 0 }, 0));
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    state.refresh_active_chunks(state.tick);
    let id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let default = state.registry.blocks.block(&id).unwrap().default;
    let mut properties = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (name, value) in &mut properties {
        match name.as_str() {
            "type" => *value = kind.into(),
            "facing" => *value = "north".into(),
            _ => {}
        }
    }
    let block = state
        .registry
        .blocks
        .by_name_and_props(&id, &properties)
        .unwrap();
    state.world.set_block(x, 200, 8, block.0);
    state.block_entities.insert(
        (x, 200, 8),
        crate::tick::BlockEntityState {
            slots: vec![ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    Endpoint::Container {
        dimension: "minecraft:overworld".into(),
        position: [x, 200, 8],
    }
}

#[test]
fn both_halves_of_one_native_chest_cannot_appear_twice_in_a_transaction() {
    let mut state = state();
    let left = chest(&mut state, 8, "left");
    let right = chest(&mut state, 9, "right");
    let left = load(&state, &left).unwrap();
    let right = load(&state, &right).unwrap();
    assert_eq!(left.slots.len(), 54);
    assert_eq!(left.fence, right.fence);
    let mut after_left = left.slots.clone();
    let mut after_right = right.slots.clone();
    after_left[0] = emeralds(&state, 1);
    after_right[0] = emeralds(&state, 2);
    assert_eq!(
        commit(
            &mut state,
            vec![
                Change {
                    before: left.clone(),
                    after: after_left
                },
                Change {
                    before: right,
                    after: after_right
                },
            ]
        )
        .unwrap_err(),
        Failure::InvalidRequest
    );
    assert_eq!(load(&state, &left.endpoint).unwrap().slots, left.slots);
    assert!(
        state
            .plugins
            .domain_entries("@native-inventories")
            .next()
            .is_none()
    );
}
