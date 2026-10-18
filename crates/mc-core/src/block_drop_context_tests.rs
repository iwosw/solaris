use super::*;
use crate::crafter::tests::{block, fixture, stack};
use mc_data::ItemStack;

fn setup() -> (TickState, crate::boundary::EntityId, BlockPos, u32) {
    let (mut state, actor) = fixture(None);
    let vanilla = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    state.registry =
        std::sync::Arc::new(crate::registry::CoreRegistry::load(Some(&vanilla)).unwrap());
    let pos = BlockPos { x: 8, y: 201, z: 8 };
    let current = block(&state, "blue_shulker_box", &[]);
    state.world.set_block(pos.x, pos.y, pos.z, current);
    let mut bytes = Vec::new();
    mc_nbt::write_network(
        &mut bytes,
        &Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:shulker_box".into())),
            ("x".into(), Tag::Int(pos.x)),
            ("y".into(), Tag::Int(pos.y)),
            ("z".into(), Tag::Int(pos.z)),
            ("CustomName".into(), Tag::String("Box".into())),
        ]),
    )
    .unwrap();
    state.world.set_opaque_block_entity(pos, Some(bytes));
    state.world.fold_overlay();
    let mut slots = vec![ItemStack::EMPTY; 27];
    slots[3] = stack(&state, "diamond", 2);
    state.block_entities.insert(
        (pos.x, pos.y, pos.z),
        crate::tick::BlockEntityState {
            slots,
            furnace: None,
        },
    );
    (state, actor, pos, current)
}

fn check_contents(state: &TickState, item: &ItemStack) {
    let fields = mc_data::item_nbt::stack_to_fields(&state.registry.items, item).unwrap();
    let Tag::Compound(components) = mc_data::item_nbt::field(&fields, "components").unwrap() else {
        panic!();
    };
    let Tag::List(contents) = mc_data::item_nbt::field(components, "minecraft:container").unwrap()
    else {
        panic!();
    };
    assert_eq!(contents.elements.len(), 1);
    let Tag::Compound(slot) = &contents.elements[0] else {
        panic!();
    };
    assert_eq!(mc_data::item_nbt::field(slot, "slot"), Some(&Tag::Int(3)));
    let Tag::Compound(item) = mc_data::item_nbt::field(slot, "item").unwrap() else {
        panic!();
    };
    assert_eq!(mc_data::item_nbt::field(item, "count"), Some(&Tag::Int(2)));
    assert_eq!(
        mc_data::item_nbt::field(item, "id"),
        Some(&Tag::String("minecraft:diamond".into()))
    );
}

#[test]
fn live_container_slots_become_one_lossless_item_image() {
    let (state, _, pos, current) = setup();
    let drops = roll(&state, pos, current, &ItemStack::EMPTY, 1).unwrap();
    assert_eq!(drops.len(), 1);
    assert!(retains_contents(&drops));
    check_contents(&state, &drops[0]);
    let encoded = serde_json::to_vec(&drops[0]).unwrap();
    let restored: ItemStack = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(restored, drops[0]);
    check_contents(&state, &restored);
}

#[test]
fn rejected_duplicate_break_never_stages_or_spills_a_second_container() {
    let (mut state, actor, pos, current) = setup();
    let air = state.registry.states.air;
    let mut intents = (1..=2)
        .map(|seq| crate::intent::Intent {
            actor,
            seq,
            kind: crate::intent::IntentKind::BreakBlock { pos },
            footprint: crate::intent::Footprint {
                block_expect: vec![(pos, current)],
                block_write: vec![(pos, air)],
                ..Default::default()
            },
        })
        .collect();
    assert_eq!(state.merge(&mut intents), 1);
    assert_eq!(state.merge_broken.len(), 1);
    state.world.set_block(pos.x, pos.y, pos.z, air);
    state.world.fold_overlay();
    let (_, _, spawned, _) = state.commit_merge_effects();
    let items: Vec<_> = spawned
        .into_iter()
        .filter_map(|id| state.entities.slot_of(id))
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .collect();
    assert_eq!(items.len(), 1, "no loose diamond spill or second box");
    check_contents(
        &state,
        items[0]
            .stack
            .as_ref()
            .expect("copied box carries its component image"),
    );
    assert!(!state.block_entities.contains_key(&(pos.x, pos.y, pos.z)));
    assert!(state.merge_broken.is_empty());
}

#[test]
fn accepted_destroy_keeps_its_once_rolled_loot_after_a_later_replacement_write() {
    let (mut state, _, pos, current) = setup();
    state.stage_command_break(pos, current);
    state
        .world
        .set_block(pos.x, pos.y, pos.z, state.registry.states.air);
    state
        .world
        .set_block(pos.x, pos.y, pos.z, state.registry.states.water);
    state.world.fold_overlay();
    let (_, _, spawned, _) = state.commit_merge_effects();
    let items: Vec<_> = spawned
        .into_iter()
        .filter_map(|id| state.entities.slot_of(id))
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .collect();
    assert_eq!(
        items.len(),
        1,
        "replacement cannot cancel an accepted break or spill the box twice"
    );
    check_contents(
        &state,
        items[0]
            .stack
            .as_ref()
            .expect("copied box carries its component image"),
    );
    assert!(state.merge_command_broken.is_empty());
    assert!(
        state.commit_merge_effects().2.is_empty(),
        "a consumed break image is never replayed in a later commit"
    );
}

#[test]
fn shulker_break_pickup_and_accepted_or_rejected_clicks_publish_one_custody_image() {
    use crate::boundary::{ClickMode, ConnectionId, PlayerChange, PlayerInput};
    let (mut state, actor, pos, current) = setup();
    state.stage_command_break(pos, current);
    state
        .world
        .set_block(pos.x, pos.y, pos.z, state.registry.states.air);
    state.world.fold_overlay();
    let (_, _, spawned, _) = state.commit_merge_effects();
    assert_eq!(spawned.len(), 1);
    let item_id = spawned[0];
    let item_slot = state.entities.slot_of(item_id).unwrap();
    state.entities.body_mut(item_slot).pickup_delay = 0;
    let actor_pos = state.entities.pos(state.entities.slot_of(actor).unwrap());
    state.entities.set_pos(item_slot, actor_pos);
    crate::tick::run_tick(&mut state, &[]);
    assert!(state.entities.slot_of_any(item_id).is_none());
    let hotbar = state.players[&actor]
        .inventory
        .slots
        .iter()
        .position(|item| {
            item.extended()
                .preserved_components
                .iter()
                .any(|component| component.name == "minecraft:container")
        })
        .expect("picked-up shulker retains its contents");
    assert!(hotbar < 9);
    check_contents(&state, &state.players[&actor].inventory.slots[hotbar]);
    for (slot, expect_cursor) in [
        (36 + hotbar as i16, true),
        (5, true),
        (36 + hotbar as i16, false),
    ] {
        let PlayerChange::MenuContents { state_id, .. } =
            state.players[&actor].inventory_contents()
        else {
            panic!()
        };
        let (_, queues, _) = crate::tick::run_tick(
            &mut state,
            &[PlayerInput::ContainerClick {
                connection: ConnectionId(1),
                window: 0,
                state_id,
                slot,
                button: 0,
                mode: ClickMode::Pickup,
            }],
        );
        let images: Vec<_> = queues
            .iter()
            .flat_map(|(_, changes)| changes)
            .filter_map(|change| {
                if let PlayerChange::MenuContents { slots, cursor, .. } = change {
                    Some((slots, cursor))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            images.len(),
            1,
            "accepted and rejected clicks each publish one full image"
        );
        let (slots, cursor) = images[0];
        assert!(slots[5].is_none(), "a shulker cannot enter the helmet slot");
        if expect_cursor {
            check_contents(&state, cursor.as_ref().expect("cursor owns the shulker"));
            assert!(slots[36 + hotbar].is_none());
        } else {
            assert!(cursor.is_none());
            check_contents(
                &state,
                slots[36 + hotbar]
                    .as_ref()
                    .expect("hotbar owns the shulker"),
            );
        }
    }
    assert!(!state.block_entities.contains_key(&(pos.x, pos.y, pos.z)));
}

#[test]
fn corrupt_block_entity_break_quarantines_without_writes_debit_or_custody_loss() {
    let mut scalar = Vec::new();
    mc_nbt::write_network_value(&mut scalar, &Tag::String("invalid root".into())).unwrap();
    for (raw, expected) in [
        (vec![10], None),
        (
            vec![10, 0, 1],
            Some("storage: block loot: trailing block entity NBT"),
        ),
        (
            scalar,
            Some(
                "storage: block loot: corrupt block entity NBT: non-Compound type 0x8 used where a Compound is required",
            ),
        ),
    ] {
        let (mut state, actor, pos, current) = setup();
        state.world.set_opaque_block_entity(pos, Some(raw.clone()));
        let chunk_pos = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        let snapshot = state.world.chunk(chunk_pos).unwrap().clone();
        let inventory = state.players[&actor].inventory.slots.clone();
        let error = capture(&state, pos, current).unwrap_err();
        assert!(matches!(error, crate::CoreError::Storage(_)));
        let reason = error.to_string();
        if let Some(expected) = expected {
            assert_eq!(reason, expected);
        } else {
            assert!(reason.starts_with("storage: block loot: corrupt block entity NBT: "));
        }
        let mut intents = vec![crate::intent::Intent {
            actor,
            seq: 1,
            kind: crate::intent::IntentKind::BreakBlock { pos },
            footprint: crate::intent::Footprint {
                block_expect: vec![(pos, current)],
                block_write: vec![(pos, state.registry.states.air)],
                ..Default::default()
            },
        }];
        assert_eq!(state.merge(&mut intents), 0);
        assert_eq!(state.world.load_error(chunk_pos).unwrap().reason, reason);
        assert!(state.world.chunk(chunk_pos).is_none());
        assert!(
            !state.stage_command_break(pos, current),
            "quarantined custody cannot be rolled again by a queued command"
        );
        assert!(state.merge_command_broken.is_empty());
        assert_eq!(state.world.load_error(chunk_pos).unwrap().reason, reason);
        assert!(state.merge_block_writes.is_empty());
        assert!(state.merge_broken.is_empty());
        assert!(state.merge_container_contents.is_empty());
        assert_eq!(state.players[&actor].inventory.slots, inventory);
        assert_eq!(
            state.block_entities[&(pos.x, pos.y, pos.z)].slots[3],
            stack(&state, "diamond", 2)
        );
        assert_eq!(snapshot.block_entities[&pos], raw);
        assert!(state.commit_merge_effects().2.is_empty());
    }
}

#[test]
fn tnt_filled_shulker_drops_one_complete_box_without_loose_contents() {
    let (mut state, _, pos, _) = setup();
    state
        .gamerules
        .insert("tnt_explosion_drop_decay".into(), "false".into());
    let tnt = crate::tnt::prime_at(
        &mut state,
        [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5],
        1,
    )
    .unwrap();
    let second_tnt = crate::tnt::prime_at(
        &mut state,
        [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5],
        1,
    )
    .unwrap();
    crate::tick::run_tick(&mut state, &[]);
    assert!(state.entities.slot_of_any(tnt).is_none());
    assert!(state.entities.slot_of_any(second_tnt).is_none());
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(state.registry.states.air)
    );
    assert!(!state.block_entities.contains_key(&(pos.x, pos.y, pos.z)));
    let boxes: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| {
            state
                .entities
                .row(slot)
                .item
                .as_ref()
                .and_then(|row| row.stack.as_ref())
        })
        .filter(|item| {
            state
                .registry
                .items
                .name_of(item.item_id)
                .is_some_and(|name| name.path() == "blue_shulker_box")
        })
        .collect();
    assert_eq!(boxes.len(), 1);
    check_contents(&state, boxes[0]);
    let diamond = stack(&state, "diamond", 1).item_id;
    assert!(!state.entities.slots().any(|slot| {
        state
            .entities
            .row(slot)
            .item
            .as_ref()
            .is_some_and(|row| row.item_id == diamond)
    }));
    assert!(state.merge_container_contents.is_empty());
}

#[test]
fn explosion_decay_applies_without_stripping_copied_custody() {
    let (state, _, pos, current) = setup();
    for seed in 0..64 {
        let drops = explosion_drops(&state, pos, current, Some(4.0), seed).unwrap();
        for drop in &drops {
            check_contents(&state, drop);
        }
    }
    let dirt = block(&state, "dirt", &[]);
    assert!((0..64).any(|seed| {
        explosion_drops(&state, pos, dirt, Some(4.0), seed)
            .unwrap()
            .is_empty()
    }));
    assert_eq!(
        explosion_drops(&state, pos, dirt, None, 1).unwrap().len(),
        1
    );
}

#[test]
fn creative_filled_shulker_break_drops_one_named_box_never_loose_contents() {
    for block_drops in ["true", "false"] {
        let (mut state, actor, pos, current) = setup();
        state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
        state
            .gamerules
            .insert("block_drops".into(), block_drops.into());
        let air = state.registry.states.air;
        let mut intents = vec![crate::intent::Intent {
            actor,
            seq: 1,
            kind: crate::intent::IntentKind::BreakBlock { pos },
            footprint: crate::intent::Footprint {
                block_expect: vec![(pos, current)],
                block_write: vec![(pos, air)],
                ..Default::default()
            },
        }];
        assert_eq!(state.merge(&mut intents), 1);
        state.world.set_block(pos.x, pos.y, pos.z, air);
        state.world.fold_overlay();
        let (_, _, spawned, _) = state.commit_merge_effects();
        let items: Vec<_> = spawned
            .into_iter()
            .filter_map(|id| state.entities.slot_of(id))
            .filter_map(|slot| state.entities.row(slot).item.as_ref())
            .collect();
        assert_eq!(items.len(), 1, "no loose diamond spill");
        let item = items[0].stack.as_ref().unwrap();
        assert_eq!(item.item_id, stack(&state, "blue_shulker_box", 1).item_id);
        assert_eq!(item.count, 1);
        check_contents(&state, item);
        let fields = mc_data::item_nbt::stack_to_fields(&state.registry.items, item).unwrap();
        let Some(Tag::Compound(components)) = mc_data::item_nbt::field(&fields, "components")
        else {
            panic!();
        };
        assert_eq!(
            mc_data::item_nbt::field(components, "minecraft:custom_name"),
            Some(&Tag::String("Box".into()))
        );
        assert!(!state.block_entities.contains_key(&(pos.x, pos.y, pos.z)));
    }
    let (mut state, actor, pos, current) = setup();
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    state
        .block_entities
        .get_mut(&(pos.x, pos.y, pos.z))
        .unwrap()
        .slots
        .fill(ItemStack::EMPTY);
    assert!(
        player_drops(&state, actor, pos, current)
            .unwrap()
            .is_empty(),
        "empty creative boxes do not drop"
    );
}
