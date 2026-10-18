use super::{flat_player_fixture, run_tick};
use crate::boundary::{BlockPos, ClickMode, ConnectionId, Hand, PlayerChange, PlayerInput};
use crate::inventory::MenuKind;
use mc_data::{Identifier, ItemStack};

fn stack(state: &crate::tick::TickState, item: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(format!("minecraft:{item}")).unwrap())
            .unwrap(),
        count,
    )
}
fn inputs(state: &crate::tick::TickState, kind: MenuKind) -> Vec<ItemStack> {
    match kind {
        MenuKind::Smithing => vec![
            stack(state, "netherite_upgrade_smithing_template", 2),
            stack(state, "diamond_sword", 1),
            stack(state, "netherite_ingot", 2),
        ],
        MenuKind::Loom => vec![
            stack(state, "white_banner", 2),
            stack(state, "red_dye", 2),
            ItemStack::EMPTY,
        ],
        MenuKind::Stonecutter => vec![stack(state, "stone", 2)],
        _ => unreachable!(),
    }
}
fn open(
    state: &mut crate::tick::TickState,
    entity: crate::boundary::EntityId,
    pos: [f64; 3],
    block: &str,
    menu_type: u32,
) {
    let target = BlockPos {
        x: pos[0] as i32 + 2,
        y: pos[1] as i32,
        z: pos[2] as i32,
    };
    let block_id = state
        .registry
        .blocks
        .states()
        .find(|entry| entry.block.id.as_str() == block)
        .unwrap()
        .id
        .0;
    state
        .world
        .set_block(target.x, target.y, target.z, block_id);
    let (_, drains, _) = run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: target,
            face: 1,
            cursor: [0.5, 1.0, 0.5],
            sequence: 1,
        }],
    );
    assert!(drains.iter().flat_map(|(_, changes)| changes).any(|change| matches!(change, PlayerChange::MenuOpened { menu_type: found, .. } if *found == menu_type)));
    assert!(state.players[&entity].open.is_some());
    assert!(
        !state
            .block_entities
            .contains_key(&(target.x, target.y, target.z))
    );
}

#[test]
fn native_use_buttons_and_shift_click_commit_workstation_results() {
    for (block, kind, menu_type) in [
        ("minecraft:smithing_table", MenuKind::Smithing, 21),
        ("minecraft:loom", MenuKind::Loom, 18),
        ("minecraft:stonecutter", MenuKind::Stonecutter, 24),
    ] {
        let (mut state, entity, pos) = flat_player_fixture();
        open(&mut state, entity, pos, block, menu_type);
        let stat = match kind {
            MenuKind::Smithing => mc_data::statistics::CustomStat::InteractWithSmithingTable,
            MenuKind::Loom => mc_data::statistics::CustomStat::InteractWithLoom,
            MenuKind::Stonecutter => mc_data::statistics::CustomStat::InteractWithStonecutter,
            _ => unreachable!(),
        };
        assert_eq!(state.players[&entity].statistics.get_custom(stat), 1);
        let inputs = inputs(&state, kind);
        let window = state.players[&entity].open.as_ref().unwrap().window;
        // Inputs enter through native inventory clicks, not client result patches.
        let start = inputs.len() + 1;
        for (index, input) in inputs
            .iter()
            .enumerate()
            .filter(|(_, input)| !input.is_empty())
        {
            state.players.get_mut(&entity).unwrap().inventory.slots[9] = input.clone();
            let state_id = state.players[&entity].open.as_ref().unwrap().state_id;
            run_tick(
                &mut state,
                &[PlayerInput::ContainerClick {
                    connection: ConnectionId(1),
                    window,
                    state_id,
                    slot: start as i16,
                    button: 0,
                    mode: ClickMode::Pickup,
                }],
            );
            let state_id = state.players[&entity].open.as_ref().unwrap().state_id;
            run_tick(
                &mut state,
                &[PlayerInput::ContainerClick {
                    connection: ConnectionId(1),
                    window,
                    state_id,
                    slot: index as i16,
                    button: 0,
                    mode: ClickMode::Pickup,
                }],
            );
        }
        if kind != MenuKind::Smithing {
            run_tick(
                &mut state,
                &[PlayerInput::ContainerButton {
                    connection: ConnectionId(1),
                    window,
                    button: 0,
                }],
            );
        }
        let menu = state.players[&entity].open.as_ref().unwrap();
        let output = menu.result.clone();
        assert!(!output.is_empty(), "{kind:?}");
        let state_id = menu.state_id;
        run_tick(
            &mut state,
            &[PlayerInput::ContainerClick {
                connection: ConnectionId(1),
                window,
                state_id,
                slot: inputs.len() as i16,
                button: 0,
                mode: ClickMode::QuickMove,
            }],
        );
        let player = &state.players[&entity];
        let produced: i32 = player
            .inventory
            .slots
            .iter()
            .filter(|stack| {
                !stack.is_empty() && mc_data::inventory_semantics_26_1_2::can_stack(stack, &output)
            })
            .map(|stack| stack.count)
            .sum();
        assert_eq!(
            produced,
            output.count * if kind == MenuKind::Smithing { 1 } else { 2 }
        );
        let menu = player.open.as_ref().unwrap();
        assert!(menu.result.is_empty());
        if kind == MenuKind::Smithing {
            assert_eq!(menu.grid[0].count, 1);
            assert_eq!(menu.grid[2].count, 1);
        } else {
            assert!(menu.grid[0].is_empty());
        }
    }
}

#[test]
fn station_disconnect_returns_inputs_once_and_rejoin_never_refunds_projection() {
    for (block, kind, menu_type) in [
        ("minecraft:smithing_table", MenuKind::Smithing, 21),
        ("minecraft:loom", MenuKind::Loom, 18),
        ("minecraft:stonecutter", MenuKind::Stonecutter, 24),
    ] {
        let (mut state, entity, pos) = flat_player_fixture();
        open(&mut state, entity, pos, block, menu_type);
        let inputs = inputs(&state, kind);
        let cursor = stack(&state, "diamond", 3);
        let expected: i32 = inputs
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.count)
            .sum::<i32>()
            + 3;
        let registry = std::sync::Arc::clone(&state.registry);
        let player = state.players.get_mut(&entity).unwrap();
        player.inventory.slots.fill(ItemStack::EMPTY);
        let menu = player.open.as_mut().unwrap();
        menu.grid = inputs;
        menu.carried = cursor;
        player.initialize_station(&registry.enchantments, &registry.item_rules(), 0);
        let window = state.players[&entity].open.as_ref().unwrap().window;
        if kind != MenuKind::Smithing {
            run_tick(
                &mut state,
                &[PlayerInput::ContainerButton {
                    connection: ConnectionId(1),
                    window,
                    button: 0,
                }],
            );
        }
        assert!(
            !state.players[&entity]
                .open
                .as_ref()
                .unwrap()
                .result
                .is_empty()
        );
        let uuid = state.connections.uuid_of(ConnectionId(1)).unwrap();
        run_tick(
            &mut state,
            &[PlayerInput::Leave {
                connection: ConnectionId(1),
            }],
        );
        let offline = state.offline_players[&uuid].inventory.clone();
        assert!(state.offline_players[&uuid].carried.is_empty());
        assert!(
            state.offline_players[&uuid]
                .grid
                .iter()
                .all(ItemStack::is_empty)
        );
        assert_eq!(
            offline
                .iter()
                .filter(|s| !s.is_empty())
                .map(|s| s.count)
                .sum::<i32>(),
            expected
        );
        assert!(
            offline
                .iter()
                .all(|s| s.extended().banner_patterns.is_empty())
        );
        run_tick(
            &mut state,
            &[PlayerInput::Leave {
                connection: ConnectionId(1),
            }],
        );
        assert_eq!(state.offline_players[&uuid].inventory, offline);
        run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(1),
                uuid,
                name: "station-return".into(),
                permission_level: 0,
            }],
        );
        let rejoined = state.connections.entity_of(ConnectionId(1)).unwrap();
        assert_eq!(state.players[&rejoined].inventory.slots, offline);
        assert!(state.players[&rejoined].open.is_none());
    }
}

#[test]
fn station_disconnect_overflow_drops_real_custody_once_and_journals_it() {
    for (block, kind, menu_type) in [
        ("minecraft:smithing_table", MenuKind::Smithing, 21),
        ("minecraft:loom", MenuKind::Loom, 18),
        ("minecraft:stonecutter", MenuKind::Stonecutter, 24),
    ] {
        let (mut state, entity, pos) = flat_player_fixture();
        open(&mut state, entity, pos, block, menu_type);
        let inputs = inputs(&state, kind);
        let cursor = stack(&state, "diamond", 3);
        let full = stack(&state, "stone", 64);
        let expected = inputs
            .iter()
            .filter(|stack| !stack.is_empty())
            .map(|stack| stack.count)
            .sum::<i32>()
            + cursor.count;
        let registry = std::sync::Arc::clone(&state.registry);
        let player = state.players.get_mut(&entity).unwrap();
        player.inventory.slots.fill(full);
        let before = player.inventory.slots.clone();
        let menu = player.open.as_mut().unwrap();
        menu.grid = inputs;
        menu.carried = cursor;
        player.initialize_station(&registry.enchantments, &registry.item_rules(), 0);
        let window = state.players[&entity].open.as_ref().unwrap().window;
        if kind != MenuKind::Smithing {
            run_tick(
                &mut state,
                &[PlayerInput::ContainerButton {
                    connection: ConnectionId(1),
                    window,
                    button: 0,
                }],
            );
        }
        assert!(
            !state.players[&entity]
                .open
                .as_ref()
                .unwrap()
                .result
                .is_empty()
        );
        let uuid = state.connections.uuid_of(ConnectionId(1)).unwrap();
        let (output, _, _) = run_tick(
            &mut state,
            &[PlayerInput::Leave {
                connection: ConnectionId(1),
            }],
        );
        let offline = &state.offline_players[&uuid];
        assert_eq!(offline.inventory, before);
        assert!(offline.carried.is_empty());
        assert!(offline.grid.iter().all(ItemStack::is_empty));
        let drops: Vec<_> = output
            .changes
            .entities_spawned
            .iter()
            .filter_map(|id| {
                let slot = state.entities.slot_of(*id)?;
                Some((*id, state.entities.row(slot).item.as_ref()?.full_stack()))
            })
            .collect();
        assert_eq!(
            drops.iter().map(|(_, stack)| stack.count).sum::<i32>(),
            expected
        );
        assert!(
            drops
                .iter()
                .all(|(_, stack)| stack.extended().banner_patterns.is_empty())
        );
        let journal = crate::core::journal_entries_for_tick(&mut state, &output);
        for (id, stack) in &drops {
            assert!(journal.iter().any(|entry| matches!(
                entry,
                crate::journal::JournalEntry::Entity { id: written, item: Some(item), .. }
                    if *written == crate::journal::entity_id_int(*id) && item == stack
            )));
        }
        let (again, _, _) = run_tick(
            &mut state,
            &[PlayerInput::Leave {
                connection: ConnectionId(1),
            }],
        );
        assert!(again.changes.entities_spawned.is_empty());
        assert_eq!(state.offline_players[&uuid].inventory, before);
        run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(1),
                uuid,
                name: "station-overflow".into(),
                permission_level: 0,
            }],
        );
        let rejoined = state.connections.entity_of(ConnectionId(1)).unwrap();
        assert_eq!(state.players[&rejoined].inventory.slots, before);
        assert!(state.players[&rejoined].open.is_none());
    }
}
