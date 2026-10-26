use super::*;

#[test]
fn session_cleanup_has_reserved_capacity_and_preserves_fifo() {
    let (sender, mut receiver) = mpsc_channel(4);
    let connection = ConnectionId(101);
    let session = PlayerSession::admit(
        &sender,
        connection,
        uuid::Uuid::from_u128(101),
        "Session".to_owned(),
        0,
        4,
        "ru_ru".into(),
    )
    .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), PlayerInput::SetViewDistance { connection: id, .. } if id == connection)
    );
    assert!(
        matches!(receiver.try_recv().unwrap(), PlayerInput::SetLocale { connection: id, locale } if id == connection && locale == "ru_ru")
    );
    assert!(
        matches!(receiver.try_recv().unwrap(), PlayerInput::Join { connection: id, .. } if id == connection)
    );
    for _ in 0..3 {
        sender
            .try_send(PlayerInput::SelectSlot {
                connection,
                slot: 2,
            })
            .unwrap();
    }
    assert!(
        sender
            .try_send(PlayerInput::SelectSlot {
                connection,
                slot: 3
            })
            .is_err()
    );
    drop(session);
    for _ in 0..3 {
        assert!(matches!(
            receiver.try_recv().unwrap(),
            PlayerInput::SelectSlot { slot: 2, .. }
        ));
    }
    assert!(
        matches!(receiver.try_recv().unwrap(), PlayerInput::Leave { connection: id } if id == connection)
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn failed_session_admission_emits_no_partial_join() {
    let (sender, mut receiver) = mpsc_channel(4);
    sender
        .try_send(PlayerInput::SelectSlot {
            connection: ConnectionId(1),
            slot: 0,
        })
        .unwrap();
    assert!(matches!(
        PlayerSession::admit(
            &sender,
            ConnectionId(2),
            uuid::Uuid::nil(),
            "Full".into(),
            0,
            2,
            "en_us".into()
        ),
        Err(CoreError::InputFull)
    ));
    assert!(matches!(
        receiver.try_recv().unwrap(),
        PlayerInput::SelectSlot { .. }
    ));
    assert!(receiver.try_recv().is_err());
    drop(receiver);
    assert!(matches!(
        PlayerSession::admit(
            &sender,
            ConnectionId(2),
            uuid::Uuid::nil(),
            "Stopped".into(),
            0,
            2,
            "en_us".into()
        ),
        Err(CoreError::TickStopped)
    ));
}

#[test]
fn session_drop_preserves_new_same_uuid_owner() {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, world, 0, 512);
    // This base permits competing login claimants, not replacement of an
    // authenticated incumbent. Exercise that real same-UUID overlap policy.
    state.require_plugin_login = true;
    let (sender, mut receiver) = mpsc_channel(8);
    let uuid = uuid::Uuid::from_u128(101);
    let old_id = ConnectionId(101);
    let new_id = ConnectionId(102);
    let old =
        PlayerSession::admit(&sender, old_id, uuid, "Saved".into(), 0, 2, "en_us".into()).unwrap();
    let inputs: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
    run_tick(&mut state, &inputs);
    let old_entity = state.connections.entity_of(old_id).unwrap();

    let next =
        PlayerSession::admit(&sender, new_id, uuid, "Saved".into(), 0, 4, "ru_ru".into()).unwrap();
    // The new Join is already queued when the old owner finally drops.
    drop(old);
    let inputs: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
    assert_eq!(inputs.len(), 4);
    assert!(
        matches!(inputs[2], PlayerInput::Join { connection, uuid: joined, .. }
        if connection == new_id && joined == uuid)
    );
    assert!(matches!(inputs[3], PlayerInput::Leave { connection } if connection == old_id));
    let (_, joined, _) = run_tick(&mut state, &inputs[..3]);
    assert!(joined.iter().any(|(id, changes)| {
        *id == new_id
            && changes
                .iter()
                .any(|change| matches!(change, PlayerChange::Joined { .. }))
    }));
    let new_entity = state.connections.entity_of(new_id).unwrap();
    crate::tick::unlock_session(&mut state, new_id);
    assert!(!state.session_locked(new_id));
    state
        .players
        .get_mut(&new_entity)
        .unwrap()
        .vitals
        .experience
        .level = 37;

    let (_, changes, _) = run_tick(&mut state, &inputs[3..]);
    assert_eq!(state.connections.entity_of(new_id), Some(new_entity));
    assert_eq!(state.connections.uuid_of(new_id), Some(uuid));
    assert_eq!(state.connections.view_distance_of(new_id), Some(4));
    assert_eq!(state.connections.locale_of(new_id), Some("ru_ru"));
    assert!(state.entities.slot_of(new_entity).is_some());
    assert_eq!(state.players[&new_entity].vitals.experience.level, 37);
    assert!(state.connections.entity_of(old_id).is_none());
    assert!(state.entities.slot_of_any(old_entity).is_none());
    assert!(!state.offline_players.contains_key(&uuid));
    assert!(!changes.iter().any(|(id, changes)| {
        *id == new_id
            && changes
                .iter()
                .any(|change| matches!(change, PlayerChange::Disconnect { .. }))
    }));

    drop(next);
    let inputs: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
    run_tick(&mut state, &inputs);
    assert!(state.connections.entity_of(new_id).is_none());
    assert_eq!(state.offline_players[&uuid].experience_level, 37);
}

#[test]
fn session_drop_allows_immediate_rejoin_with_saved_inventory() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(dir.path().join("world"), 42);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config.input_queue_cap = 8;
    config.view_distance = 2;
    config.simulation_distance = 0;
    config.checkpoint_every_ticks = u64::MAX;
    let core = Core::open(config).unwrap();
    let mut outputs = core.subscribe();
    let connection = ConnectionId(101);
    let uuid = uuid::Uuid::from_u128(101);
    let item = core
        .registry()
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let session = core
        .join_session(connection, uuid, "Saved".into(), 4, 2, "en_us".into())
        .unwrap();
    let mut joined = false;
    for _ in 0..20 {
        outputs.blocking_recv().unwrap();
        joined |= core.try_drain_outbox().iter().any(|(id, changes)| {
            *id == connection
                && changes
                    .iter()
                    .any(|change| matches!(change, PlayerChange::Joined { .. }))
        });
        if joined {
            break;
        }
    }
    assert!(joined);
    assert!(core.submit(PlayerInput::Command {
        connection,
        command: "gamemode creative".into(),
        permission_level: 4
    }));
    // Commands cross the shared dimension boundary after ordinary input. Wait
    // for committed creative mode before sending a creative-only inventory edit.
    let mut creative = false;
    for _ in 0..20 {
        outputs.blocking_recv().unwrap();
        creative |= core.try_drain_outbox().iter().any(|(id, changes)| {
            *id == connection
                && changes
                    .iter()
                    .any(|change| matches!(change, PlayerChange::GameModeChanged { game_mode: 1 }))
        });
        if creative {
            break;
        }
    }
    assert!(creative, "creative mode must commit before inventory setup");
    assert!(core.submit(PlayerInput::CreativeSlot {
        connection,
        slot: 36,
        stack: mc_data::ItemStack::new(item, 7)
    }));
    let mut given = false;
    for _ in 0..20 {
        outputs.blocking_recv().unwrap();
        given |= core.try_drain_outbox().iter().any(|(id, changes)| {
            *id == connection
                && changes.iter().any(|change| match change {
                    PlayerChange::MenuSlot {
                        slot: 36,
                        item: Some(stack),
                        ..
                    } => stack.item_id == item && stack.count == 7,
                    PlayerChange::MenuContents { slots, .. } => slots
                        .iter()
                        .flatten()
                        .any(|stack| stack.item_id == item && stack.count == 7),
                    _ => false,
                })
        });
        if given {
            break;
        }
    }
    assert!(given, "inventory setup must commit before disconnect");
    let ordinary_credit = core
        .inner
        .input_tx
        .try_reserve_many(core.inner.input_tx.capacity())
        .unwrap();
    assert!(!core.submit(PlayerInput::SelectSlot {
        connection,
        slot: 3
    }));
    drop(session);
    drop(ordinary_credit);
    let rejoined = ConnectionId(102);
    let next = core
        .join_session(rejoined, uuid, "Saved".into(), 4, 2, "en_us".into())
        .unwrap();
    let mut restored = false;
    for _ in 0..20 {
        outputs.blocking_recv().unwrap();
        for (id, changes) in core.try_drain_outbox() {
            if id == rejoined {
                assert!(
                    !changes
                        .iter()
                        .any(|change| matches!(change, PlayerChange::Disconnect { .. })),
                    "old session must not block rejoin"
                );
                restored |= changes.iter().any(|change| matches!(change, PlayerChange::MenuContents { slots, .. } if slots.iter().flatten().any(|stack| stack.item_id == item && stack.count == 7)));
            }
        }
        if restored {
            break;
        }
    }
    assert!(
        restored,
        "the real native rejoin restores the old inventory"
    );
    drop(next);
    core.shutdown();
}
