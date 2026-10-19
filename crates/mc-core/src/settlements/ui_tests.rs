use super::*;
use crate::boundary::PlayerInput;
use crate::settlements::allocator::tests::{block, test_state, villager};
use crate::tick::run_tick;
use mc_nbt::Tag;
use std::sync::Arc;

const CONN: ConnectionId = ConnectionId(7);
const PLAYER: &str = "00000000-0000-0000-0000-000000000000";

/// `test_state` plus the native work-recipe catalog, with one joined player.
fn joined(permission_level: u8) -> TickState {
    let mut state = test_state();
    let mut registry = (*state.registry).clone();
    crate::plugin_startup::apply(
        &mut registry,
        &mc_plugin_host::DeploymentContribution::default(),
        None,
        &[],
    )
    .unwrap();
    state.registry = Arc::new(registry);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: CONN,
            uuid: uuid::Uuid::nil(),
            name: "Mayor".into(),
            permission_level,
        }],
    );
    state
}

fn town(state: &mut TickState, owner: &str) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "oak".into(),
            owner.into(),
            [8, 201, 8],
            48,
        )
        .unwrap()
}

fn field<'a>(tag: &'a Tag, key: &str) -> Option<&'a Tag> {
    match tag {
        Tag::Compound(entries) => entries.iter().find(|(name, _)| name == key).map(|(_, v)| v),
        _ => None,
    }
}

fn list(tag: Option<&Tag>) -> &[Tag] {
    match tag {
        Some(Tag::List(list)) => &list.elements,
        _ => &[],
    }
}

fn text(tag: &Tag) -> String {
    match tag {
        Tag::String(value) => value.clone(),
        _ => String::new(),
    }
}

/// What a client sees of one dialog.
struct Shown {
    title: String,
    body: String,
    actions: Vec<(String, String)>,
    inputs: Vec<String>,
}

fn shown(tag: &Tag) -> Shown {
    let mut actions = Vec::new();
    for button in list(field(tag, "actions")) {
        let id = field(button, "action")
            .and_then(|action| field(action, "id"))
            .map(text)
            .unwrap_or_default();
        actions.push((text(field(button, "label").unwrap()), id));
    }
    Shown {
        title: text(field(tag, "title").unwrap()),
        body: list(field(tag, "body"))
            .iter()
            .map(|paragraph| text(field(paragraph, "contents").unwrap()))
            .collect::<Vec<_>>()
            .join("\n"),
        actions,
        inputs: list(field(tag, "inputs"))
            .iter()
            .map(|input| text(field(input, "key").unwrap()))
            .collect(),
    }
}

impl Shown {
    fn has(&self, action: &str) -> bool {
        self.actions
            .iter()
            .any(|(_, id)| id == &format!("solaris:settlement/{action}"))
    }
    fn label_of(&self, action: &str) -> &str {
        &self
            .actions
            .iter()
            .find(|(_, id)| id == &format!("solaris:settlement/{action}"))
            .unwrap()
            .0
    }
}

fn run(state: &mut TickState, inputs: &[PlayerInput]) -> Vec<PlayerChange> {
    let (_, outbox, _) = run_tick(state, inputs);
    outbox
        .into_iter()
        .filter(|(connection, _)| *connection == CONN)
        .flat_map(|(_, changes)| changes)
        .collect()
}

fn last_dialog(changes: &[PlayerChange]) -> Shown {
    changes
        .iter()
        .rev()
        .find_map(|change| match change {
            PlayerChange::ShowDialog { dialog } => Some(shown(dialog)),
            _ => None,
        })
        .expect("a dialog was shown")
}

fn chat(changes: &[PlayerChange]) -> Vec<String> {
    changes
        .iter()
        .filter_map(|change| match change {
            PlayerChange::Chat { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn command(state: &mut TickState, line: &str) -> Vec<PlayerChange> {
    run(
        state,
        &[PlayerInput::Command {
            connection: CONN,
            command: line.into(),
            permission_level: 0,
        }],
    )
}

fn click(
    state: &mut TickState,
    id: SettlementId,
    action: &str,
    extra: &[(&str, Tag)],
) -> Vec<PlayerChange> {
    let mut entries = vec![("s".to_owned(), Tag::String(id.0.to_string()))];
    entries.extend(extra.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
    run(
        state,
        &[PlayerInput::DialogClick {
            connection: CONN,
            id: format!("solaris:settlement/{action}"),
            payload: Some(Tag::Compound(entries)),
        }],
    )
}

fn string(value: &str) -> Tag {
    Tag::String(value.into())
}

fn station(state: &mut TickState, at: [i32; 3], name: &str) {
    let id = block(state, name, &[]);
    state.world.set_block(at[0], at[1], at[2], id);
    state.world.fold_overlay();
}

fn certify_form(kind: &str) -> Vec<(&'static str, Tag)> {
    vec![
        ("kind", string(kind)),
        ("from", string("3 200 3")),
        ("to", string("7 200 7")),
        ("extra", Tag::Float(4.0)),
    ]
}

#[test]
fn settlement_command_opens_the_main_screen_with_live_numbers_for_a_player_inside() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let changes = command(&mut state, "settlement");
    let dialog = last_dialog(&changes);
    assert_eq!(dialog.title, "Settlement oak");
    assert!(dialog.body.contains("Radius: 48 blocks around 8 201 8"));
    assert!(dialog.body.contains("Residents: 0"));
    assert!(dialog.body.contains("Storage: none certified"));
    assert!(dialog.body.contains("Buildings: none yet"));
    assert!(!dialog.body.contains("need a certified workshop"));
    for action in ["buildings", "residents", "orders", "settings"] {
        assert!(dialog.has(action), "{action}");
    }
    // Pages go straight to a screen.
    let page = last_dialog(&command(&mut state, "settlement menu buildings"));
    assert!(page.title.starts_with("Buildings"));
    assert!(page.has("certify"));
    assert_eq!(id, SettlementId(1));
}

#[test]
fn command_without_any_settlement_explains_how_to_found_one() {
    let mut state = joined(0);
    let changes = command(&mut state, "settlement");
    assert!(chat(&changes)[0].contains("/settlement create"));
    assert!(
        !changes
            .iter()
            .any(|change| matches!(change, PlayerChange::ShowDialog { .. }))
    );
}

#[test]
fn non_owner_sees_the_screens_read_only_and_every_mutating_click_is_refused() {
    let mut state = joined(0);
    let id = town(&mut state, "someone-else");
    let main = last_dialog(&command(&mut state, "settlement"));
    assert!(main.body.contains("View only"));
    let buildings = last_dialog(&click(&mut state, id, "buildings", &[]));
    assert!(!buildings.has("certify"));
    let settings = last_dialog(&click(&mut state, id, "settings", &[]));
    assert!(!settings.has("rename") && settings.inputs.is_empty());

    station(&mut state, [4, 201, 4], "minecraft:composter");
    for (action, extra) in [
        ("certify_do", certify_form("farmer")),
        ("certify", vec![]),
        ("order_start", vec![]),
        ("rename", vec![("name", string("hijacked"))]),
        ("upgrade", vec![("b", string("1"))]),
    ] {
        let refused = last_dialog(&click(&mut state, id, action, &extra));
        assert!(
            refused.body.contains("Only the owner or an operator"),
            "{action}"
        );
    }
    let record = state.settlements.get(id).unwrap();
    assert!(record.buildings.is_empty());
    assert_eq!(record.name, "oak");
}

#[test]
fn operator_manages_a_settlement_it_does_not_own() {
    let mut state = joined(2);
    let id = town(&mut state, "someone-else");
    let buildings = last_dialog(&click(&mut state, id, "buildings", &[]));
    assert!(buildings.has("certify"));
    let renamed = last_dialog(&click(&mut state, id, "rename", &[("name", string("elm"))]));
    assert!(renamed.body.contains("Renamed to elm."));
    assert_eq!(state.settlements.get(id).unwrap().name, "elm");
}

#[test]
fn stale_or_foreign_ids_never_panic_or_leak() {
    let mut state = joined(0);
    town(&mut state, PLAYER);
    let changes = click(&mut state, SettlementId(99), "buildings", &[]);
    assert_eq!(chat(&changes), ["That settlement no longer exists."]);
    // Missing/garbled settlement entry and unknown actions are ignored.
    let changes = run(
        &mut state,
        &[
            PlayerInput::DialogClick {
                connection: CONN,
                id: "solaris:settlement/buildings".into(),
                payload: Some(Tag::Compound(vec![("s".into(), string("nope"))])),
            },
            PlayerInput::DialogClick {
                connection: CONN,
                id: "solaris:settlement/ghost".into(),
                payload: None,
            },
        ],
    );
    assert!(chat(&changes).is_empty());
    let changes = click(&mut state, SettlementId(1), "ghost", &[]);
    assert!(changes.is_empty());
}

#[test]
fn certify_reports_missing_requirements_then_certifies_and_lists_with_upgrade_needs() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    // Nothing built: the form comes back with the reasons and the typed values.
    let failed = last_dialog(&click(
        &mut state,
        id,
        "certify_do",
        &certify_form("farmer"),
    ));
    assert_eq!(failed.title, "Certify building");
    assert!(
        failed
            .body
            .contains("Cannot certify Farmer workshop. Missing:")
    );
    assert_eq!(failed.inputs, ["kind", "from", "to", "extra"]);
    assert!(state.settlements.get(id).unwrap().buildings.is_empty());

    station(&mut state, [4, 201, 4], "minecraft:composter");
    let done = last_dialog(&click(
        &mut state,
        id,
        "certify_do",
        &certify_form("farmer"),
    ));
    assert!(done.title.starts_with("Buildings"));
    assert!(
        done.body
            .contains("Certified Farmer workshop #1 (level 1).")
    );
    assert!(done.body.contains("jobs 0/1"));
    assert!(done.body.contains("L2 needs: workstations 1/2"));
    assert!(done.has("upgrade"));
    assert_eq!(state.settlements.get(id).unwrap().buildings.len(), 1);

    // Upgrade refused while the second station is missing, then granted.
    let refused = last_dialog(&click(
        &mut state,
        id,
        "upgrade",
        &[("b", string(&(id.0 << 32 | 1).to_string()))],
    ));
    assert!(
        refused
            .body
            .contains("Cannot upgrade #1. Missing: workstations 1/2.")
    );
    station(&mut state, [6, 201, 4], "minecraft:composter");
    let upgraded = last_dialog(&click(
        &mut state,
        id,
        "upgrade",
        &[("b", string(&(id.0 << 32 | 1).to_string()))],
    ));
    assert!(upgraded.body.contains("Building #1 upgraded to level 2."));
    assert!(upgraded.body.contains("Farmer workshop L2"));
    // A building of another settlement id is rejected, not trusted.
    let forged = last_dialog(&click(
        &mut state,
        id,
        "upgrade",
        &[("b", string(&(5u64 << 32 | 1).to_string()))],
    ));
    assert!(forged.body.contains("no longer exists"));
}

#[test]
fn marking_corners_prefills_the_certify_form_and_bad_input_is_explained() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let first = last_dialog(&click(&mut state, id, "mark1", &certify_form("house")));
    let at = player_pos(&state, CONN).unwrap();
    assert!(
        first
            .body
            .contains(&format!("Corner 1 = {}", corner_text(at)))
    );
    assert_eq!(state.settlement_marks[&CONN][0], Some(at));
    // Reopening from the buildings screen uses the stored mark as corner 1.
    click(&mut state, id, "buildings", &[]);
    let reopened = last_dialog(&click(&mut state, id, "certify", &[]));
    assert_eq!(reopened.inputs, ["kind", "from", "to", "extra"]);
    let bad = last_dialog(&click(
        &mut state,
        id,
        "certify_do",
        &[
            ("kind", string("house")),
            ("from", string("a b c")),
            ("to", string("1 2 3")),
        ],
    ));
    assert!(bad.body.contains("three whole numbers"));
    let unknown = last_dialog(&click(
        &mut state,
        id,
        "certify_do",
        &[
            ("kind", string("castle")),
            ("from", string("1 2 3")),
            ("to", string("1 2 3")),
        ],
    ));
    assert!(unknown.body.contains("Choose a building type"));
}

fn bound_resident(state: &mut TickState, id: SettlementId) -> uuid::Uuid {
    let slot = villager(state, [5.5, 201.0, 7.5]);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(state, id, entity).unwrap();
    state.entities.row(slot).uuid
}

#[test]
fn main_hint_counts_working_profession_residents_without_a_certified_job() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let farmer = bound_resident(&mut state, id);
    let idle = bound_resident(&mut state, id);
    let nitwit = bound_resident(&mut state, id);
    for (uuid, profession) in [
        (farmer, Profession::Farmer),
        (idle, Profession::None),
        (nitwit, Profession::Nitwit),
    ] {
        let slot = state.entities.slot_of_uuid(uuid).unwrap();
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .data
            .profession = profession;
    }
    let main = last_dialog(&click(&mut state, id, "main", &[]));
    assert!(
        main.body
            .contains("1 workers need a certified workshop — open Buildings → Certify"),
        "{}",
        main.body
    );
    // Certifying a matching workshop and claiming its work point recovers the
    // worker; the profession is kept throughout.
    station(&mut state, [4, 201, 4], "minecraft:composter");
    click(&mut state, id, "certify_do", &certify_form("farmer"));
    let dimension = state.dimension.name().to_owned();
    let slot = state.entities.slot_of_uuid(farmer).unwrap();
    let villager = state.entities.row_mut(slot).villager.as_mut().unwrap();
    villager.job_site = Some(crate::boundary::BlockPos { x: 4, y: 201, z: 4 });
    villager.job_site_dimension = dimension;
    let main = last_dialog(&click(&mut state, id, "main", &[]));
    assert!(
        !main.body.contains("need a certified workshop"),
        "{}",
        main.body
    );
    let slot = state.entities.slot_of_uuid(farmer).unwrap();
    let villager = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(villager.data.profession, Profession::Farmer);
}

#[test]
fn residents_screen_lists_jobs_and_free_slots() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    click(&mut state, id, "certify_do", &certify_form("farmer"));
    let uuid = bound_resident(&mut state, id);
    let screen = last_dialog(&click(&mut state, id, "residents", &[]));
    let name = super::super::presentation::villager_name(uuid);
    assert!(
        screen
            .body
            .contains("#1 Farmer workshop L1: 0/1 jobs filled")
    );
    assert!(
        screen
            .body
            .contains("Free job slots: 1; unemployed residents: 1")
    );
    assert!(screen.body.contains(&format!("{name} (None): no job")));
}

#[test]
fn production_order_flow_creates_pauses_and_cancels_a_real_job() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    station(&mut state, [5, 201, 5], "minecraft:crafting_table");
    click(&mut state, id, "certify_do", &certify_form("farmer"));
    let building = BuildingId(id.0 << 32 | 1);

    // No resident yet: the flow explains it instead of opening an empty form.
    let none = last_dialog(&click(&mut state, id, "order_start", &[]));
    assert!(none.body.contains("No free resident"));

    let worker = bound_resident(&mut state, id);
    let form = last_dialog(&click(&mut state, id, "order_start", &[]));
    assert_eq!(form.title, "New production order");
    assert_eq!(form.inputs, ["count", "worker"]);
    assert!(form.has("order_create"));

    let recipe = state
        .registry
        .item_facts
        .work_recipes()
        .find(|recipe| recipe.station.as_str() == "minecraft:crafting_table")
        .unwrap()
        .id
        .as_str()
        .to_owned();
    let create = |recipe: &str, at: &str| {
        vec![
            ("b", string(&building.0.to_string())),
            ("p", string(at)),
            ("r", string(recipe)),
            ("worker", string(&worker.to_string())),
            ("count", Tag::Float(3.0)),
        ]
    };
    // A station position that is not a crafting table is refused.
    let wrong = last_dialog(&click(
        &mut state,
        id,
        "order_create",
        &create(&recipe, "4 201 4"),
    ));
    assert!(wrong.body.contains("needs a different workstation"));
    assert!(
        state
            .settlements
            .get(id)
            .unwrap()
            .economy
            .production
            .jobs
            .is_empty()
    );

    let ok = last_dialog(&click(
        &mut state,
        id,
        "order_create",
        &create(&recipe, "5 201 5"),
    ));
    assert!(ok.body.contains("Order ui-1 started"), "{}", ok.body);
    let job = state.settlements.get(id).unwrap().economy.production.jobs["ui-1"].clone();
    assert_eq!(
        (job.worker, job.count, job.station),
        (worker, 3, [5, 201, 5])
    );
    assert_eq!(job.building.kind, "building");
    assert_eq!(job.building.id, building.0.to_string());
    assert!((0..3).all(|i| (job.stand_at[i] - job.station[i]).abs() <= 1));
    assert_ne!(job.stand_at, job.station);

    // The same worker cannot take a second order.
    let busy = last_dialog(&click(
        &mut state,
        id,
        "order_create",
        &create(&recipe, "5 201 5"),
    ));
    assert!(busy.body.contains("already has an active order"));
    let none = last_dialog(&click(&mut state, id, "order_start", &[]));
    assert!(none.body.contains("No free resident"));

    let listing = last_dialog(&click(&mut state, id, "orders", &[]));
    assert!(listing.body.contains("ui-1:"));
    assert!(listing.has("order_pause") && listing.has("order_cancel"));
    assert_eq!(listing.label_of("order_pause"), "Pause ui-1");

    let paused = last_dialog(&click(
        &mut state,
        id,
        "order_pause",
        &[("j", string("ui-1"))],
    ));
    assert!(paused.body.contains("paused"));
    assert!(paused.has("order_resume"));
    let resumed = last_dialog(&click(
        &mut state,
        id,
        "order_resume",
        &[("j", string("ui-1"))],
    ));
    assert!(resumed.has("order_pause"));
    let cancelled = last_dialog(&click(
        &mut state,
        id,
        "order_cancel",
        &[("j", string("ui-1"))],
    ));
    assert!(cancelled.body.contains("cancelled"));
    assert!(state.settlements.get(id).unwrap().economy.production.jobs["ui-1"].cancelled);
    let gone = last_dialog(&click(
        &mut state,
        id,
        "order_cancel",
        &[("j", string("nope"))],
    ));
    assert!(gone.body.contains("no longer exists"));
}

#[test]
fn production_without_a_crafting_table_says_what_to_place() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    click(&mut state, id, "certify_do", &certify_form("farmer"));
    let note = last_dialog(&click(&mut state, id, "order_start", &[]));
    assert!(
        note.body
            .contains("No crafting table inside a certified workshop")
    );
}

#[test]
fn rename_validates_and_updates_the_record() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let bad = last_dialog(&click(
        &mut state,
        id,
        "rename",
        &[("name", string("bad name!"))],
    ));
    assert!(bad.body.contains("invalid settlement name"));
    assert_eq!(state.settlements.get(id).unwrap().name, "oak");
    let good = last_dialog(&click(
        &mut state,
        id,
        "rename",
        &[("name", string("birch"))],
    ));
    assert!(good.body.contains("Renamed to birch."));
    assert_eq!(state.settlements.get(id).unwrap().name, "birch");
    assert_eq!(good.inputs, ["name"]);
}
