//! Session authentication is enforced before native or plugin gameplay dispatch.
use super::*;
use crate::boundary::ClickMode;

const SESSION: ConnectionId = ConnectionId(1);

fn fixture(required: bool) -> (TickState, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    state.require_plugin_login = required;
    let mut changes = ChangeSet::default();
    input::session::join(
        &mut state,
        (
            &SESSION,
            &uuid::Uuid::from_u128(1),
            &"AuthPlayer".into(),
            &0,
        ),
        &mut changes,
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let entity = state.connections.entity_of(SESSION).unwrap();
    state.players.get_mut(&entity).unwrap().awaiting_teleport = None;
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [8.5, 81.0, 8.5]);
    state.world.set_block(8, 80, 9, state.registry.states.stone);
    let item_id = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = mc_data::ItemStack {
        item_id,
        count: 8,
        ..Default::default()
    };
    state.outbox.get_mut(&SESSION).unwrap().clear();
    (state, entity)
}

fn apply(state: &mut TickState, inputs: &[PlayerInput]) -> (Vec<Intent>, ChangeSet) {
    let mut intents = Vec::new();
    let mut changes = ChangeSet::default();
    apply_inputs(state, inputs, &mut intents, &mut changes);
    (intents, changes)
}

#[test]
fn auth_gate_new_session_is_locked_and_leave_preserves_inventory() {
    let (mut state, entity) = fixture(true);
    assert!(state.session_locked(SESSION));
    let inventory = state.players[&entity].inventory.clone();
    apply(
        &mut state,
        &[PlayerInput::DropSelected {
            connection: SESSION,
            whole_stack: true,
        }],
    );
    apply(
        &mut state,
        &[PlayerInput::Leave {
            connection: SESSION,
        }],
    );
    assert_eq!(
        state.offline_players[&uuid::Uuid::from_u128(1)].inventory,
        inventory.slots
    );
    assert!(!state.locked_sessions.contains(&SESSION));
}

#[test]
fn auth_gate_locked_restore_never_spills_cursor_and_grid() {
    for grid_slots in [0, 4, 9] {
        let (mut state, entity) = fixture(false);
        let player = state.players.get_mut(&entity).unwrap();
        let stack = player.inventory.slots[0].clone();
        player.inventory.slots.fill(mc_data::ItemStack {
            count: 64,
            ..stack.clone()
        });
        player.inventory_menu.carried = stack.clone();
        // Model a non-crafting container, player grid, or crafting-table grid.
        // Capture before a normal close could merge or drop this custody.
        let mut record = crate::core::player_record_for(&state, entity, 1).unwrap();
        record.grid = vec![mc_data::ItemStack::EMPTY; grid_slots];
        if let Some(last) = record.grid.last_mut() {
            *last = stack;
        }
        let slot = state.entities.slot_of(entity).unwrap();
        state.connections.remove_by_connection(SESSION);
        state.remove_entity(slot);
        state
            .offline_players
            .insert(uuid::Uuid::from_u128(1), record.clone());
        state.require_plugin_login = true;
        let join = PlayerInput::Join {
            connection: SESSION,
            uuid: uuid::Uuid::from_u128(1),
            name: "AuthPlayer".into(),
            permission_level: 0,
        };
        let (_, changes) = apply(&mut state, std::slice::from_ref(&join));
        assert_eq!(
            changes.entities_spawned.len(),
            1,
            "only player, no inventory spill"
        );
        let (_, changes) = apply(
            &mut state,
            &[PlayerInput::Leave {
                connection: SESSION,
            }],
        );
        assert!(
            changes.entities_spawned.is_empty(),
            "locked disconnect must not spill"
        );
        let restored = &state.offline_players[&uuid::Uuid::from_u128(1)];
        assert_eq!(restored.inventory, record.inventory);
        assert_eq!(restored.carried, record.carried);
        assert_eq!(&restored.grid[..grid_slots], record.grid);
        assert!(
            restored.grid[grid_slots..]
                .iter()
                .all(mc_data::ItemStack::is_empty)
        );
        apply(&mut state, &[join]);
        unlock_session(&mut state, SESSION);
        let entity = state.connections.entity_of(SESSION).unwrap();
        assert_eq!(state.players[&entity].inventory_menu.grid.len(), 4);
        assert!(state.players[&entity].inventory_menu.carried.is_empty());
        assert_eq!(
            state.plugin_spawned.len(),
            if grid_slots == 0 { 1 } else { 2 },
            "overflow only becomes public after authentication"
        );
    }
}

#[test]
fn auth_gate_locked_gameplay_has_no_native_side_effects() {
    let (mut state, entity) = fixture(true);
    // Mark explicitly as well so this regression isolates admission from Join.
    state.locked_sessions.insert(SESSION);
    let target = BlockPos { x: 8, y: 80, z: 9 };
    let mob_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let victim = state.spawn_mob(mob_type, [10.0, 81.0, 8.5], 20.0);
    let inputs = [
        PlayerInput::ContainerClick {
            connection: SESSION,
            window: 0,
            state_id: 0,
            slot: 36,
            button: 0,
            mode: ClickMode::Pickup,
        },
        PlayerInput::DropSelected {
            connection: SESSION,
            whole_stack: true,
        },
        PlayerInput::Dig {
            connection: SESSION,
            position: target,
            action: DigAction::Start,
            sequence: 1,
        },
        PlayerInput::Dig {
            connection: SESSION,
            position: target,
            action: DigAction::Finish,
            sequence: 2,
        },
        PlayerInput::UseItemOn {
            connection: SESSION,
            position: target,
            hand: Hand::Main,
            face: 1,
            cursor: [0.5; 3],
            sequence: 3,
        },
        PlayerInput::Move {
            connection: SESSION,
            position: [9.0, 81.0, 8.5],
            yaw: 30.0,
            pitch: 10.0,
            on_ground: true,
        },
        PlayerInput::Attack {
            connection: SESSION,
            target: victim,
        },
        PlayerInput::SelectTrade {
            connection: SESSION,
            offer: 1,
        },
        PlayerInput::UseItem {
            connection: SESSION,
            hand: Hand::Main,
            yaw: 0.0,
            pitch: 0.0,
            sequence: 4,
        },
        PlayerInput::SelectSlot {
            connection: SESSION,
            slot: 1,
        },
        PlayerInput::Chat {
            connection: SESSION,
            message: "not public".into(),
        },
        PlayerInput::Command {
            connection: SESSION,
            command: "gamemode creative".into(),
            permission_level: 4,
        },
    ];
    for input in inputs {
        let before = state.players[&entity].clone();
        let slot = state.entities.slot_of(entity).unwrap();
        let position = state.entities.pos(slot);
        let (intents, changes) = apply(&mut state, std::slice::from_ref(&input));
        assert!(intents.is_empty(), "locked input emitted intent: {input:?}");
        assert!(
            changes.entities_spawned.is_empty(),
            "locked input dropped items: {input:?}"
        );
        assert_eq!(
            state.entities.pos(slot),
            position,
            "locked input moved: {input:?}"
        );
        let after = &state.players[&entity];
        assert_eq!(
            after.inventory, before.inventory,
            "locked inventory edit: {input:?}"
        );
        assert_eq!(after.game_mode, before.game_mode);
        assert_eq!(after.vitals, before.vitals);
        assert_eq!(after.open, before.open);
    }
    assert!(!state.outbox[&SESSION].iter().any(
        |change| matches!(change, PlayerChange::Chat { message } if message.contains("not public"))
    ));
}

#[test]
fn auth_gate_config_off_and_unlock_restore_inventory_and_movement_inputs() {
    for required in [false, true] {
        let (mut state, entity) = fixture(required);
        // The SDK journal command tests cover removal through native plugin admission.
        if required {
            unlock_session(&mut state, SESSION);
            assert!(!state.session_locked(SESSION));
        }
        let slot = state.entities.slot_of(entity).unwrap();
        apply(
            &mut state,
            &[PlayerInput::Move {
                connection: SESSION,
                position: [9.0, 81.0, 8.5],
                yaw: 0.0,
                pitch: 0.0,
                on_ground: true,
            }],
        );
        assert_eq!(state.entities.pos(slot), [9.0, 81.0, 8.5]);
        let block = BlockPos { x: 8, y: 80, z: 9 };
        let (intents, _) = apply(
            &mut state,
            &[PlayerInput::Dig {
                connection: SESSION,
                position: block,
                action: DigAction::Finish,
                sequence: 1,
            }],
        );
        assert!(!intents.is_empty(), "dig resumes after login");
        let (intents, _) = apply(
            &mut state,
            &[PlayerInput::UseItemOn {
                connection: SESSION,
                position: block,
                hand: Hand::Main,
                face: 1,
                cursor: [0.5; 3],
                sequence: 2,
            }],
        );
        assert!(!intents.is_empty(), "placement resumes after login");
        let mob_type = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
            .unwrap();
        let victim = state.spawn_mob(mob_type, [10.0, 81.0, 8.5], 20.0);
        let (intents, _) = apply(
            &mut state,
            &[PlayerInput::Attack {
                connection: SESSION,
                target: victim,
            }],
        );
        assert!(!intents.is_empty(), "attacks resume after login");
        let click = |state_id| PlayerInput::ContainerClick {
            connection: SESSION,
            window: 0,
            state_id,
            slot: 36,
            button: 0,
            mode: ClickMode::Pickup,
        };
        let revision = state.players[&entity].inventory_menu.state_id;
        apply(&mut state, &[click(revision)]);
        assert!(state.players[&entity].inventory.slots[0].is_empty());
        let revision = state.players[&entity].inventory_menu.state_id;
        apply(&mut state, &[click(revision)]);
        assert!(!state.players[&entity].inventory.slots[0].is_empty());
        let (_, changes) = apply(
            &mut state,
            &[PlayerInput::DropSelected {
                connection: SESSION,
                whole_stack: true,
            }],
        );
        assert!(state.players[&entity].inventory.slots[0].is_empty());
        assert_eq!(changes.entities_spawned.len(), 1);
    }
}

#[test]
fn auth_gate_locked_movement_corrects_once_and_never_ping_pongs() {
    // Regression: every locked Move produced a fresh teleport; the client answers
    // each teleport with accept + full pose, so the pair flooded the connection
    // until the client's outbound queue overflowed and it dropped the session.
    let (mut state, entity) = fixture(true);
    let slot = state.entities.slot_of(entity).unwrap();
    let home = state.entities.pos(slot);
    let corrections = |state: &TickState| {
        state.outbox[&SESSION]
            .iter()
            .filter(|change| matches!(change, PlayerChange::PositionCorrected { .. }))
            .count()
    };
    let moved = |position: [f64; 3]| PlayerInput::Move {
        connection: SESSION,
        position,
        yaw: 30.0,
        pitch: 10.0,
        on_ground: true,
    };
    // A genuine displacement is corrected once; further poses before the client
    // confirms are ignored, exactly like unlocked movement while a teleport is pending.
    apply(&mut state, &[moved([9.0, 81.0, 8.5])]);
    apply(
        &mut state,
        &[moved([9.5, 81.0, 8.5]), moved([10.0, 81.0, 8.5])],
    );
    assert_eq!(corrections(&state), 1);
    let teleport_id = state.players[&entity].awaiting_teleport.unwrap();
    apply(
        &mut state,
        &[PlayerInput::ConfirmTeleport {
            connection: SESSION,
            teleport_id,
        }],
    );
    // The client echoes the corrected pose, rotation-only and status packets
    // follow: nothing diverged, so nothing is sent back.
    apply(
        &mut state,
        &[
            moved(home),
            PlayerInput::MoveRotation {
                connection: SESSION,
                yaw: 90.0,
                pitch: 0.0,
                on_ground: true,
            },
            PlayerInput::MoveStatus {
                connection: SESSION,
                on_ground: true,
            },
        ],
    );
    assert_eq!(
        corrections(&state),
        1,
        "settled client must not be re-teleported"
    );
    assert_eq!(state.entities.pos(slot), home);
    // A later real divergence is fenced again.
    apply(&mut state, &[moved([9.0, 81.0, 8.5])]);
    assert_eq!(corrections(&state), 2);
}

#[tokio::test]
async fn auth_gate_only_declared_login_roots_reach_plugin_without_echo() {
    let (mut state, _) = tokio::task::spawn_blocking(|| fixture(true)).await.unwrap();
    state.locked_sessions.insert(SESSION);
    let capacity = std::num::NonZeroUsize::new(16).unwrap();
    let (boundary, mut endpoint) = mc_script::script_boundary_pair(capacity, capacity);
    let manifest = mc_script::ScriptPluginManifest::new(
        "auth",
        "Auth",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("login")
    .declare_player_command_root("register")
    .declare_player_command_root("changepassword")
    .declare_player_command_root("trade")
    .subscribe_event("player.chat")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    state.plugin_runtime = Some(crate::plugin::PluginRuntime::with_test_boundary(
        boundary.clone(),
    ));
    for command in [
        "login secret",
        "/register secret secret",
        "changepassword old new",
        "trade secret",
        "say secret",
    ] {
        apply(
            &mut state,
            &[PlayerInput::Command {
                connection: SESSION,
                command: command.into(),
                permission_level: 0,
            }],
        );
    }
    apply(
        &mut state,
        &[PlayerInput::Chat {
            connection: SESSION,
            message: "secret chat".into(),
        }],
    );
    crate::plugin::seal_events(&mut state, 1).unwrap();
    crate::plugin::acknowledge_events(&mut state, 1);
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 1 }
    ));
    for expected in ["login", "register", "changepassword"] {
        let event = endpoint.recv_event().await.unwrap();
        assert!(
            matches!(event.kind(), mc_script::ScriptEventKind::PlayerCommand { root, .. } if root == expected)
        );
    }
    boundary
        .try_enqueue_event(mc_script::ScriptEvent::server_started())
        .unwrap();
    assert!(
        matches!(
            endpoint.recv_event().await.unwrap().kind(),
            mc_script::ScriptEventKind::ServerStarted
        ),
        "non-login plugin commands and chat must not reach the plugin"
    );
    assert!(state.outbox[&SESSION].iter().all(
        |change| !matches!(change, PlayerChange::Chat { message } if message.contains("secret"))
    ));
}

#[test]
fn auth_gate_blocks_ambient_damage_and_item_custody_until_unlock() {
    let (mut state, entity) = fixture(true);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    let position = state.entities.pos(slot);
    run_tick(&mut state, &[]);
    assert_eq!(
        state.entities.pos(slot),
        position,
        "locked position stays fixed"
    );
    let hit = || DamagePiece {
        physical: None,
        custom_armor: false,
        amount: 2.0,
        kind: crate::survival::DamageKind::Melee,
        source: None,
    };
    for kind in [
        crate::survival::DamageKind::Melee,
        crate::survival::DamageKind::Lava,
        crate::survival::DamageKind::Fall,
        crate::survival::DamageKind::Drowning,
    ] {
        state
            .merge_damage
            .push((slot, DamagePiece { kind, ..hit() }));
    }
    let inventory = state.players[&entity].inventory.clone();
    let mut stack = inventory.slots[0].clone();
    stack.count = 3;
    let item = spawn_item_entity(&mut state, position, stack);
    let item_slot = state.entities.slot_of(item).unwrap();
    state.merge_pickups.push((item_slot, slot, 3));
    let (deaths, _, _, _) = state.commit_merge_effects();
    assert!(deaths.is_empty());
    assert_eq!(state.players[&entity].vitals.health, 20.0);
    assert_eq!(state.players[&entity].inventory, inventory);
    assert!(state.entities.slot_of(item).is_some());
    unlock_session(&mut state, SESSION);
    state.merge_damage.push((slot, hit()));
    state.merge_pickups.push((item_slot, slot, 3));
    state.commit_merge_effects();
    assert!(state.players[&entity].vitals.health < 20.0);
    assert_eq!(state.players[&entity].inventory.slots[0].count, 11);
    assert!(state.entities.slot_of(item).is_none());
}
