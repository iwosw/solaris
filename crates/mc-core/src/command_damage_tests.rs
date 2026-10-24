use super::*;
use crate::boundary::{EntityEvent, PlayerChange};
use mc_nbt::Tag;

fn run(state: &mut TickState, actor: EntityId, text: &str) -> ChangeSet {
    let command = crate::commands::parse(text, true).unwrap();
    let mut changes = ChangeSet::default();
    crate::command_execution::execute(state, ConnectionId(1), actor, &command, &mut changes);
    changes
}

fn feedback(state: &TickState, key: &str) -> bool {
    state.outbox.get(&ConnectionId(1)).into_iter().flatten().any(|change| {
        matches!(change, PlayerChange::StyledChat { content: Tag::Compound(fields) }
            if fields.iter().any(|(name, value)| name == "translate" && matches!(value, Tag::String(value) if value == key)))
    })
}

#[test]
fn damage_command_parses_vanilla_forms_and_rejects_before_mutation() {
    for text in [
        "damage @s 0",
        "damage @p 3.5 generic",
        "damage @e[type=pig,limit=1] 2 minecraft:fall at ~1 ~ ~",
        "damage @s 2 arrow by @e[type=pig,limit=1]",
        "damage @s 2 mob_attack by @p from @s",
    ] {
        assert!(crate::commands::parse(text, true).is_ok(), "{text}");
    }
    for text in [
        "damage @a 2",
        "damage @e 2",
        "damage @s -1",
        "damage @s NaN",
        "damage @s inf",
        "damage @s 2 absent",
        "damage @s 2 mob_attack_no_aggro",
        "damage @s 2 generic by @a",
        "damage @s 2 generic by @s from @e",
        "damage @s 2 generic at ~ ^ ~",
        "damage @s 2 by @s",
    ] {
        assert!(crate::commands::parse(text, true).is_err(), "{text}");
    }
    assert!(crate::commands::parse("damage @s 1", false).is_err());
    for id in 0.. {
        let Some(name) = mc_data::required_registry_entry("damage_type", id) else {
            break;
        };
        if let Some((kind, identity)) = DamageKind::command_type(name.as_str()) {
            let source = Some(DamageSource::Command {
                connection: ConnectionId(1),
                damage_type: identity,
                direct: None,
                cause: None,
                position: None,
            });
            assert_eq!(kind.type_name(source, true), name.as_str());
            assert_eq!(kind.type_id(source, true), id as i32);
        }
    }
}

#[test]
fn damage_command_commits_default_damage_and_reports_actual_rejection() {
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    state.outbox.clear();
    let before = state.players[&actor].vitals.health;
    run(&mut state, actor, "damage @s 3");
    assert_eq!(state.players[&actor].vitals.health, before - 3.0);
    assert!(state.merge_damage.is_empty());
    assert!(feedback(&state, "commands.damage.success"));
    state.outbox.clear();
    run(&mut state, actor, "damage @s 2");
    assert_eq!(state.players[&actor].vitals.health, before - 3.0);
    assert!(feedback(&state, "commands.damage.invulnerable"));
    assert!(!feedback(&state, "commands.damage.success"));
    state.outbox.clear();
    let changes = run(&mut state, actor, "damage @s 5");
    assert_eq!(state.players[&actor].vitals.health, before - 5.0);
    assert!(feedback(&state, "commands.damage.success"));
    assert!(
        !changes
            .entity_events
            .iter()
            .any(|event| matches!(event, EntityEvent::Damage { .. }))
    );
}

#[test]
fn damage_command_retains_explicit_position_and_direct_cause_identity() {
    let (mut state, actor, centre) = crate::hostile_tests::setup(8.0);
    let actor_slot = state.entities.slot_of(actor).unwrap();
    let origin = state.entities.pos(actor_slot);
    let changes = run(&mut state, actor, "damage @s 2 fall at ~1 ~ ~");
    assert!(changes.entity_events.iter().any(|event| matches!(event,
        EntityEvent::Damage { entity, cause: None, direct: None, source_position: Some(position), .. }
        if *entity == actor && *position == [origin[0] + 1.0, origin[1], origin[2]])));
    state.tick += crate::entities::HURT_INVULNERABLE_TICKS;
    let pig_type = state.registry.entity_type_id("minecraft:pig").unwrap();
    let pig = state.spawn_mob(pig_type, centre, 10.0);
    let changes = run(
        &mut state,
        actor,
        "damage @s 1 mob_attack by @e[type=pig,limit=1] from @s",
    );
    let type_id = mc_data::required_registry_entry_id(
        "damage_type",
        &mc_data::Identifier::parse("minecraft:mob_attack").unwrap(),
    )
    .unwrap() as i32;
    assert!(changes.entity_events.iter().any(|event| matches!(event,
        EntityEvent::Damage { entity, source_type_id, cause: Some(cause), direct: Some(direct), source_position: None }
        if *entity == actor && *cause == actor && *direct == pig && *source_type_id == type_id)));
}

#[test]
fn damage_command_missing_source_does_not_stage_or_apply_damage() {
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    state.outbox.clear();
    let before = state.players[&actor].vitals.health;
    run(
        &mut state,
        actor,
        "damage @s 100 generic by @e[type=pig,limit=1]",
    );
    assert_eq!(state.players[&actor].vitals.health, before);
    assert!(state.merge_damage.is_empty());
    assert!(!feedback(&state, "commands.damage.success"));
    assert!(state.outbox[&ConnectionId(1)].iter().any(
        |change| matches!(change, PlayerChange::Chat { message } if message.contains("exactly one"))
    ));
}

#[test]
fn damage_command_lethal_hit_credits_player_and_preserves_damage_type() {
    let (mut state, actor, centre) = crate::hostile_tests::setup(8.0);
    let name = state.connections.name_of(actor).unwrap().to_owned();
    for args in [
        vec!["objectives", "add", "kills", "totalKillCount"],
        vec!["players", "set", "@s", "kills", "0"],
    ] {
        let command = crate::scoreboard::ScoreboardCommand::parse("scoreboard", &args).unwrap();
        crate::scoreboard::execute(&mut state, actor, &command);
    }
    let pig_type = state.registry.entity_type_id("minecraft:pig").unwrap();
    let pig = state.spawn_mob(pig_type, centre, 10.0);
    let changes = run(
        &mut state,
        actor,
        "damage @e[type=pig,limit=1] 100 generic by @s",
    );
    assert!(
        changes
            .entity_events
            .iter()
            .any(|event| matches!(event, EntityEvent::Death { entity } if *entity == pig))
    );
    assert_eq!(state.scoreboard.scores[&name]["kills"].value, 1);
    state.commit_merge_effects();
    assert_eq!(state.scoreboard.scores[&name]["kills"].value, 1);
}
