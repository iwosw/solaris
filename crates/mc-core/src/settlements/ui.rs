//! Settlement management dialogs (26.1.2 server-driven UI): `/settlement`,
//! `/settlement menu [page]`, and every button click routed here as
//! `solaris:settlement/<action>` by [`crate::dialog::route_click`].
//!
//! Authority stays in the tick: a dialog is a view built from live state, and a
//! click is an untrusted request. Every click re-resolves the settlement from the
//! `s` payload entry, re-derives permission (record owner or operator, the same
//! rule as the chat commands; reads are open) and re-validates all other input
//! before calling the existing native paths ([`buildings::certify`],
//! [`buildings::upgrade`], production [`Action`]s, registry rename). Labels are
//! English; the client locale never reaches the core, so the Russian wording lives
//! in `docs/SETTLEMENTS_GUIDE.md`.

use std::collections::BTreeSet;

use mc_data::Identifier;
use mc_nbt::Tag;
use mc_script::{
    ScriptOperationFailure as Failure, ScriptSettlementBuildingRef,
    ScriptSettlementEconomyAction as Action, ScriptWorldBounds,
};
use uuid::Uuid;

use super::buildings::{self, BuildingId, BuildingKind, CertError};
use super::{SettlementId, economy_customization, inventory};
use crate::boundary::{ConnectionId, PlayerChange};
use crate::dialog::{self, Button, ClickPayload, Dialog, Input};
use crate::tick::TickState;
use crate::villager::VillagerProfession as Profession;

pub(crate) const OWNER: &str = "settlement";

/// The two block positions a player marked for the certify form.
pub(crate) type Marks = [Option<[i32; 3]>; 2];

const MAX_LISTED: usize = 12;
const MAX_RESIDENTS: usize = 30;
const MAX_STATIONS: usize = 4;
const MAX_MARKS: usize = 64;
const MAX_ORDER: f32 = 64.0;
const MAX_EXTRA_HEIGHT: f32 = 24.0;
const DENIED: &str = "Only the owner or an operator can change this settlement.";

/// Actions that change state; everything else is a read-only screen.
const MUTATING: &[&str] = &[
    "certify",
    "mark1",
    "mark2",
    "certify_do",
    "upgrade",
    "order_start",
    "order_pick",
    "order_create",
    "order_pause",
    "order_resume",
    "order_cancel",
    "rename",
];

struct Ctx {
    id: SettlementId,
    manage: bool,
}

struct Resident {
    uuid: Uuid,
    profession: Profession,
    job: Option<BuildingId>,
}

struct Station {
    building: BuildingId,
    kind: BuildingKind,
    at: [i32; 3],
    block: String,
}

#[derive(Default)]
struct Form {
    kind: String,
    from: String,
    to: String,
    extra: f32,
}

// ---------------------------------------------------------------- entry points

/// `/settlement` and `/settlement menu [page]`: the settlement around the player
/// (else one they own), on the requested page.
pub(crate) fn open_menu(state: &mut TickState, connection: ConnectionId, page: Option<&str>) {
    let Some(id) = locate(state, connection) else {
        say(
            state,
            connection,
            "No settlement here. Stand inside one, or found one with /settlement create <name> [radius].",
        );
        return;
    };
    let Some(ctx) = ctx(state, connection, id) else {
        say(
            state,
            connection,
            "Settlement command admission is unavailable.",
        );
        return;
    };
    let action = match page.unwrap_or("main") {
        "main" => "main",
        "buildings" => "buildings",
        "certify" => "certify",
        "residents" | "jobs" => "residents",
        "production" | "orders" => "orders",
        "settings" => "settings",
        _ => {
            say(
                state,
                connection,
                "Pages: main, buildings, certify, residents, production, settings.",
            );
            return;
        }
    };
    dispatch(state, connection, &ctx, action, ClickPayload(None));
}

/// One `solaris:settlement/<action>` click.
pub(crate) fn click(
    state: &mut TickState,
    connection: ConnectionId,
    action: &str,
    payload: ClickPayload<'_>,
) {
    let Some(id) = payload
        .string("s")
        .and_then(|text| text.parse().ok())
        .map(SettlementId)
    else {
        return;
    };
    let Some(ctx) = ctx(state, connection, id) else {
        say(state, connection, "That settlement no longer exists.");
        return;
    };
    dispatch(state, connection, &ctx, action, payload);
}

fn dispatch(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    action: &str,
    payload: ClickPayload<'_>,
) {
    if MUTATING.contains(&action) && !ctx.manage {
        main_screen(state, connection, ctx, DENIED);
        return;
    }
    match action {
        "main" => main_screen(state, connection, ctx, ""),
        "buildings" => buildings_screen(state, connection, ctx, ""),
        "residents" => residents_screen(state, connection, ctx),
        "orders" => orders_screen(state, connection, ctx, ""),
        "settings" => settings_screen(state, connection, ctx, ""),
        "certify" => {
            let form = form_of(state, connection, payload);
            certify_screen(state, connection, ctx, &form, "");
        }
        "mark1" | "mark2" => {
            let index = usize::from(action == "mark2");
            let mut form = form_of(state, connection, payload);
            match player_pos(state, connection) {
                Some(at) => {
                    set_mark(state, connection, index, at);
                    *(if index == 0 {
                        &mut form.from
                    } else {
                        &mut form.to
                    }) = corner_text(at);
                    let note = format!("Corner {} = {}.", index + 1, corner_text(at));
                    certify_screen(state, connection, ctx, &form, &note);
                }
                None => certify_screen(state, connection, ctx, &form, "Your position is unknown."),
            }
        }
        "certify_do" => certify(state, connection, ctx, payload),
        "upgrade" => upgrade(state, connection, ctx, payload),
        "order_start" => order_start(state, connection, ctx),
        "order_pick" => order_pick(state, connection, ctx, payload),
        "order_create" => order_create(state, connection, ctx, payload),
        "order_pause" | "order_resume" | "order_cancel" => {
            order_control(state, connection, ctx, action, payload)
        }
        "rename" => rename(state, connection, ctx, payload),
        _ => {
            tracing::debug!(target: "dialog", connection = connection.0, action, "unknown settlement action");
        }
    }
}

// ------------------------------------------------------------------- resolving

fn ctx(state: &TickState, connection: ConnectionId, id: SettlementId) -> Option<Ctx> {
    let record = state.settlements.get(id)?;
    let actor = state.connections.uuid_of(connection)?.to_string();
    Some(Ctx {
        id,
        manage: record.owner == actor || state.plugin_players.is_operator(connection),
    })
}

fn player_pos(state: &TickState, connection: ConnectionId) -> Option<[i32; 3]> {
    let entity = state.connections.entity_of(connection)?;
    let slot = state.entities.slot_of(entity)?;
    Some(state.entities.pos(slot).map(|value| value.floor() as i32))
}

/// The nearest settlement whose radius contains the player, else the first the
/// player owns.
fn locate(state: &TickState, connection: ConnectionId) -> Option<SettlementId> {
    let actor = state.connections.uuid_of(connection)?.to_string();
    if let Some(at) = player_pos(state, connection) {
        let inside = state
            .settlements
            .records()
            .filter_map(|record| {
                let distance: i64 = at
                    .iter()
                    .zip(record.anchor)
                    .map(|(a, b)| (i64::from(*a) - i64::from(b)).pow(2))
                    .sum();
                (distance <= i64::from(record.radius).pow(2)).then_some((distance, record.id))
            })
            .min();
        if let Some((_, id)) = inside {
            return Some(id);
        }
    }
    state
        .settlements
        .records()
        .find(|record| record.owner == actor)
        .map(|record| record.id)
}

fn say(state: &mut TickState, connection: ConnectionId, message: &str) {
    state.push_player(
        connection,
        PlayerChange::Chat {
            message: message.to_owned(),
        },
    );
}

fn present(state: &mut TickState, connection: ConnectionId, dialog: Dialog) {
    if let Err(error) = dialog::open(state, connection, dialog) {
        tracing::warn!(target: "dialog", %error, "settlement dialog rejected");
    }
}

fn button(label: impl Into<String>, action: &str, id: SettlementId) -> Button {
    Button::custom(label.into(), &format!("solaris:{OWNER}/{action}"))
        .with("s", Tag::String(id.0.to_string()))
}

fn frame(title: String, note: &str, text: String, buttons: Vec<Button>) -> Dialog {
    let mut dialog = Dialog::multi_action(title, buttons);
    if !note.is_empty() {
        dialog = dialog.text(note);
    }
    dialog.text(text).exit(Button::plain("Close"))
}

fn short(id: BuildingId) -> u64 {
    id.0 & u64::from(u32::MAX)
}

fn corner_text(at: [i32; 3]) -> String {
    format!("{} {} {}", at[0], at[1], at[2])
}

fn kind_label(kind: BuildingKind) -> String {
    match kind {
        BuildingKind::House => "House".into(),
        BuildingKind::Storage => "Storage".into(),
        BuildingKind::Mine => "Mine".into(),
        BuildingKind::LumberCamp => "Lumber camp".into(),
        BuildingKind::Barracks => "Barracks".into(),
        BuildingKind::Wall => "Wall section".into(),
        BuildingKind::Tower => "Tower".into(),
        BuildingKind::Gate => "Gate".into(),
        BuildingKind::Workshop(profession) => format!("{profession:?} workshop"),
    }
}

fn category(kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::House => "Housing",
        BuildingKind::Storage => "Storage",
        BuildingKind::Workshop(_) | BuildingKind::Mine | BuildingKind::LumberCamp => "Production",
        BuildingKind::Barracks => "Military",
        BuildingKind::Wall | BuildingKind::Tower | BuildingKind::Gate => "Fortification",
    }
}

fn all_kinds() -> Vec<BuildingKind> {
    let mut kinds = vec![BuildingKind::House, BuildingKind::Storage];
    kinds.extend(
        [
            Profession::Armorer,
            Profession::Butcher,
            Profession::Cartographer,
            Profession::Cleric,
            Profession::Farmer,
            Profession::Fisherman,
            Profession::Fletcher,
            Profession::Leatherworker,
            Profession::Librarian,
            Profession::Mason,
            Profession::Shepherd,
            Profession::Toolsmith,
            Profession::Weaponsmith,
        ]
        .map(BuildingKind::Workshop),
    );
    kinds.extend([
        BuildingKind::Mine,
        BuildingKind::LumberCamp,
        BuildingKind::Barracks,
        BuildingKind::Wall,
        BuildingKind::Tower,
        BuildingKind::Gate,
    ]);
    kinds
}

fn missing_text(error: &CertError) -> String {
    error
        .missing
        .iter()
        .map(|requirement| {
            if requirement.required <= 1 && requirement.found == 0 {
                requirement.description.clone()
            } else {
                format!(
                    "{} {}/{}",
                    requirement.description, requirement.found, requirement.required
                )
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn block_name(state: &TickState, at: [i32; 3]) -> Option<&str> {
    let native = state.world.block_at(at[0], at[1], at[2])?;
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(native))
        .map(|block| block.block.id.as_str())
}

/// Live bound residents, deterministic order (uuid).
fn residents(state: &TickState, id: SettlementId) -> Vec<Resident> {
    let mut people = Vec::new();
    for slots in state.entities.villager_chunks().values() {
        for &entity in slots {
            let Some(slot) = state.entities.slot_of(entity) else {
                continue;
            };
            if !state.entities.body(slot).alive {
                continue;
            }
            let row = state.entities.row(slot);
            let Some(villager) = row.villager.as_ref() else {
                continue;
            };
            if villager.settlement != Some(id) || villager.wandering {
                continue;
            }
            people.push(Resident {
                uuid: row.uuid,
                profession: villager.data.profession,
                job: buildings::building_for_job(state, villager).map(|building| building.id),
            });
        }
    }
    people.sort_by_key(|resident| resident.uuid);
    people
}

fn busy_workers(state: &TickState, id: SettlementId) -> BTreeSet<Uuid> {
    state
        .settlements
        .get(id)
        .into_iter()
        .flat_map(|record| record.economy.production.jobs.values())
        .filter(|job| !job.cancelled && job.done < job.count)
        .map(|job| job.worker)
        .collect()
}

// ------------------------------------------------------------------------ main

fn storage_summary(state: &TickState, id: SettlementId) -> String {
    let points = buildings::storage_points(state, id);
    if points.is_empty() {
        return "Storage: none certified".to_owned();
    }
    let dimension = state.dimension.name();
    let mut totals = std::collections::BTreeMap::<&str, i64>::new();
    for at in points.iter().take(64) {
        let endpoint = inventory::Endpoint::Container {
            dimension: dimension.to_owned(),
            position: *at,
        };
        let Ok(image) = inventory::load(state, &endpoint) else {
            continue;
        };
        for stack in image.slots.iter().filter(|stack| !stack.is_empty()) {
            if let Some(name) = state.registry.items.name_of(stack.item_id) {
                *totals.entry(name.path()).or_default() += i64::from(stack.count);
            }
        }
    }
    let mut top: Vec<_> = totals.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let items = if top.is_empty() {
        "empty".to_owned()
    } else {
        top.iter()
            .take(6)
            .map(|(name, count)| format!("{name} x{count}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("Storage: {} containers; {items}", points.len())
}

fn main_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let people = residents(state, ctx.id);
    let working = people
        .iter()
        .filter(|resident| resident.job.is_some())
        .count();
    let owner = record.owner_name.as_deref().unwrap_or(&record.owner);
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for building in record.buildings.values() {
        *counts.entry(category(building.kind)).or_default() += 1;
    }
    let certified = if counts.is_empty() {
        "none yet".to_owned()
    } else {
        counts
            .iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = format!(
        "Owner: {owner}\nRadius: {} blocks around {} {} {}\nResidents: {} ({working} working, {} idle)\nBuildings: {certified}\n{}",
        record.radius,
        record.anchor[0],
        record.anchor[1],
        record.anchor[2],
        people.len(),
        people.len() - working,
        storage_summary(state, ctx.id),
    );
    // Workers whose profession has no certified job: only certified workshops give jobs.
    let stranded = people
        .iter()
        .filter(|resident| {
            resident.job.is_none()
                && !matches!(resident.profession, Profession::None | Profession::Nitwit)
        })
        .count();
    if stranded > 0 {
        text.push_str(&format!(
            "\n{stranded} workers need a certified workshop — open Buildings → Certify"
        ));
    }
    if !ctx.manage {
        text.push_str("\nView only: the owner or an operator manages this settlement.");
    }
    let title = format!("Settlement {}", record.name);
    let buttons = vec![
        button("Buildings", "buildings", ctx.id),
        button("Residents & jobs", "residents", ctx.id),
        button("Production orders", "orders", ctx.id),
        button("Settings", "settings", ctx.id),
    ];
    present(state, connection, frame(title, note, text, buttons));
}

// ------------------------------------------------------------------- buildings

fn buildings_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let people = residents(state, ctx.id);
    let mut lines = Vec::new();
    let mut buttons = Vec::new();
    for building in record.buildings.values().take(MAX_LISTED) {
        let seq = short(building.id);
        let mut line = format!(
            "#{seq} {} L{} [{} .. {}]",
            kind_label(building.kind),
            building.level,
            corner_text(building.bounds.min),
            corner_text(building.bounds.max),
        );
        let slots = buildings::job_slots(building.kind, building.level);
        if slots > 0 {
            let filled = people
                .iter()
                .filter(|resident| resident.job == Some(building.id))
                .count();
            line.push_str(&format!(" jobs {filled}/{slots}"));
        }
        if building.level >= 3 {
            line.push_str(" - max level");
        } else {
            let next = building.level + 1;
            match buildings::inspect(state, building.kind, next, building.bounds) {
                Ok(_) => line.push_str(&format!(" - ready for L{next}")),
                Err(error) => {
                    line.push_str(&format!(" - L{next} needs: {}", missing_text(&error)));
                }
            }
            if ctx.manage {
                buttons.push(
                    button(format!("Upgrade #{seq} to L{next}"), "upgrade", ctx.id)
                        .with("b", Tag::String(building.id.0.to_string())),
                );
            }
        }
        lines.push(line);
    }
    if record.buildings.len() > MAX_LISTED {
        lines.push(format!("(+{} more)", record.buildings.len() - MAX_LISTED));
    }
    let mut text = if lines.is_empty() {
        "No certified buildings yet. Build freely, then certify the finished building: the settlement checks its blocks and size.".to_owned()
    } else {
        lines.join("\n")
    };
    if !ctx.manage {
        text.push_str("\nView only.");
    }
    if ctx.manage {
        buttons.push(button("Certify new building", "certify", ctx.id));
    }
    buttons.push(button("Back", "main", ctx.id));
    let title = format!("Buildings - {}", record.name);
    present(state, connection, frame(title, note, text, buttons));
}

fn form_of(state: &TickState, connection: ConnectionId, payload: ClickPayload<'_>) -> Form {
    let marks = state
        .settlement_marks
        .get(&connection)
        .copied()
        .unwrap_or_default();
    let here = player_pos(state, connection);
    let default_corner =
        |mark: Option<[i32; 3]>| mark.or(here).map(corner_text).unwrap_or_default();
    Form {
        kind: payload.string("kind").unwrap_or("house").to_owned(),
        from: payload
            .string("from")
            .map_or_else(|| default_corner(marks[0]), str::to_owned),
        to: payload
            .string("to")
            .map_or_else(|| default_corner(marks[1]), str::to_owned),
        extra: payload.float("extra").unwrap_or(4.0),
    }
}

fn set_mark(state: &mut TickState, connection: ConnectionId, index: usize, at: [i32; 3]) {
    if state.settlement_marks.len() >= MAX_MARKS
        && !state.settlement_marks.contains_key(&connection)
    {
        let connections = &state.connections;
        state
            .settlement_marks
            .retain(|other, _| connections.entity_of(*other).is_some());
    }
    state.settlement_marks.entry(connection).or_default()[index] = Some(at);
}

fn certify_screen(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    form: &Form,
    note: &str,
) {
    let kinds = all_kinds();
    let selected = BuildingKind::parse(&form.kind).unwrap_or(BuildingKind::House);
    let options = kinds
        .iter()
        .map(|kind| {
            (
                kind.name().to_owned(),
                Tag::String(format!("{}: {}", category(*kind), kind_label(*kind))),
            )
        })
        .collect();
    // Requirements are reported on failure (and per building under "needs"), not
    // as a paragraph: that keeps the form inside a small window.
    // Short labels, one button row and no Close button (Escape closes): the
    // form must fit a small window without scrolling.
    let buttons = vec![
        button("Certify", "certify_do", ctx.id).width(100).tooltip(
            "Corners are two opposite corners of the finished building, inclusive. Height adds blocks above the higher corner.",
        ),
        button("Corner 1 here", "mark1", ctx.id)
            .width(100)
            .tooltip("Stand in a corner block and press this; walk to the opposite corner, then /settlement menu certify and press Corner 2 here."),
        button("Corner 2 here", "mark2", ctx.id)
            .width(100)
            .tooltip("Stand in the opposite corner block and press this."),
        button("Back", "buildings", ctx.id).width(100),
    ];
    let mut dialog = Dialog::multi_action("Certify building", buttons).columns(4);
    if !note.is_empty() {
        dialog = dialog.text(note);
    }
    let dialog = dialog
        .input(
            Input::single_option("kind", "Building type", options).initial_option(selected.name()),
        )
        .input(
            Input::text("from", "Corner 1 (x y z)")
                .initial_text(&form.from)
                .max_length(40),
        )
        .input(
            Input::text("to", "Corner 2 (x y z)")
                .initial_text(&form.to)
                .max_length(40),
        )
        .input(
            Input::number_range("extra", "Height above corners", 0.0, MAX_EXTRA_HEIGHT)
                .initial_number(form.extra.clamp(0.0, MAX_EXTRA_HEIGHT).round())
                .step(1.0),
        );
    present(state, connection, dialog);
}

fn corner(text: &str) -> Option<[i32; 3]> {
    let mut parts = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|part| !part.is_empty());
    let at = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(at)
}

fn certify(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, payload: ClickPayload<'_>) {
    let form = form_of(state, connection, payload);
    let Some(kind) = BuildingKind::parse(&form.kind) else {
        return certify_screen(state, connection, ctx, &form, "Choose a building type.");
    };
    let (Some(a), Some(b)) = (corner(&form.from), corner(&form.to)) else {
        return certify_screen(
            state,
            connection,
            ctx,
            &form,
            "Corners must be three whole numbers: x y z.",
        );
    };
    let extra = form.extra.clamp(0.0, MAX_EXTRA_HEIGHT) as i32;
    let min = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
    let mut max = [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
    max[1] = max[1].saturating_add(extra);
    let Ok(bounds) = ScriptWorldBounds::new(min, max) else {
        return certify_screen(state, connection, ctx, &form, "Invalid corners.");
    };
    match buildings::certify(state, ctx.id, kind, bounds) {
        Ok(id) => {
            state.settlement_marks.remove(&connection);
            let note = format!("Certified {} #{} (level 1).", kind_label(kind), short(id));
            buildings_screen(state, connection, ctx, &note);
        }
        Err(error) => {
            let note = format!(
                "Cannot certify {}. Missing: {}.",
                kind_label(kind),
                missing_text(&error)
            );
            certify_screen(state, connection, ctx, &form, &note);
        }
    }
}

fn building_of(state: &TickState, ctx: &Ctx, payload: ClickPayload<'_>) -> Option<BuildingId> {
    let id = BuildingId(payload.string("b")?.parse().ok()?);
    state
        .settlements
        .get(ctx.id)?
        .buildings
        .contains_key(&id)
        .then_some(id)
}

fn upgrade(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, payload: ClickPayload<'_>) {
    let Some(id) = building_of(state, ctx, payload) else {
        return buildings_screen(state, connection, ctx, "That building no longer exists.");
    };
    let note = match buildings::upgrade(state, id) {
        Ok(()) => {
            let level = state
                .settlements
                .get(ctx.id)
                .and_then(|record| record.buildings.get(&id))
                .map_or(0, |building| building.level);
            format!("Building #{} upgraded to level {level}.", short(id))
        }
        Err(error) => format!(
            "Cannot upgrade #{}. Missing: {}.",
            short(id),
            missing_text(&error)
        ),
    };
    buildings_screen(state, connection, ctx, &note);
}

// ------------------------------------------------------------------- residents

fn residents_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let people = residents(state, ctx.id);
    let mut lines = Vec::new();
    let mut free = 0;
    for building in record.buildings.values() {
        let slots = buildings::job_slots(building.kind, building.level);
        if slots == 0 {
            continue;
        }
        let filled = people
            .iter()
            .filter(|resident| resident.job == Some(building.id))
            .count();
        free += slots.saturating_sub(filled);
        lines.push(format!(
            "#{} {} L{}: {filled}/{slots} jobs filled",
            short(building.id),
            kind_label(building.kind),
            building.level
        ));
    }
    lines.push(format!(
        "Free job slots: {free}; unemployed residents: {}",
        people
            .iter()
            .filter(|resident| resident.job.is_none())
            .count()
    ));
    lines.push(String::new());
    for resident in people.iter().take(MAX_RESIDENTS) {
        let name = super::presentation::villager_name(resident.uuid);
        let place = resident
            .job
            .and_then(|id| record.buildings.get(&id))
            .map_or_else(
                || "no job".to_owned(),
                |building| {
                    format!(
                        "works at #{} {}",
                        short(building.id),
                        kind_label(building.kind)
                    )
                },
            );
        lines.push(format!("{name} ({:?}): {place}", resident.profession));
    }
    if people.len() > MAX_RESIDENTS {
        lines.push(format!("(+{} more)", people.len() - MAX_RESIDENTS));
    }
    let title = format!("Residents & jobs - {}", record.name);
    let buttons = vec![button("Back", "main", ctx.id)];
    present(
        state,
        connection,
        frame(title, "", lines.join("\n"), buttons),
    );
}

// ---------------------------------------------------------------------- orders

fn job_status(job: &super::production::Job) -> String {
    if job.cancelled {
        "cancelled".to_owned()
    } else if job.done >= job.count {
        "done".to_owned()
    } else {
        match job.paused.as_deref() {
            Some("paused") => "paused".to_owned(),
            Some(reason) => format!("waiting: {reason}"),
            None => "working".to_owned(),
        }
    }
}

fn orders_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let mut jobs: Vec<_> = record.economy.production.jobs.values().collect();
    jobs.sort_by_key(|job| (job.cancelled || job.done >= job.count, job.accepted_tick));
    let mut lines = Vec::new();
    let mut buttons = Vec::new();
    for job in jobs.iter().take(MAX_LISTED) {
        let recipe = job.recipe.rsplit(':').next().unwrap_or(&job.recipe);
        lines.push(format!(
            "{}: {recipe} {}/{} by {} - {}",
            job.id,
            job.done,
            job.count,
            super::presentation::villager_name(job.worker),
            job_status(job)
        ));
        if ctx.manage && !job.cancelled && job.done < job.count {
            let (label, action) = if job.paused.as_deref() == Some("paused") {
                (format!("Resume {}", job.id), "order_resume")
            } else {
                (format!("Pause {}", job.id), "order_pause")
            };
            buttons.push(button(label, action, ctx.id).with("j", Tag::String(job.id.clone())));
            buttons.push(
                button(format!("Cancel {}", job.id), "order_cancel", ctx.id)
                    .with("j", Tag::String(job.id.clone())),
            );
        }
    }
    if jobs.len() > MAX_LISTED {
        lines.push(format!("(+{} more)", jobs.len() - MAX_LISTED));
    }
    let mut text = if lines.is_empty() {
        "No production orders yet.".to_owned()
    } else {
        lines.join("\n")
    };
    if ctx.manage {
        text.push_str(
            "\nA new order sends one resident to a crafting table inside a certified workshop; \
the resident crafts from the ingredients in their own inventory.",
        );
        buttons.insert(0, button("New order", "order_start", ctx.id));
    } else {
        text.push_str("\nView only.");
    }
    buttons.push(button("Back", "main", ctx.id));
    let title = format!("Production orders - {}", record.name);
    present(state, connection, frame(title, note, text, buttons));
}

/// Crafting stations inside certified workshops, a bounded scan on click only.
fn stations(state: &TickState, id: SettlementId) -> Vec<Station> {
    let wanted: BTreeSet<&str> = state
        .registry
        .item_facts
        .work_recipes()
        .map(|recipe| recipe.station.as_str())
        .collect();
    let mut found = Vec::new();
    let Some(record) = state.settlements.get(id) else {
        return found;
    };
    for building in record
        .buildings
        .values()
        .filter(|building| matches!(building.kind, BuildingKind::Workshop(_)))
        .take(MAX_LISTED)
    {
        let mut here = 0;
        'scan: for y in building.bounds.min[1]..=building.bounds.max[1] {
            for z in building.bounds.min[2]..=building.bounds.max[2] {
                for x in building.bounds.min[0]..=building.bounds.max[0] {
                    let Some(name) = block_name(state, [x, y, z]) else {
                        continue;
                    };
                    if wanted.contains(name) {
                        found.push(Station {
                            building: building.id,
                            kind: building.kind,
                            at: [x, y, z],
                            block: name.to_owned(),
                        });
                        here += 1;
                        if here == MAX_STATIONS {
                            break 'scan;
                        }
                    }
                }
            }
        }
    }
    found
}

fn order_start(state: &mut TickState, connection: ConnectionId, ctx: &Ctx) {
    let found = stations(state, ctx.id);
    match found.as_slice() {
        [] => orders_screen(
            state,
            connection,
            ctx,
            "No crafting table inside a certified workshop. Place a crafting table in a workshop and try again.",
        ),
        [only] => recipes_screen(state, connection, ctx, only.building, only.at),
        _ => {
            let buttons = found
                .iter()
                .map(|station| {
                    button(
                        format!(
                            "#{} {}: {} at {}",
                            short(station.building),
                            kind_label(station.kind),
                            station.block.rsplit(':').next().unwrap_or(&station.block),
                            corner_text(station.at)
                        ),
                        "order_pick",
                        ctx.id,
                    )
                    .with("b", Tag::String(station.building.0.to_string()))
                    .with("p", Tag::String(corner_text(station.at)))
                    .width(300)
                })
                .chain([button("Back", "orders", ctx.id)])
                .collect();
            let dialog = frame(
                "Choose a workstation".to_owned(),
                "",
                "Where should the resident work?".to_owned(),
                buttons,
            )
            .columns(1);
            present(state, connection, dialog);
        }
    }
}

fn order_pick(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    payload: ClickPayload<'_>,
) {
    let (Some(building), Some(at)) = (
        building_of(state, ctx, payload),
        payload.string("p").and_then(corner),
    ) else {
        return orders_screen(state, connection, ctx, "That workstation is gone.");
    };
    recipes_screen(state, connection, ctx, building, at);
}

fn recipes_screen(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    building: BuildingId,
    at: [i32; 3],
) {
    let Some(block) = block_name(state, at).map(str::to_owned) else {
        return orders_screen(state, connection, ctx, "That workstation is not loaded.");
    };
    let busy = busy_workers(state, ctx.id);
    let free: Vec<Resident> = residents(state, ctx.id)
        .into_iter()
        .filter(|resident| !busy.contains(&resident.uuid))
        .take(50)
        .collect();
    if free.is_empty() {
        return orders_screen(
            state,
            connection,
            ctx,
            "No free resident: bind villagers with /settlement bind, or wait for current orders to finish.",
        );
    }
    let workers = free
        .iter()
        .map(|resident| {
            (
                resident.uuid.to_string(),
                Tag::String(format!(
                    "{} ({:?})",
                    super::presentation::villager_name(resident.uuid),
                    resident.profession
                )),
            )
        })
        .collect();
    let mut buttons = Vec::new();
    for recipe in state
        .registry
        .item_facts
        .work_recipes()
        .filter(|recipe| recipe.station.as_str() == block)
        .take(60)
    {
        let outputs = recipe
            .outputs
            .iter()
            .map(|stack| format!("{} x{}", stack.item.path(), stack.count))
            .collect::<Vec<_>>()
            .join(", ");
        buttons.push(
            button(
                format!("{} -> {outputs}", recipe.id.path()),
                "order_create",
                ctx.id,
            )
            .with("b", Tag::String(building.0.to_string()))
            .with("p", Tag::String(corner_text(at)))
            .with("r", Tag::String(recipe.id.as_str().to_owned()))
            .width(300),
        );
    }
    buttons.push(button("Back", "orders", ctx.id));
    let dialog = frame(
        "New production order".to_owned(),
        "",
        format!(
            "Pick the product, a quantity and the resident. Crafting table at {}.",
            corner_text(at)
        ),
        buttons,
    )
    .columns(1)
    .input(
        Input::number_range("count", "Quantity", 1.0, MAX_ORDER)
            .initial_number(8.0)
            .step(1.0),
    )
    .input(Input::single_option("worker", "Resident", workers));
    present(state, connection, dialog);
}

/// A free cell beside `station` to stand on: air at feet and head, support below.
/// Cells inside the building win; order is fixed for determinism.
fn stand_at(state: &TickState, station: [i32; 3], bounds: ScriptWorldBounds) -> Option<[i32; 3]> {
    let air = state.registry.states.air;
    let is_air = |at: [i32; 3]| state.world.block_at(at[0], at[1], at[2]) == Some(air);
    let solid_below = |at: [i32; 3]| {
        state
            .world
            .block_at(at[0], at[1] - 1, at[2])
            .is_some_and(|block| block != air)
    };
    let inside = |at: [i32; 3]| (0..3).all(|i| bounds.min[i] <= at[i] && at[i] <= bounds.max[i]);
    let mut candidates = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            if (dx, dz) != (0, 0) {
                candidates.push([station[0] + dx, station[1], station[2] + dz]);
            }
        }
    }
    let usable =
        |at: &[i32; 3]| is_air(*at) && is_air([at[0], at[1] + 1, at[2]]) && solid_below(*at);
    candidates
        .iter()
        .copied()
        .find(|at| inside(*at) && usable(at))
        .or_else(|| candidates.iter().copied().find(usable))
}

fn failure_text(failure: Failure) -> String {
    match failure {
        Failure::Busy => "That resident already has an active order.".to_owned(),
        Failure::NotFound => "Resident, workshop or recipe not found.".to_owned(),
        Failure::Forbidden => "That resident does not belong to this settlement.".to_owned(),
        Failure::Blocked => "The workstation is missing or blocked.".to_owned(),
        Failure::Unloaded => "The workshop is not loaded.".to_owned(),
        other => format!("Refused: {other:?}."),
    }
}

fn order_create(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    payload: ClickPayload<'_>,
) {
    let (Some(building), Some(at), Some(recipe), Some(worker)) = (
        building_of(state, ctx, payload),
        payload.string("p").and_then(corner),
        payload
            .string("r")
            .and_then(|name| Identifier::parse(name).ok()),
        payload
            .string("worker")
            .and_then(|text| text.parse::<Uuid>().ok()),
    ) else {
        return orders_screen(state, connection, ctx, "The order form was incomplete.");
    };
    let count = payload
        .float("count")
        .map_or(1, |value| value.clamp(1.0, MAX_ORDER) as u32);
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let Some(found) = record.buildings.get(&building).cloned() else {
        return orders_screen(state, connection, ctx, "That workshop no longer exists.");
    };
    let valid = matches!(found.kind, BuildingKind::Workshop(_))
        && (0..3).all(|i| found.bounds.min[i] <= at[i] && at[i] <= found.bounds.max[i])
        && state
            .registry
            .item_facts
            .work_recipe(&recipe)
            .is_some_and(|work| block_name(state, at) == Some(work.station.as_str()));
    if !valid {
        return orders_screen(
            state,
            connection,
            ctx,
            "That recipe needs a different workstation than the one chosen.",
        );
    }
    let Some(stand) = stand_at(state, at, found.bounds) else {
        return orders_screen(
            state,
            connection,
            ctx,
            "No free standing cell next to that workstation.",
        );
    };
    let production = &record.economy.production;
    let mut sequence = production.sequence.saturating_add(1);
    let job = loop {
        let candidate = format!("ui-{sequence}");
        if !production.jobs.contains_key(&candidate) {
            break candidate;
        }
        sequence += 1;
    };
    let action = Action::ProductionStart {
        job: job.clone(),
        worker: worker.to_string(),
        recipe: recipe.as_str().to_owned(),
        count,
        building: ScriptSettlementBuildingRef::new(
            "building".to_owned(),
            building.0.to_string(),
            format!("solaris:{}", found.kind.name()),
        ),
        station: at,
        stand_at: stand,
    };
    let note = match economy_customization::execute(state, ctx.id, &action) {
        Ok(()) => format!("Order {job} started: {} x{count}.", recipe.path()),
        Err(failure) => failure_text(failure),
    };
    orders_screen(state, connection, ctx, &note);
}

fn order_control(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    action: &str,
    payload: ClickPayload<'_>,
) {
    let Some(job) = payload.string("j").map(str::to_owned).filter(|job| {
        state
            .settlements
            .get(ctx.id)
            .is_some_and(|record| record.economy.production.jobs.contains_key(job))
    }) else {
        return orders_screen(state, connection, ctx, "That order no longer exists.");
    };
    let request = match action {
        "order_pause" => Action::ProductionPause { job: job.clone() },
        "order_resume" => Action::ProductionResume { job: job.clone() },
        _ => Action::ProductionCancel { job: job.clone() },
    };
    let note = match economy_customization::execute(state, ctx.id, &request) {
        Ok(()) => format!("Order {job} updated."),
        Err(failure) => failure_text(failure),
    };
    orders_screen(state, connection, ctx, &note);
}

// -------------------------------------------------------------------- settings

fn settings_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let text = format!(
        "Name: {}\nRadius: {} blocks (grows with population)\nRevision: {}",
        record.name, record.radius, record.revision
    );
    let mut buttons = Vec::new();
    if ctx.manage {
        buttons.push(button("Rename", "rename", ctx.id));
    }
    buttons.push(button("Back", "main", ctx.id));
    let title = format!("Settings - {}", record.name);
    let mut dialog = frame(title, note, text, buttons);
    if ctx.manage {
        dialog = dialog.input(
            Input::text("name", "New name (letters, digits, - _)")
                .initial_text(&record.name)
                .max_length(64),
        );
    }
    present(state, connection, dialog);
}

fn rename(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, payload: ClickPayload<'_>) {
    let Some(name) = payload.string("name") else {
        return settings_screen(state, connection, ctx, "Enter a name.");
    };
    let note = match state.settlements.rename(ctx.id, name.to_owned()) {
        Ok(()) => format!("Renamed to {name}."),
        Err(message) => message.to_owned(),
    };
    settings_screen(state, connection, ctx, &note);
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
