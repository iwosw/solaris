use super::*;

#[test]
fn event_is_a_validated_authoritative_player_snapshot() {
    let context = ScriptPlayerContext::new(
        "123e4567-e89b-12d3-a456-426614174000",
        "kaiser",
        true,
        12.25,
        70.0,
        -4.5,
    );
    let event = ScriptEvent::try_player_died_with_context(
        ScriptPlayerId::new(42),
        context.clone(),
        "minecraft:the_nether",
        ScriptGameMode::Adventure,
    )
    .unwrap();

    assert_eq!(event.event_name(), "player.died");
    assert_eq!(event.target_plugin_id(), None);
    assert_eq!(event.validate(), Ok(()));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerDied {
            player_id,
            context: event_context,
            dimension,
            game_mode,
        } if *player_id == ScriptPlayerId::new(42)
            && event_context == &context
            && dimension == "minecraft:the_nether"
            && *game_mode == ScriptGameMode::Adventure
    ));
}

#[test]
fn event_rejects_invalid_context_and_dimension() {
    let valid = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
    assert!(
        ScriptEvent::try_player_died_with_context(
            ScriptPlayerId::new(42),
            valid,
            "overworld",
            ScriptGameMode::Survival,
        )
        .is_err()
    );

    assert!(ScriptPlayerContext::try_new("player-42", "", false, 0.0, 64.0, 0.0).is_err());
}

#[test]
fn grave_observation_is_targeted_and_validated_against_its_own_bounds() {
    let context = ScriptPlayerContext::new(
        "123e4567-e89b-12d3-a456-426614174000",
        "kaiser",
        false,
        12.25,
        70.0,
        -4.5,
    );
    let event = ScriptEvent::try_player_grave_created_with_context(
        "graves",
        ScriptPlayerId::new(42),
        context.clone(),
        "minecraft:overworld",
        -3,
        70,
        8,
        5,
        2,
    )
    .unwrap();
    assert_eq!(event.event_name(), "player.grave_created");
    assert_eq!(event.target_plugin_id(), Some("graves"));
    assert_eq!(event.validate(), Ok(()));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerGraveCreated {
            player_id,
            context: event_context,
            dimension,
            x,
            y,
            z,
            items,
            spilled,
        } if *player_id == ScriptPlayerId::new(42)
            && event_context == &context
            && dimension == "minecraft:overworld"
            && (*x, *y, *z) == (-3, 70, 8)
            && *items == 5
            && *spilled == 2
    ));
}

#[test]
fn grave_observation_rejects_a_dimension_code_and_unbounded_counts() {
    let context = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
    let build = |dimension: &str, items: u32| {
        ScriptEvent::try_player_grave_created_with_context(
            "graves",
            ScriptPlayerId::new(42),
            context.clone(),
            dimension,
            0,
            64,
            0,
            items,
            0,
        )
    };
    assert!(build("overworld", 1).is_err());
    assert!(build("minecraft:overworld", 4096).is_err());
    assert!(build("minecraft:overworld", 1).is_ok());
}
