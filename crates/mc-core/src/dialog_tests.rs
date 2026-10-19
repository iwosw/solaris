use super::*;
use crate::boundary::PlayerInput;
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use std::sync::Arc;

const CONN: ConnectionId = ConnectionId(7);

/// A fake connection joined to a real tick state: no sockets, no edge.
fn joined() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().unwrap();
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: CONN,
            uuid: uuid::Uuid::nil(),
            name: "Dialoger".into(),
            permission_level: 0,
        }],
    );
    state
}

fn field<'a>(tag: &'a Tag, key: &str) -> &'a Tag {
    match tag {
        Tag::Compound(entries) => &entries.iter().find(|(name, _)| name == key).unwrap().1,
        other => panic!("not a compound: {other:?}"),
    }
}

fn items(tag: &Tag) -> &[Tag] {
    match tag {
        Tag::List(list) => &list.elements,
        other => panic!("not a list: {other:?}"),
    }
}

fn changes_for(outbox: &[(ConnectionId, Vec<PlayerChange>)]) -> Vec<PlayerChange> {
    outbox
        .iter()
        .filter(|(connection, _)| *connection == CONN)
        .flat_map(|(_, changes)| changes.clone())
        .collect()
}

fn click(id: &str, payload: Option<Tag>) -> PlayerInput {
    PlayerInput::DialogClick {
        connection: CONN,
        id: id.into(),
        payload,
    }
}

#[test]
fn multi_action_with_every_input_kind_matches_the_vanilla_codec_shape() {
    let tag = Dialog::multi_action(
        "Found settlement",
        vec![Button::custom("Found", "solaris:settlement/found").with("v", Tag::Int(2))],
    )
    .text("Pick a name")
    .input(
        Input::text("name", "Name")
            .initial_text("Ёлки")
            .max_length(16),
    )
    .input(
        Input::number_range("radius", "Radius", 1.0, 32.0)
            .initial_number(8.0)
            .step(1.0),
    )
    .input(Input::boolean("walls", "Walls").initial_bool(true))
    .input(
        Input::single_option(
            "kind",
            "Kind",
            vec![
                ("a".into(), Text::from("A").0),
                ("b".into(), Text::from("B").0),
            ],
        )
        .initial_option("b"),
    )
    .exit(Button::plain("Close").tooltip("Leave"))
    .into_tag()
    .unwrap();

    assert_eq!(
        field(&tag, "type"),
        &Tag::String("minecraft:multi_action".into())
    );
    assert_eq!(field(&tag, "pause"), &Tag::Byte(0));
    assert_eq!(field(&tag, "after_action"), &Tag::String("close".into()));
    assert_eq!(field(&tag, "can_close_with_escape"), &Tag::Byte(1));
    assert_eq!(field(&tag, "columns"), &Tag::Int(2));

    let action = &items(field(&tag, "actions"))[0];
    let custom = field(action, "action");
    assert_eq!(
        field(custom, "type"),
        &Tag::String("minecraft:dynamic/custom".into())
    );
    assert_eq!(
        field(custom, "id"),
        &Tag::String("solaris:settlement/found".into())
    );
    assert_eq!(field(field(custom, "additions"), "v"), &Tag::Int(2));
    assert!(matches!(field(&tag, "exit_action"), Tag::Compound(_)));

    let body = &items(field(&tag, "body"))[0];
    assert_eq!(
        field(body, "type"),
        &Tag::String("minecraft:plain_message".into())
    );

    let inputs = items(field(&tag, "inputs"));
    let kinds: Vec<_> = inputs.iter().map(|input| field(input, "type")).collect();
    assert_eq!(
        kinds,
        [
            &Tag::String("minecraft:text".into()),
            &Tag::String("minecraft:number_range".into()),
            &Tag::String("minecraft:boolean".into()),
            &Tag::String("minecraft:single_option".into()),
        ]
    );
    assert_eq!(field(&inputs[1], "start"), &Tag::Float(1.0));
    assert_eq!(field(&inputs[1], "initial"), &Tag::Float(8.0));
    assert_eq!(field(&inputs[1], "step"), &Tag::Float(1.0));
    assert_eq!(field(&inputs[2], "initial"), &Tag::Byte(1));
    let options = items(field(&inputs[3], "options"));
    assert_eq!(field(&options[1], "initial"), &Tag::Byte(1));
}

#[test]
fn notice_and_confirmation_use_their_vanilla_field_names() {
    let notice = Dialog::notice("Hi", Button::plain("OK"))
        .after_action(AfterAction::WaitForResponse)
        .can_close_with_escape(false)
        .into_tag()
        .unwrap();
    assert_eq!(
        field(&notice, "type"),
        &Tag::String("minecraft:notice".into())
    );
    assert!(matches!(field(&notice, "action"), Tag::Compound(_)));
    assert_eq!(
        field(&notice, "after_action"),
        &Tag::String("wait_for_response".into())
    );
    assert_eq!(field(&notice, "can_close_with_escape"), &Tag::Byte(0));

    let confirm = Dialog::confirmation(
        "Sure?",
        Button::custom("Yes", "solaris:x/yes"),
        Button::plain("No"),
    )
    .into_tag()
    .unwrap();
    assert_eq!(
        field(&confirm, "type"),
        &Tag::String("minecraft:confirmation".into())
    );
    assert!(matches!(field(&confirm, "yes"), Tag::Compound(_)));
    assert!(matches!(field(&confirm, "no"), Tag::Compound(_)));
}

#[test]
fn malformed_dialogs_are_rejected_before_reaching_a_client() {
    let ok = || Button::custom("B", "solaris:x/y");
    let multi = || Dialog::multi_action("T", vec![ok()]);
    assert!(multi().into_tag().is_ok());
    assert!(Dialog::multi_action("T", vec![]).into_tag().is_err());
    assert!(multi().columns(0).into_tag().is_err());
    assert!(
        multi()
            .input(Input::text("bad key", "L"))
            .into_tag()
            .is_err()
    );
    assert!(
        multi()
            .input(Input::text("a", "L"))
            .input(Input::boolean("a", "L"))
            .into_tag()
            .is_err()
    );
    assert!(
        multi()
            .input(Input::number_range("n", "L", 0.0, 1.0).initial_number(2.0))
            .into_tag()
            .is_err()
    );
    assert!(
        multi()
            .input(Input::text("t", "L").initial_text("too long").max_length(3))
            .into_tag()
            .is_err()
    );
    assert!(
        multi()
            .input(Input::single_option("o", "L", vec![]))
            .into_tag()
            .is_err()
    );
    assert!(
        Dialog::multi_action("T", vec![Button::custom("B", "no-namespace")])
            .into_tag()
            .is_err()
    );
    assert!(
        Dialog::multi_action("T", vec![Button::custom("B", "solaris:x/y").width(0)])
            .into_tag()
            .is_err()
    );
}

#[test]
fn open_and_clear_reach_the_connection_outbox_in_order() {
    let mut state = joined();
    let dialog = Dialog::notice("Hello", Button::plain("OK"));
    let expected = dialog.clone().into_tag().unwrap();
    open(&mut state, CONN, dialog).unwrap();
    clear(&mut state, CONN);
    assert!(open(&mut state, CONN, Dialog::multi_action("T", vec![])).is_err());
    let (_, outbox, _) = run_tick(&mut state, &[]);
    let changes = changes_for(&outbox);
    let shown = changes
        .iter()
        .position(|change| {
            *change
                == PlayerChange::ShowDialog {
                    dialog: expected.clone(),
                }
        })
        .expect("ShowDialog queued");
    let cleared = changes
        .iter()
        .position(|change| *change == PlayerChange::ClearDialog)
        .expect("ClearDialog queued");
    assert!(shown < cleared);
    assert_eq!(
        changes
            .iter()
            .filter(|change| matches!(change, PlayerChange::ShowDialog { .. }))
            .count(),
        1,
        "the rejected dialog must not be queued"
    );
}

#[test]
fn click_reaches_its_owner_with_typed_payload_and_unknown_ids_are_ignored() {
    let mut state = joined();
    run_tick(&mut state, &[]);
    let payload = Tag::Compound(vec![
        ("source".into(), Tag::String("probe".into())),
        ("name".into(), Tag::String("Ёлки".into())),
        ("radius".into(), Tag::Float(12.0)),
        ("walls".into(), Tag::Byte(1)),
    ]);
    let (_, outbox, _) = run_tick(
        &mut state,
        &[
            click("solaris:dialog_probe/submit", Some(payload)),
            click("solaris:nobody/home", None),
            click("minecraft:other", None),
        ],
    );
    let messages: Vec<_> = changes_for(&outbox)
        .into_iter()
        .filter_map(|change| match change {
            PlayerChange::Chat { message } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        messages,
        [
            r#"dialog probe: action=submit name=Some("Ёлки") radius=Some(12.0) walls=Some(true) source=Some("probe")"#
        ]
    );
}

#[test]
fn click_payload_reader_rejects_wrong_types_and_non_finite_numbers() {
    let tag = Tag::Compound(vec![
        ("n".into(), Tag::Float(f32::NAN)),
        ("s".into(), Tag::Int(1)),
        ("b".into(), Tag::String("true".into())),
        ("dup".into(), Tag::String("first".into())),
        ("dup".into(), Tag::String("last".into())),
    ]);
    let payload = ClickPayload(Some(&tag));
    assert_eq!(payload.float("n"), None);
    assert_eq!(payload.string("s"), None);
    assert_eq!(payload.bool("b"), None);
    assert_eq!(payload.string("dup"), Some("last"));
    assert_eq!(payload.string("missing"), None);
    assert_eq!(ClickPayload(None).string("dup"), None);
}

#[test]
fn locked_sessions_cannot_click_dialogs() {
    let mut state = joined();
    state.require_plugin_login = true;
    state.locked_sessions.insert(CONN);
    let (_, outbox, _) = run_tick(
        &mut state,
        &[click(
            "solaris:dialog_probe/submit",
            Some(Tag::Compound(vec![])),
        )],
    );
    assert!(
        !changes_for(&outbox)
            .iter()
            .any(|change| matches!(change, PlayerChange::Chat { .. }))
    );
}
