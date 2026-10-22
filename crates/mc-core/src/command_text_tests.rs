use super::*;
use crate::horde_bench::flat_world;
use std::sync::Arc;

fn state() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).expect("registry"));
    let world = flat_world(&registry);
    TickState::new(registry, world, 2, 512)
}

fn field_of<'a>(tag: &'a Tag, key: &str) -> Option<&'a Tag> {
    match tag {
        Tag::Compound(fields) => field(fields, key),
        _ => None,
    }
}

#[test]
fn parses_snbt_and_json_components() {
    assert_eq!(
        parse_component("\"hello world\"").unwrap(),
        Tag::String("hello world".into())
    );
    assert_eq!(
        parse_component("hello").unwrap(),
        Tag::String("hello".into())
    );
    let json = parse_component(r#"{"text":"hi","color":"red","bold":true}"#).unwrap();
    assert_eq!(field_of(&json, "color"), Some(&Tag::String("red".into())));
    let snbt = parse_component("{text:'hi',color:red}").unwrap();
    assert_eq!(field_of(&snbt, "text"), Some(&Tag::String("hi".into())));
    assert!(matches!(
        parse_component(r#"[{"text":"a"},{"text":"b"}]"#).unwrap(),
        Tag::List(_)
    ));
}

#[test]
fn rejects_malformed_or_non_component_values() {
    for bad in ["", "{", "{\"text\":", "42", "{a:1} trailing"] {
        assert!(parse_component(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn score_components_resolve_to_the_scoreboard_value() {
    let mut state = state();
    state
        .scoreboard
        .scores
        .entry("Alex".into())
        .or_default()
        .insert("kills".into(), 7);
    let component =
        parse_component(r#"{"score":{"name":"Alex","objective":"kills"},"color":"gold"}"#).unwrap();
    let resolved = resolve_component(&state, EntityId(-1), &component).unwrap();
    assert_eq!(
        field_of(&resolved, "color"),
        Some(&Tag::String("gold".into()))
    );
    let Some(Tag::List(extra)) = field_of(&resolved, "extra") else {
        panic!("score expands into extra: {resolved:?}");
    };
    assert_eq!(
        field_of(&extra.elements[0], "text"),
        Some(&Tag::String("7".into()))
    );
    assert!(field_of(&resolved, "score").is_none());
}

#[test]
fn selectors_with_no_match_expand_to_empty_text_and_nbt_is_refused() {
    let state = state();
    let component = parse_component(r#"{"selector":"@a"}"#).unwrap();
    let resolved = resolve_component(&state, EntityId(-1), &component).unwrap();
    assert_eq!(
        field_of(&resolved, "text"),
        Some(&Tag::String(String::new()))
    );
    assert!(field_of(&resolved, "extra").is_none());
    let nbt = parse_component(r#"{"nbt":"Health","entity":"@s"}"#).unwrap();
    assert!(resolve_component(&state, EntityId(-1), &nbt).is_err());
}

#[test]
fn plain_text_translate_and_with_arguments_pass_through() {
    let state = state();
    let component =
        parse_component(r#"{"translate":"chat.type.say","with":[{"text":"a"},{"text":"b"}]}"#)
            .unwrap();
    assert_eq!(
        resolve_component(&state, EntityId(-1), &component).unwrap(),
        component
    );
}

fn text_state() -> (TickState, EntityId, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).expect("registry"));
    let world = flat_world(&registry);
    let mut state = TickState::new(registry, world, 1, 4096);
    use crate::boundary::{ConnectionId, PlayerInput};
    crate::tick::run_tick(
        &mut state,
        &[
            PlayerInput::Join {
                connection: ConnectionId(1),
                uuid: uuid::Uuid::new_v4(),
                name: "Alice".into(),
                permission_level: 0,
            },
            PlayerInput::Join {
                connection: ConnectionId(2),
                uuid: uuid::Uuid::new_v4(),
                name: "Bob".into(),
                permission_level: 0,
            },
        ],
    );
    let alice = state.connections.entity_of(ConnectionId(1)).unwrap();
    let bob = state.connections.entity_of(ConnectionId(2)).unwrap();
    state.outbox.clear();
    (state, alice, bob)
}

fn styled(queue: &[crate::boundary::PlayerChange]) -> Vec<&mc_nbt::Tag> {
    queue
        .iter()
        .filter_map(|change| match change {
            crate::boundary::PlayerChange::StyledChat { content } => Some(content),
            _ => None,
        })
        .collect()
}

fn titles(queue: &[crate::boundary::PlayerChange]) -> Vec<&crate::boundary::TitleChange> {
    queue
        .iter()
        .filter_map(|change| match change {
            crate::boundary::PlayerChange::Title(change) => Some(change),
            _ => None,
        })
        .collect()
}

fn translation_key(tag: &mc_nbt::Tag) -> Option<&str> {
    match tag {
        Tag::Compound(fields) => fields.iter().find_map(|(name, value)| {
            (name == "translate").then_some(match value {
                Tag::String(key) => Some(key.as_str()),
                _ => None,
            })?
        }),
        _ => None,
    }
}

#[test]
fn tellraw_parse_keeps_component_text_split_safe() {
    let command =
        crate::commands::parse(r#"tellraw @a {"text":"hi","color":"red"}"#, true).unwrap();
    let crate::commands::Command::TellRaw { target, message } = command else {
        panic!("not tellraw: {command:?}");
    };
    assert_eq!(target, crate::commands::Target::AllPlayers);
    assert_eq!(
        field_of(&message, "color"),
        Some(&Tag::String("red".into()))
    );
    // List components and quoted strings with spaces survive too.
    let command = crate::commands::parse(r#"tellraw Bob ["a", {"text":"b c"}]"#, true).unwrap();
    assert!(matches!(command, crate::commands::Command::TellRaw { .. }));
    // Missing message / bad component / non-operator are rejected.
    assert!(crate::commands::parse("tellraw @a", true).is_err());
    assert!(crate::commands::parse("tellraw @a {", true).is_err());
    assert!(crate::commands::parse("tellraw @a hello", false).is_err());
}

#[test]
fn title_parse_covers_every_verb_and_times_suffixes() {
    let title = |rest: &str| crate::commands::parse(&format!("title @a {rest}"), true).unwrap();
    assert!(matches!(
        title("clear"),
        crate::commands::Command::Title {
            action: TitleAction::Clear { reset_times: false },
            ..
        }
    ));
    assert!(matches!(
        title("reset"),
        crate::commands::Command::Title {
            action: TitleAction::Clear { reset_times: true },
            ..
        }
    ));
    assert!(matches!(
        title(r#"title {"text":"hi"}"#),
        crate::commands::Command::Title {
            action: TitleAction::ShowTitle(_),
            ..
        }
    ));
    assert!(matches!(
        title(r#"subtitle "sub title""#),
        crate::commands::Command::Title {
            action: TitleAction::ShowSubtitle(_),
            ..
        }
    ));
    assert!(matches!(
        title("actionbar hello"),
        crate::commands::Command::Title {
            action: TitleAction::ShowActionBar(_),
            ..
        }
    ));
    let crate::commands::Command::Title { action, .. } = title("times 10 70 20") else {
        panic!("not title times");
    };
    assert_eq!(
        action,
        TitleAction::Times {
            fade_in: 10,
            stay: 70,
            fade_out: 20
        }
    );
    // Vanilla `time` suffixes: 1s = 20 ticks, 0.5s = 10, 1d = 24000.
    let crate::commands::Command::Title { action, .. } = title("times 1s 0.5s 1t") else {
        panic!("not title times");
    };
    assert_eq!(
        action,
        TitleAction::Times {
            fade_in: 20,
            stay: 10,
            fade_out: 1
        }
    );
    // Missing args, bad verb, negative/bad times, non-operator rejected.
    for bad in [
        "title",
        "title @a",
        "title @a frobnicate",
        "title @a title",
        "title @a times 1 2",
        "title @a times 1 2 x",
        "title @a times -1 2 3",
    ] {
        let input = if bad.starts_with("title @a") {
            bad.to_owned()
        } else {
            format!("title @a {bad}")
        };
        assert!(crate::commands::parse(&input, true).is_err(), "{input:?}");
    }
    assert!(crate::commands::parse("title @a clear", false).is_err());
}

#[test]
fn tellraw_sends_resolved_chat_with_no_sender_feedback() {
    let (mut state, alice, _) = text_state();
    let command = crate::commands::parse(r#"tellraw @a {"text":"hi"}"#, true).unwrap();
    let mut changes = crate::boundary::ChangeSet::default();
    crate::command_execution::execute(
        &mut state,
        crate::boundary::ConnectionId(1),
        alice,
        &command,
        &mut changes,
    );
    for connection in [
        crate::boundary::ConnectionId(1),
        crate::boundary::ConnectionId(2),
    ] {
        let queue = state.outbox.get(&connection).expect("recipient queue");
        assert_eq!(styled(queue).len(), 1);
        assert_eq!(
            field_of(styled(queue)[0], "text"),
            Some(&Tag::String("hi".into()))
        );
    }
    // Unknown targets carry the vanilla player key; selector resolution is
    // per player (entity override = that player).
    let (mut state, alice, _) = text_state();
    let command = crate::commands::parse("tellraw Nobody hello", true).unwrap();
    let mut changes = crate::boundary::ChangeSet::default();
    crate::command_execution::execute(
        &mut state,
        crate::boundary::ConnectionId(1),
        alice,
        &command,
        &mut changes,
    );
    let queue = state
        .outbox
        .get(&crate::boundary::ConnectionId(1))
        .expect("feedback queue");
    assert!(
        styled(queue)
            .iter()
            .any(|tag| translation_key(tag) == Some("argument.entity.notfound.player"))
    );
    assert_eq!(styled(queue).len(), 1);
}

#[test]
fn title_shows_resolve_per_player_and_clear_times_feedback() {
    use crate::boundary::ConnectionId;
    // show.title resolves `selector` per target: @s names each recipient.
    let (mut state, alice, _) = text_state();
    let command = crate::commands::parse(r#"title @a title {"selector":"@s"}"#, true).unwrap();
    let mut changes = crate::boundary::ChangeSet::default();
    crate::command_execution::execute(&mut state, ConnectionId(1), alice, &command, &mut changes);
    let alice_titles = titles(state.outbox.get(&ConnectionId(1)).expect("alice queue"));
    let bob_titles = titles(state.outbox.get(&ConnectionId(2)).expect("bob queue"));
    assert_eq!(alice_titles.len(), 1);
    assert_eq!(bob_titles.len(), 1);
    let name_of = |queue_titles: &[&crate::boundary::TitleChange]| match queue_titles[0] {
        crate::boundary::TitleChange::Title(tag) => match field_of(tag, "extra") {
            Some(Tag::List(list)) => format!("{list:?}"),
            other => format!("{other:?}"),
        },
        other => panic!("not a title: {other:?}"),
    };
    assert!(name_of(&alice_titles).contains("Alice"), "{alice_titles:?}");
    assert!(name_of(&bob_titles).contains("Bob"), "{bob_titles:?}");
    let feedback: Vec<_> = styled(state.outbox.get(&ConnectionId(1)).expect("feedback queue"))
        .into_iter()
        .filter_map(translation_key)
        .collect();
    assert!(feedback.contains(&"commands.title.show.title.multiple"));
    // Single-target feedback carries the display name; times/clear/subtitle/actionbar keys match vanilla.
    let (mut state, alice, _) = text_state();
    for (input, key) in [
        (
            "title Alice subtitle sub",
            "commands.title.show.subtitle.single",
        ),
        (
            "title Alice actionbar bar",
            "commands.title.show.actionbar.single",
        ),
        ("title Alice times 1 2 3", "commands.title.times.single"),
        ("title Alice clear", "commands.title.cleared.single"),
        ("title Alice reset", "commands.title.reset.single"),
    ] {
        let command = crate::commands::parse(input, true).unwrap();
        let mut changes = crate::boundary::ChangeSet::default();
        crate::command_execution::execute(
            &mut state,
            ConnectionId(1),
            alice,
            &command,
            &mut changes,
        );
        let queue = state.outbox.get(&ConnectionId(1)).expect("feedback queue");
        assert!(
            styled(queue)
                .iter()
                .any(|tag| translation_key(tag) == Some(key)),
            "{input}: {queue:?}"
        );
    }
    let queue = state.outbox.get(&ConnectionId(1)).expect("delivery queue");
    assert!(
        titles(queue)
            .iter()
            .any(|change| matches!(change, crate::boundary::TitleChange::Subtitle(_)))
    );
    assert!(
        titles(queue)
            .iter()
            .any(|change| matches!(change, crate::boundary::TitleChange::ActionBar(_)))
    );
    assert!(titles(queue).iter().any(|change| matches!(
        change,
        crate::boundary::TitleChange::Times {
            fade_in: 1,
            stay: 2,
            fade_out: 3
        }
    )));
    assert!(
        titles(queue)
            .iter()
            .any(|change| matches!(change, crate::boundary::TitleChange::Clear { .. }))
    );
}
