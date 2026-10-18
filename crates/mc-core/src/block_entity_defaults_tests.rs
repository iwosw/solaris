use super::*;
use crate::boundary::{ChangeSet, ConnectionId, EntityId};
use crate::crafter::tests::{block, fixture};

pub(crate) fn command(state: &mut TickState, actor: EntityId, text: &str) {
    let mut words = text.split_whitespace();
    let root = words.next().unwrap();
    let parsed = crate::command_world::parse(root, &words.collect::<Vec<_>>()).unwrap();
    crate::command_world::execute(
        state,
        ConnectionId(1),
        actor,
        &parsed,
        &mut ChangeSet::default(),
    );
}

#[test]
fn every_native_block_entity_type_is_created_by_setblock_at_shared_commit_boundary() {
    let (mut state, actor) = fixture(None);
    let pos = BlockPos { x: 8, y: 201, z: 8 };
    let mut count = 0;
    for kind in BlockEntityKind::ALL {
        for path in kind.block_paths() {
            state.block_entities.remove(&(8, 201, 8));
            state.world.set_opaque_block_entity(pos, None);
            state.world.set_block(8, 201, 8, state.registry.states.air);
            state.world.fold_overlay();
            command(&mut state, actor, &format!("setblock 8 201 8 {path}"));
            let writes = state.merge_block_writes.clone();
            assert_eq!(writes.len(), 1, "{path}");
            for &(pos, id) in &writes {
                state.world.set_block(pos.x, pos.y, pos.z, id);
            }
            state.world.fold_overlay();
            state.commit_merge_effects();
            after_blocks(&mut state, &writes);
            let bytes = &state
                .world
                .chunk(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .block_entities[&pos];
            let Tag::Compound(fields) = mc_nbt::read_network(&mut bytes.as_slice()).unwrap() else {
                panic!("{path}");
            };
            assert_eq!(
                mc_data::item_nbt::field(&fields, "id"),
                Some(&Tag::String(kind.vanilla_id().into())),
                "{path}"
            );
            assert_eq!(mc_data::item_nbt::field(&fields, "x"), Some(&Tag::Int(8)));
            if let Some(size) = kind.inventory_size() {
                assert_eq!(
                    state.block_entities[&(8, 201, 8)].slots.len(),
                    size,
                    "{path}"
                );
            }
            state.merge_block_writes.clear();
            count += 1;
        }
    }
    assert_eq!(count, 201);
}

#[test]
fn fill_creates_each_container_without_opening_and_property_write_keeps_custody() {
    let (mut state, actor) = fixture(None);
    command(&mut state, actor, "fill 7 201 8 9 201 8 black_shulker_box");
    crate::tick::run_tick(&mut state, &[]);
    for x in 7..=9 {
        let pos = BlockPos { x, y: 201, z: 8 };
        assert!(
            state
                .world
                .chunk(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .block_entities
                .contains_key(&pos)
        );
        assert_eq!(state.block_entities[&(x, 201, 8)].slots.len(), 27);
    }
    let diamond = crate::crafter::tests::stack(&state, "diamond", 4);
    state.block_entities.get_mut(&(8, 201, 8)).unwrap().slots[26] = diamond.clone();
    crate::shulker::run(&mut state);
    let id = block(&state, "black_shulker_box", &[("facing", "north")]);
    let pos = BlockPos { x: 8, y: 201, z: 8 };
    state.world.set_block(8, 201, 8, id);
    state.world.fold_overlay();
    after_blocks(&mut state, &[(pos, id)]);
    assert_eq!(state.block_entities[&(8, 201, 8)].slots[26], diamond);
}

#[test]
fn accepted_same_state_placements_replace_every_old_owner_not_property_write_custody() {
    let (mut state, actor) = fixture(None);
    let pos = BlockPos { x: 8, y: 201, z: 8 };
    for path in [
        "chest",
        "oak_shelf",
        "crafter",
        "beacon",
        "black_shulker_box",
    ] {
        state.block_entity_removed.clear();
        let id = block(&state, path, &[]);
        let kind = BlockEntityKind::for_block(path).unwrap();
        state.world.set_block(8, 201, 8, id);
        state.world.fold_overlay();
        let mut old = Vec::new();
        mc_nbt::write_network(
            &mut old,
            &Tag::Compound(vec![
                ("id".into(), Tag::String(kind.vanilla_id().into())),
                ("CustomName".into(), Tag::String("old owner".into())),
            ]),
        )
        .unwrap();
        state.world.set_opaque_block_entity(pos, Some(old));
        if let Some(size) = kind.inventory_size() {
            let mut slots = vec![mc_data::ItemStack::EMPTY; size];
            slots[0] = crate::crafter::tests::stack(&state, "diamond", 7);
            state.block_entities.insert(
                (8, 201, 8),
                BlockEntityState {
                    slots,
                    furnace: None,
                },
            );
        }
        state.world.take_block_entity_removals();
        // A pending break exposes air, but folded resident custody is still old.
        state.world.set_block(8, 201, 8, state.registry.states.air);
        let air = state.registry.states.air;
        let mut intents = vec![crate::intent::Intent {
            actor,
            seq: 1,
            kind: crate::intent::IntentKind::PlaceBlock { pos, state: id },
            footprint: crate::intent::Footprint {
                block_expect: vec![(pos, air)],
                block_write: vec![(pos, id)],
                ..Default::default()
            },
        }];
        assert_eq!(state.merge(&mut intents), 1, "{path}");
        state.world.set_block(8, 201, 8, id);
        state.world.fold_overlay();
        state.commit_merge_effects();
        crate::shulker::commit_placements(&mut state);
        after_blocks(&mut state, &[(pos, id)]);
        // Journal emission combines merge-owned and world-owned tombstones.
        let world_removed = state.world.take_block_entity_removals();
        assert!(
            state.block_entity_removed.contains(&(8, 201, 8)) || world_removed.contains(&pos),
            "{path}"
        );
        let image = &state
            .world
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities[&pos];
        let Tag::Compound(fields) = mc_nbt::read_network(&mut image.as_slice()).unwrap() else {
            panic!();
        };
        assert!(
            mc_data::item_nbt::field(&fields, "CustomName").is_none(),
            "{path}"
        );
        if kind.inventory_size().is_some() {
            assert!(
                state.block_entities[&(8, 201, 8)]
                    .slots
                    .iter()
                    .all(mc_data::ItemStack::is_empty),
                "{path}"
            );
        }
        state.merge_block_writes.clear();
        state.block_entities.remove(&(8, 201, 8));
    }
}

#[test]
fn setblock_and_fill_preserve_filled_chest_and_shulker_on_facing_changes() {
    for path in ["chest", "black_shulker_box"] {
        let (mut state, actor) = fixture(None);
        command(
            &mut state,
            actor,
            &format!("setblock 8 201 8 {path}[facing=north]"),
        );
        crate::tick::run_tick(&mut state, &[]);
        let diamond = crate::crafter::tests::stack(&state, "diamond", 13).with_custom_name("kept");
        state.block_entities.get_mut(&(8, 201, 8)).unwrap().slots[26] = diamond.clone();
        command(
            &mut state,
            actor,
            &format!("setblock 8 201 8 {path}[facing=east]"),
        );
        crate::tick::run_tick(&mut state, &[]);
        assert_eq!(
            state.world.block_at(8, 201, 8),
            Some(block(&state, path, &[("facing", "east")]))
        );
        assert_eq!(state.block_entities[&(8, 201, 8)].slots[26], diamond);
        command(
            &mut state,
            actor,
            &format!("fill 8 201 8 8 201 8 {path}[facing=south]"),
        );
        crate::tick::run_tick(&mut state, &[]);
        assert_eq!(state.block_entities[&(8, 201, 8)].slots[26], diamond);
        command(&mut state, actor, "setblock 8 201 8 barrel");
        crate::tick::run_tick(&mut state, &[]);
        assert!(
            state.block_entities[&(8, 201, 8)]
                .slots
                .iter()
                .all(mc_data::ItemStack::is_empty)
        );
    }
}

#[test]
fn setblock_and_fill_clear_same_be_type_when_actual_block_changes() {
    for root in ["setblock", "fill"] {
        for (old, new, size) in [
            ("white_shulker_box", "red_shulker_box", 27),
            ("chest", "copper_chest", 27),
            ("oak_shelf", "birch_shelf", 3),
        ] {
            let (mut state, actor) = fixture(None);
            command(&mut state, actor, &format!("setblock 8 201 8 {old}"));
            crate::tick::run_tick(&mut state, &[]);
            let diamond =
                crate::crafter::tests::stack(&state, "diamond", 13).with_custom_name("old custody");
            state.block_entities.get_mut(&(8, 201, 8)).unwrap().slots[size - 1] = diamond;
            let text = if root == "setblock" {
                format!("setblock 8 201 8 {new}")
            } else {
                format!("fill 8 201 8 8 201 8 {new}")
            };
            command(&mut state, actor, &text);
            crate::tick::run_tick(&mut state, &[]);
            assert_eq!(
                state.world.block_at(8, 201, 8),
                Some(block(&state, new, &[]))
            );
            assert_eq!(state.block_entities[&(8, 201, 8)].slots.len(), size);
            assert!(
                state.block_entities[&(8, 201, 8)]
                    .slots
                    .iter()
                    .all(mc_data::ItemStack::is_empty),
                "{root}: {old} -> {new}"
            );
        }
    }
}

#[test]
fn command_copper_family_transforms_preserve_existing_custody_like_world_writes() {
    let (mut state, actor) = fixture(None);
    command(&mut state, actor, "setblock 8 201 8 copper_chest");
    crate::tick::run_tick(&mut state, &[]);
    let diamond =
        crate::crafter::tests::stack(&state, "diamond", 13).with_custom_name("copper custody");
    state.block_entities.get_mut(&(8, 201, 8)).unwrap().slots[26] = diamond.clone();
    for text in [
        "setblock 8 201 8 exposed_copper_chest",
        "fill 8 201 8 8 201 8 waxed_exposed_copper_chest",
    ] {
        command(&mut state, actor, text);
        crate::tick::run_tick(&mut state, &[]);
        assert_eq!(state.block_entities[&(8, 201, 8)].slots[26], diamond);
    }
}
