use super::*;
use crate::boundary::{ChangeSet, EntityEvent};
use mc_data::{Identifier, ItemStack};

fn item_id(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
}
fn fixture(level: Option<u8>, count: i32) -> (TickState, EntityId, BlockPos) {
    let (mut state, actor, centre) = setup();
    let pos = BlockPos { y: Y, ..centre };
    let block = level.map_or(state.registry.states.air, |level| {
        state.registry.states.water_levels[level as usize]
    });
    place(&mut state, pos, block);
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(
        slot,
        [
            f64::from(pos.x) + 0.5,
            f64::from(Y + 1),
            f64::from(pos.z) + 0.5,
        ],
    );
    let id = item_id(&state, "glass_bottle");
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(id, count);
    (state, actor, pos)
}
fn use_item(hand: Hand, pitch: f32) -> PlayerInput {
    PlayerInput::UseItem {
        connection: ConnectionId(1),
        hand,
        yaw: 0.0,
        pitch,
        sequence: 11,
    }
}
fn click(pos: BlockPos) -> PlayerInput {
    PlayerInput::UseItemOn {
        connection: ConnectionId(1),
        position: pos,
        hand: Hand::Main,
        face: 1,
        cursor: [0.5; 3],
        sequence: 12,
    }
}
fn water(stack: &ItemStack, state: &TickState) -> bool {
    stack.item_id == item_id(state, "potion")
        && stack
            .extended()
            .potion
            .as_ref()
            .is_some_and(|id| id.as_str() == "minecraft:water")
}
fn potion(state: &TickState, name: &str) -> ItemStack {
    let mut stack = ItemStack::new(item_id(state, "potion"), 1);
    stack.extended_mut().potion = Some(Identifier::parse(format!("minecraft:{name}")).unwrap());
    stack
}
fn named(state: &TickState, name: &str, level: u8) -> u32 {
    let props = [(
        "level",
        match level {
            1 => "1",
            2 => "2",
            _ => "3",
        },
    )];
    block(
        state,
        name,
        if name == "water_cauldron" {
            &props
        } else {
            &[]
        },
    )
}

#[test]
fn bottle_use_item_source_fills_water_potion_without_draining_and_publishes_authority() {
    let (mut state, actor, pos) = fixture(Some(0), 1);
    let source = state.registry.states.water;
    let (out, reliable, _) = run_tick(&mut state, &[use_item(Hand::Main, 90.0)]);
    assert!(water(&state.players[&actor].inventory.slots[0], &state));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(source));
    assert!(
        reliable
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change,
        PlayerChange::MenuSlot { item: Some(stack), .. } if water(stack, &state)))
    );
    assert!(
        reliable
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change, PlayerChange::BlockActionsAcked { sequence: 11, .. }))
    );
    assert!(out.changes.entity_events.contains(&EntityEvent::Bottle {
        entity: actor,
        empty: false,
        cauldron: false
    }));
}

#[test]
fn bottle_use_item_air_flowing_lava_and_blocked_source_do_not_consume() {
    for (level, lava, blocked) in [
        (None, false, false),
        (Some(1), false, false),
        (None, true, false),
        (Some(0), false, true),
    ] {
        let (mut state, actor, pos) = fixture(level, 1);
        if lava {
            let lava = state.registry.states.lava_levels[0];
            place(&mut state, pos, lava);
        }
        if blocked {
            let stone = state.registry.states.stone;
            place(&mut state, BlockPos { y: Y + 1, ..pos }, stone);
        }
        let before = state.players[&actor].inventory.slots.clone();
        let (out, _, _) = run_tick(&mut state, &[use_item(Hand::Main, 90.0)]);
        assert_eq!(state.players[&actor].inventory.slots, before);
        assert!(
            !out.changes
                .entity_events
                .iter()
                .any(|event| matches!(event, EntityEvent::Bottle { .. }))
        );
    }
}

#[test]
fn bottle_use_item_stack_of_sixteen_conserves_fifteen_bottles_and_one_water_potion() {
    let (mut state, actor, _) = fixture(Some(0), 16);
    run_tick(&mut state, &[use_item(Hand::Main, 90.0)]);
    let slots = &state.players[&actor].inventory.slots;
    assert_eq!(
        slots[0],
        ItemStack::new(item_id(&state, "glass_bottle"), 15)
    );
    assert_eq!(
        slots
            .iter()
            .filter(|stack| water(stack, &state))
            .map(|stack| stack.count)
            .sum::<i32>(),
        1
    );
}

#[test]
fn bottle_use_item_full_inventory_drops_exactly_one_component_water_potion() {
    let (mut state, actor, _) = fixture(Some(0), 16);
    let stone = item_id(&state, "stone");
    state.players.get_mut(&actor).unwrap().inventory.slots[1..].fill(ItemStack::new(stone, 64));
    let (out, _, _) = run_tick(&mut state, &[use_item(Hand::Main, 90.0)]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 15);
    let drops: Vec<_> = out
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| state.entities.row(slot).item.as_deref())
        .map(crate::entities::StackRow::full_stack)
        .filter(|stack| water(stack, &state))
        .collect();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].count, 1);
}

#[test]
fn bottle_use_item_offhand_and_creative_input_keep_correct_custody() {
    let (mut state, actor, _) = fixture(Some(0), 1);
    let bottle = state.players[&actor].inventory.slots[0].clone();
    let stone = ItemStack::new(item_id(&state, "stone"), 1);
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.slots[0] = stone.clone();
    player.inventory.slots[crate::inventory::OFF_HAND] = bottle;
    run_tick(&mut state, &[use_item(Hand::Off, 90.0)]);
    assert_eq!(state.players[&actor].inventory.slots[0], stone);
    assert!(water(
        &state.players[&actor].inventory.slots[crate::inventory::OFF_HAND],
        &state
    ));

    let (mut state, actor, _) = fixture(Some(0), 16);
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    run_tick(
        &mut state,
        &[use_item(Hand::Main, 90.0), use_item(Hand::Main, 90.0)],
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 16);
    assert_eq!(
        state.players[&actor]
            .inventory
            .slots
            .iter()
            .filter(|stack| water(stack, &state))
            .count(),
        1
    );
}

#[test]
fn bottle_cauldron_fill_and_empty_change_one_level_and_one_held_item() {
    let (mut state, actor, pos) = fixture(None, 1);
    let two = named(&state, "water_cauldron", 2);
    let one = named(&state, "water_cauldron", 1);
    place(&mut state, pos, two);
    let (out, _, _) = run_tick(&mut state, &[click(pos)]);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(one));
    assert!(water(&state.players[&actor].inventory.slots[0], &state));
    assert!(out.changes.entity_events.contains(&EntityEvent::Bottle {
        entity: actor,
        empty: false,
        cauldron: true
    }));
    let (out, _, _) = run_tick(&mut state, &[click(pos)]);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(two));
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        ItemStack::new(item_id(&state, "glass_bottle"), 1)
    );
    assert!(out.changes.entity_events.contains(&EntityEvent::Bottle {
        entity: actor,
        empty: true,
        cauldron: true
    }));
    let empty = named(&state, "cauldron", 0);
    place(&mut state, pos, empty);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = potion(&state, "water");
    run_tick(&mut state, &[click(pos)]);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(one));
    run_tick(&mut state, &[click(pos)]);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(empty));
    assert!(water(&state.players[&actor].inventory.slots[0], &state));
}

#[test]
fn bottle_cauldron_full_empty_and_nonwater_refusals_pay_nothing() {
    for (name, level, held) in [
        ("cauldron", 0, "glass_bottle"),
        ("water_cauldron", 3, "water"),
        ("cauldron", 0, "awkward"),
        ("lava_cauldron", 0, "glass_bottle"),
    ] {
        let (mut state, actor, pos) = fixture(None, 1);
        let old = named(&state, name, level);
        place(&mut state, pos, old);
        if held != "glass_bottle" {
            state.players.get_mut(&actor).unwrap().inventory.slots[0] = potion(&state, held);
        }
        let before = state.players[&actor].inventory.slots.clone();
        let (out, _, _) = run_tick(&mut state, &[click(pos)]);
        assert_eq!(state.players[&actor].inventory.slots, before);
        assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(old));
        assert!(
            !out.changes
                .entity_events
                .iter()
                .any(|event| matches!(event, EntityEvent::Bottle { .. }))
        );
    }
}

#[test]
fn bottle_water_potion_result_survives_player_wal_and_native_checkpoint() {
    let (mut state, actor, _) = fixture(Some(0), 16);
    let (out, _, _) = run_tick(&mut state, &[use_item(Hand::Main, 90.0)]);
    let record = crate::core::player_record_for(&state, actor, state.tick).unwrap();
    let entries = crate::core::journal_entries_for_tick(&mut state, &out);
    let uuid = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid
        .to_string();
    let replay = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &[(1, out.tick, entries)],
    );
    assert_eq!(replay.players[&uuid].inventory, record.inventory);
    assert!(
        replay.players[&uuid]
            .inventory
            .iter()
            .any(|stack| water(stack, &state))
    );
    let dir = std::env::temp_dir().join(format!("solaris-bottles-{}", uuid::Uuid::new_v4()));
    let config = crate::CoreConfig::new(dir.clone(), 42);
    let restored = crate::native_checkpoint::tests::roundtrip_player(
        &config,
        &uuid,
        &record,
        &state.registry.items,
    )
    .unwrap();
    assert_eq!(restored.inventory, record.inventory);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_water_filling_consumes_only_unreserved_bottles() {
    let (mut state, actor, pos) = fixture(Some(0), 2);
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "glass_bottle",
        1,
    );
    let mut changes = ChangeSet::default();
    assert!(crate::bottles::use_item(
        &mut state,
        actor,
        Hand::Main,
        0.0,
        90.0,
        &mut changes
    ));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(
        state.players[&actor]
            .inventory
            .slots
            .iter()
            .any(|stack| water(stack, &state))
    );
    let before = state.players[&actor].inventory.slots.clone();
    let mut refused = ChangeSet::default();
    assert!(crate::bottles::use_item(
        &mut state,
        actor,
        Hand::Main,
        0.0,
        90.0,
        &mut refused
    ));
    assert_eq!(state.players[&actor].inventory.slots, before);
    assert!(refused.entity_events.is_empty());
    assert!(refused.entities_spawned.is_empty());
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(state.registry.states.water)
    );
}

#[test]
fn cauldron_exchange_floor_refusal_preserves_both_physical_owners() {
    let (mut state, actor, pos) = fixture(None, 1);
    let cauldron = named(&state, "water_cauldron", 2);
    place(&mut state, pos, cauldron);
    for resource in ["glass_bottle", "potion"] {
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = if resource == "potion" {
            potion(&state, "water")
        } else {
            ItemStack::new(item_id(&state, "glass_bottle"), 1)
        };
        crate::crafter::tests::hold(
            &mut state,
            mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
            resource,
            1,
        );
        let before = state.players[&actor].inventory.slots.clone();
        let mut refused = ChangeSet::default();
        assert!(crate::bottles::use_on(
            &mut state,
            actor,
            Hand::Main,
            pos,
            &mut refused
        ));
        assert_eq!(state.players[&actor].inventory.slots, before);
        assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(cauldron));
        assert!(refused.entity_events.is_empty());
        assert!(refused.entities_spawned.is_empty());
        crate::plugin_inventories::invalidate_player(&mut state, actor);
    }
}
