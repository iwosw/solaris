use super::*;
#[test]
fn all_native_frequency_groups_and_resonances_match_table() {
    for (event, freq) in [
        (GameEvent::Step, 1),
        (GameEvent::ProjectileLand, 2),
        (GameEvent::ProjectileShoot, 3),
        (GameEvent::EntityAction, 4),
        (GameEvent::Equip, 5),
        (GameEvent::EntityInteract, 6),
        (GameEvent::EntityDamage, 7),
        (GameEvent::Eat, 8),
        (GameEvent::ContainerClose, 9),
        (GameEvent::ContainerOpen, 10),
        (GameEvent::BlockChange, 11),
        (GameEvent::BlockDestroy, 12),
        (GameEvent::BlockPlace, 13),
        (GameEvent::EntityPlace, 14),
        (GameEvent::EntityDie, 15),
    ] {
        assert_eq!(mc_data::game_events::frequency(event.name()), freq);
    }
    for n in 1..=15 {
        let event = GameEvent::Resonate(n);
        assert_eq!(GameEvent::parse(event.name()), Some(event));
        assert_eq!(mc_data::game_events::frequency(event.name()), n);
    }
    assert_eq!(mc_data::game_events::frequency("resonate_16"), 0);
}
#[test]
fn listener_codec_uses_native_types_and_preserves_source_uuid_and_travel_delay() {
    let uuid = uuid::Uuid::from_u128(0x123456789abcdef);
    let p = Pending {
        vibration: Vibration {
            event: GameEvent::BlockPlace,
            pos: [1.5, 201.0, 2.5],
            source: Some(EntityId(9)),
            frequency: 13,
            distance: 4.0,
            source_uuid: Some(uuid),
            projectile_owner_uuid: Some(uuid),
        },
        arrival: 104,
        selected: 100,
    };
    let tag = listener_nbt(Some(&p), 101);
    let decoded = decode_listener(&tag, 101).unwrap().unwrap();
    assert_eq!(decoded.arrival, 104);
    assert_eq!(decoded.vibration.source_uuid, Some(uuid));
    assert_eq!(decoded.vibration.projectile_owner_uuid, Some(uuid));
    assert_eq!(decoded.vibration.event, p.vibration.event);
    assert_eq!(decoded.vibration.pos, p.vibration.pos);
    let Tag::Compound(f) = tag else { panic!() };
    assert!(matches!(field(&f, "event_delay"), Some(Tag::Int(3))));
    let Some(Tag::Compound(event)) = field(&f, "event") else {
        panic!()
    };
    assert!(matches!(field(event, "distance"), Some(Tag::Float(4.0))));
    assert!(matches!(field(event,"source"),Some(Tag::IntArray(words)) if words.len()==4));
    assert!(
        decode_listener(&listener_nbt(None, 0), 0)
            .unwrap()
            .is_none()
    );
}
#[test]
fn corrupt_listener_delay_and_selector_fail_closed() {
    let tag = Tag::Compound(vec![
        ("event_delay".into(), Tag::Int(-1)),
        (
            "selector".into(),
            Tag::Compound(vec![("tick".into(), Tag::Long(-1))]),
        ),
    ]);
    assert_eq!(
        decode_listener(&tag, 0).unwrap_err(),
        "listener.event_delay must be a nonnegative int"
    );
    assert_eq!(
        decode_listener(&Tag::Compound(vec![]), 0).unwrap_err(),
        "listener.selector must be a compound"
    );
}

fn avoiding_player() -> (TickState, EntityId) {
    let mut state = crate::bees::tests::state(None);
    let player = crate::bees::tests::joined(&mut state);
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    let mut tree = mc_data::advancements::AdvancementTree::default();
    tree.definitions.insert(
        "test:avoid".into(),
        serde_json::from_value(serde_json::json!({
            "criteria": {"avoid": {"trigger": "minecraft:avoid_vibration"}},
            "requirements": [["avoid"]],
        }))
        .unwrap(),
    );
    registry.advancements = std::sync::Arc::new(tree);
    state.registry = std::sync::Arc::new(registry);
    let slot = state.entities.slot_of(player).unwrap();
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags |= 2;
    (state, player)
}

#[test]
fn avoiding_vibration_requires_real_sensor_or_warden_not_isolated_sneaking() {
    let (mut state, player) = avoiding_player();
    let pos = [4.5, 201.0, 6.5];
    emit(&mut state, pos, GameEvent::Step, Some(player));
    assert!(state.advancement_events.is_empty());
    let sensor = crate::bees::tests::POS;
    crate::bees::tests::place(&mut state, sensor, "minecraft:sculk_sensor");
    crate::sculk::changed(&mut state, sensor);
    emit(&mut state, pos, GameEvent::Step, Some(player));
    assert_eq!(state.advancement_events.len(), 1);
    assert_eq!(state.advancement_events[0].entity, player);
    assert_eq!(
        state.advancement_events[0].trigger,
        "minecraft:avoid_vibration"
    );
    assert!(pending(&state, sensor).is_none());
    state.advancement_events.clear();
    emit(
        &mut state,
        [30.5, 201.0, 6.5],
        GameEvent::Step,
        Some(player),
    );
    assert!(state.advancement_events.is_empty());
    crate::bees::tests::place(&mut state, sensor, "minecraft:sculk_shrieker");
    crate::sculk::removed(&mut state, sensor);
    crate::shrieker::changed(&mut state, sensor);
    emit(&mut state, pos, GameEvent::Step, Some(player));
    assert!(state.advancement_events.is_empty());
    crate::bees::tests::mob(&mut state, "minecraft:warden", [5.5, 201.0, 6.5]);
    emit(&mut state, pos, GameEvent::Step, Some(player));
    assert_eq!(state.advancement_events.len(), 1);
}

#[test]
fn nonsuppressed_events_spectators_and_busy_sensors_do_not_award_avoidance() {
    let (mut state, player) = avoiding_player();
    let sensor = crate::bees::tests::POS;
    crate::bees::tests::place(&mut state, sensor, "minecraft:sculk_sensor");
    crate::sculk::changed(&mut state, sensor);
    emit(&mut state, [4.5, 201.0, 6.5], GameEvent::Eat, Some(player));
    assert!(state.advancement_events.is_empty());
    state.tick += 1;
    emit(&mut state, [4.5, 201.0, 6.5], GameEvent::Step, Some(player));
    assert!(state.advancement_events.is_empty());
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Spectator;
    emit(&mut state, [4.5, 201.0, 6.5], GameEvent::Step, Some(player));
    assert!(state.advancement_events.is_empty());
}
