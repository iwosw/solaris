//! Native Army dialog gates: nav, rows, recruit/dismiss/order buttons invoke
//! the runtime API, and ownerless mutations refuse. Parser coverage lives in
//! commands_tests (army verbs); execution chat coverage is asserted here via
//! the same dialog harness. Uses the existing ui_tests dialog patterns.
use super::*;
use crate::boundary::PlayerInput;
use crate::settlements::allocator::tests::{block, test_state, villager};
use crate::tick::{TickState, run_tick};
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
    }
}

impl Shown {
    fn has(&self, action: &str) -> bool {
        self.actions
            .iter()
            .any(|(_, id)| id == &format!("solaris:settlement/{action}"))
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

/// Certified barracks (bed via level*2 slots) plus a storage chest the army
/// equipment path debits. Mirrors the army runtime fixture shape.
fn barracks_with_storage(state: &mut TickState, id: SettlementId) {
    use super::super::buildings::{Building, BuildingId, BuildingKind};
    let bid = BuildingId((id.0 << 32) | 1);
    state.settlements.get_mut(id).unwrap().buildings.insert(
        bid,
        Building {
            id: bid,
            settlement: id,
            kind: BuildingKind::Barracks,
            level: 1,
            bounds: mc_script::ScriptWorldBounds::new([1, 200, 1], [5, 204, 5]).unwrap(),
            work_points: vec![[2, 201, 2]],
            revision: 1,
            natural_zone: false,
        },
    );
    state.settlements.get_mut(id).unwrap().building_sequence = 1;
    let chest = block(state, "minecraft:chest", &[]);
    state.world.set_block(6, 201, 6, chest);
    state.world.fold_overlay();
    let sid = BuildingId((id.0 << 32) | 2);
    state.settlements.get_mut(id).unwrap().buildings.insert(
        sid,
        Building {
            id: sid,
            settlement: id,
            kind: BuildingKind::Storage,
            level: 1,
            bounds: mc_script::ScriptWorldBounds::new([6, 201, 6], [8, 203, 8]).unwrap(),
            work_points: vec![[6, 201, 6]],
            revision: 1,
            natural_zone: false,
        },
    );
    state.settlements.get_mut(id).unwrap().building_sequence = 2;
    state.block_entities.insert(
        (6, 201, 6),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
}

fn idle_resident(state: &mut TickState, id: SettlementId) {
    let slot = villager(state, [5.5, 201.0, 7.5]);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(state, id, entity).unwrap();
}

#[test]
fn army_nav_and_projects_page_render() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let main = last_dialog(&run(
        &mut state,
        &[PlayerInput::Command {
            connection: CONN,
            command: "settlement".into(),
            permission_level: 0,
        }],
    ));
    assert!(main.has("army"));
    assert!(main.has("projects"));
    let army = last_dialog(&click(&mut state, id, "army", &[]));
    assert_eq!(army.title, "Army - oak");
    assert!(army.body.contains("No soldiers"));
    assert!(army.has("army_recruit"));
    let projects = last_dialog(&click(&mut state, id, "projects", &[]));
    assert_eq!(projects.title, "Projects - oak");
    // The Projects page carries no status text: its grid comes first.
    assert!(projects.body.is_empty(), "{}", projects.body);
    assert!(projects.has("project_order"));
    let status = last_dialog(&click(&mut state, id, "project_status", &[]));
    assert_eq!(status.title, "Project status - oak");
    assert!(!status.body.is_empty());
    assert!(status.has("projects"));
    let page = last_dialog(&run(
        &mut state,
        &[PlayerInput::Command {
            connection: CONN,
            command: "settlement menu army".into(),
            permission_level: 0,
        }],
    ));
    assert_eq!(page.title, "Army - oak");
}

#[test]
fn army_recruit_order_and_dismiss_flow_through_dialogs() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    barracks_with_storage(&mut state, id);
    idle_resident(&mut state, id);
    let recruited = last_dialog(&click(&mut state, id, "army_recruit", &[]));
    assert!(recruited.body.contains("Recruited"), "{}", recruited.body);
    let uuid = state
        .entities
        .slots()
        .find_map(|slot| {
            let row = state.entities.row(slot);
            let villager = row.villager.as_ref()?;
            (villager.settlement == Some(id) && villager.soldier.is_some()).then_some(row.uuid)
        })
        .expect("recruited soldier");
    let id_text = soldier_id(uuid);
    let listed = last_dialog(&click(&mut state, id, "army", &[]));
    assert!(listed.body.contains(&id_text), "{}", listed.body);
    assert!(listed.has("army_soldier"));
    assert!(
        listed.has("army_order"),
        "all-soldier orders stay on the roster"
    );
    assert!(
        !listed.has("army_dismiss"),
        "dismiss lives on the soldier page"
    );
    let page = last_dialog(&click(
        &mut state,
        id,
        "army_soldier",
        &[("u", string(&uuid.to_string()))],
    ));
    assert!(page.title.contains(&id_text), "{}", page.title);
    assert!(page.body.contains(&uuid.to_string()), "{}", page.body);
    assert!(page.has("army_order"));
    assert!(page.has("army_dismiss"));
    let ordered = last_dialog(&click(
        &mut state,
        id,
        "army_order",
        &[("u", string(&uuid.to_string())), ("o", string("patrol"))],
    ));
    assert!(
        ordered.body.contains("Ordered 1 soldier(s)."),
        "{}",
        ordered.body
    );
    assert!(
        ordered.title.contains(&id_text),
        "a single-soldier order stays on that soldier's page: {}",
        ordered.title
    );
    let all = last_dialog(&click(
        &mut state,
        id,
        "army_order",
        &[("u", string("all")), ("o", string("return"))],
    ));
    assert!(all.body.contains("Ordered 1 soldier(s)."), "{}", all.body);
    assert_eq!(all.title, "Army - oak");
    let dismissed = last_dialog(&click(
        &mut state,
        id,
        "army_dismiss",
        &[("u", string(&uuid.to_string()))],
    ));
    assert!(dismissed.body.contains("Dismissed"), "{}", dismissed.body);
    let gone = last_dialog(&click(
        &mut state,
        id,
        "army_soldier",
        &[("u", string(&uuid.to_string()))],
    ));
    assert_eq!(gone.title, "Army - oak");
    assert!(gone.body.contains("no longer exists"), "{}", gone.body);
}

/// One click must recruit one soldier: the screen after a successful recruit
/// must not offer Recruit, or a second click on the same spot silently
/// recruits another villager.
#[test]
fn recruit_result_screen_offers_no_second_recruit() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    barracks_with_storage(&mut state, id);
    idle_resident(&mut state, id);
    idle_resident(&mut state, id);
    let recruited = last_dialog(&click(&mut state, id, "army_recruit", &[]));
    assert!(recruited.body.contains("Recruited"), "{}", recruited.body);
    assert!(
        !recruited.has("army_recruit"),
        "post-recruit screen kept its Recruit button: {:?}",
        recruited.actions
    );
    assert!(
        recruited.has("army"),
        "post-recruit screen needs a way back to the roster"
    );
    assert_eq!(super::super::army::list(&state, id).len(), 1);
    let back = last_dialog(&click(&mut state, id, "army", &[]));
    assert!(back.has("army_recruit"), "the roster still recruits");
}

/// A roster of six must stay reachable: one short line and one button per
/// soldier (not five order buttons plus Dismiss each), with every soldier's
/// orders and Dismiss one click away on its own page.
#[test]
fn six_soldier_roster_is_compact_and_every_soldier_reachable() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    barracks_with_storage(&mut state, id);
    let barracks = super::super::buildings::BuildingId((id.0 << 32) | 1);
    state
        .settlements
        .get_mut(id)
        .unwrap()
        .buildings
        .get_mut(&barracks)
        .unwrap()
        .level = 3;
    for _ in 0..6 {
        idle_resident(&mut state, id);
        click(&mut state, id, "army_recruit", &[]);
    }
    let soldiers: Vec<_> = super::super::army::rows(&state, id)
        .into_iter()
        .map(|row| row.uuid)
        .collect();
    assert_eq!(soldiers.len(), 6);
    let roster = last_dialog(&click(&mut state, id, "army", &[]));
    let lines: Vec<_> = roster.body.lines().collect();
    assert_eq!(lines.len(), 6, "one line per soldier: {}", roster.body);
    assert!(
        lines.iter().all(|line| line.chars().count() <= 40),
        "roster lines must not wrap into walls of text: {}",
        roster.body
    );
    // Recruit + six soldiers + four all-soldier orders + Back (Close is the exit).
    assert_eq!(roster.actions.len(), 12, "{:?}", roster.actions);
    for uuid in soldiers {
        let page = last_dialog(&click(
            &mut state,
            id,
            "army_soldier",
            &[("u", string(&uuid.to_string()))],
        ));
        assert!(page.title.contains(&soldier_id(uuid)), "{}", page.title);
        assert!(page.has("army_dismiss"), "{:?}", page.actions);
        assert_eq!(
            page.actions
                .iter()
                .filter(|(_, action)| action == "solaris:settlement/army_order")
                .count(),
            4
        );
        assert!(page.has("army"), "back to the roster");
    }
}

#[test]
fn army_mutations_refuse_without_manage_and_ownerless_settlements_stay_read_only() {
    let mut state = joined(0);
    let id = town(&mut state, "someone-else");
    let army = last_dialog(&click(&mut state, id, "army", &[]));
    assert!(army.body.contains("View only"));
    assert!(!army.has("army_recruit"));
    for (action, extra) in [
        ("army_recruit", vec![]),
        (
            "army_dismiss",
            vec![("u", string("12345678-1234-1234-1234-123456789012"))],
        ),
        (
            "army_order",
            vec![("u", string("all")), ("o", string("patrol"))],
        ),
    ] {
        let refused = last_dialog(&click(&mut state, id, action, &extra));
        assert!(
            refused.body.contains("Only the owner or an operator"),
            "{action}"
        );
    }
    // Ownerless villages: recruit/order/dismiss clicks refuse via dispatch.
    let at = player_pos(&state, CONN).unwrap();
    let free = state
        .settlements
        .adopt_village(
            &mut state.settlement_sequence,
            "minecraft:overworld:army-village",
            at,
        )
        .unwrap();
    let refused = last_dialog(&click(&mut state, free, "army_recruit", &[]));
    assert!(
        refused.body.contains("Only the owner or an operator"),
        "{}",
        refused.body
    );
    assert!(super::super::army::list(&state, free).is_empty());
}

#[test]
fn residents_screen_counts_barracks_soldiers_not_job_slots() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    barracks_with_storage(&mut state, id);
    idle_resident(&mut state, id);
    let before = last_dialog(&click(&mut state, id, "residents", &[]));
    assert!(
        before.body.contains("unemployed residents: 1"),
        "{}",
        before.body
    );
    click(&mut state, id, "army_recruit", &[]);
    let after = last_dialog(&click(&mut state, id, "residents", &[]));
    assert!(after.body.contains("1/2 soldiers"), "{}", after.body);
    assert!(
        after.body.contains("unemployed residents: 0"),
        "{}",
        after.body
    );
    // Recruit rows carry the runtime order; the main hint no longer flags
    // soldiers as workshop-less workers.
    assert!(after.body.contains("; soldier Patrol"), "{}", after.body);
    let main = last_dialog(&click(&mut state, id, "main", &[]));
    assert!(
        !main.body.contains("need a certified workshop"),
        "{}",
        main.body
    );
}

#[test]
fn army_commands_execute_roles_orders_and_permission_checks() {
    let mut state = joined(0);
    let id = town(&mut state, "someone-else");
    barracks_with_storage(&mut state, id);
    idle_resident(&mut state, id);
    let command = |state: &mut TickState, text: String| {
        run(
            state,
            &[PlayerInput::Command {
                connection: CONN,
                command: text,
                permission_level: 0,
            }],
        )
    };
    command(&mut state, "settlement army recruit".into());
    assert!(super::super::army::list(&state, id).is_empty());
    state.settlements.get_mut(id).unwrap().owner = Some(PLAYER.into());
    command(&mut state, "settlement army recruit".into());
    let slot = state
        .entities
        .slots()
        .find(|&slot| {
            state
                .entities
                .row(slot)
                .villager
                .as_ref()
                .is_some_and(|v| v.soldier.is_some())
        })
        .unwrap();
    let uuid = state.entities.row(slot).uuid;
    command(&mut state, "settlement army order all follow".into());
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .soldier
            .as_ref()
            .unwrap()
            .order,
        super::super::army::Order::Follow(uuid::Uuid::nil())
    );
    command(&mut state, format!("settlement army order {uuid} guard"));
    assert!(matches!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .soldier
            .as_ref()
            .unwrap()
            .order,
        super::super::army::Order::Guard(_)
    ));
    command(&mut state, format!("settlement army order {uuid} return"));
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .soldier
            .as_ref()
            .unwrap()
            .order,
        super::super::army::Order::Return
    );
    command(&mut state, format!("settlement army dismiss {uuid}"));
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .soldier
            .is_none()
    );
}
