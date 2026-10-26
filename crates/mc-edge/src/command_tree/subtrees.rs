//! Per-command subtrees (settlement, team, scoreboard, locate, tick, text).
//! Moved verbatim from `command_tree.rs`.

use super::*;

pub(super) fn settlement_tree(t: &mut Tree, root: i32) {
    t.literal(root, "guide", true);
    let menu = t.literal(root, "menu", true);
    t.argument(
        menu,
        "page",
        Parser::String(CommandStringKind::SingleWord),
        true,
    );
    let building = t.literal(root, "building", false);
    t.literal(building, "list", true);
    let certify = t.literal(building, "certify", false);
    let mut previous = t.argument(
        certify,
        "kind",
        Parser::String(CommandStringKind::SingleWord),
        false,
    );
    for (index, name) in ["x1", "y1", "z1", "x2", "y2", "z2"].into_iter().enumerate() {
        previous = t.argument(previous, name, integer(None, None), index == 5);
    }
    let upgrade = t.literal(building, "upgrade", false);
    t.argument(
        upgrade,
        "id",
        Parser::String(CommandStringKind::SingleWord),
        true,
    );
    let word = Parser::String(CommandStringKind::SingleWord);
    let build = t.literal(root, "build", false);
    t.argument(build, "blueprint", word.clone(), true);
    let upgrade = t.literal(build, "upgrade", false);
    t.argument(upgrade, "id", word.clone(), true);
    let create = t.literal(root, "create", false);
    let name = t.argument(create, "name", word.clone(), true);
    t.argument(name, "radius", integer(Some(1), Some(256)), true);
    t.literal(root, "claim", true);
    t.literal(root, "list", true);
    for action in ["info", "rename", "remove", "bind", "relocate"] {
        let operation = t.literal(root, action, action == "info");
        let name = t.argument(
            operation,
            "settlement",
            word.clone(),
            matches!(action, "info" | "remove" | "bind"),
        );
        match action {
            "rename" => {
                t.argument(name, "new_name", word.clone(), true);
            }
            "bind" => {
                t.argument(name, "villager", Parser::Uuid, true);
            }
            "relocate" => {
                let household = t.argument(name, "household", word.clone(), false);
                t.argument(household, "destination", word.clone(), true);
            }
            _ => {}
        }
    }
    let unbind = t.literal(root, "unbind", true);
    t.argument(unbind, "villager", Parser::Uuid, true);
    for domain in ["production", "commerce", "payroll", "caravan"] {
        let operation = t.literal(root, domain, false);
        let name = t.argument(operation, "settlement", word.clone(), false);
        t.literal(name, "status", true);
        for &(_, action, arguments) in SETTLEMENT_ECONOMY.iter().filter(|entry| entry.0 == domain) {
            let operation = t.literal(name, action, false);
            t.argument(
                operation,
                arguments,
                Parser::String(CommandStringKind::GreedyPhrase),
                true,
            );
        }
    }
    let army = t.literal(root, "army", false);
    t.literal(army, "list", true);
    t.literal(army, "recruit", true);
    let dismiss = t.literal(army, "dismiss", false);
    t.argument(dismiss, "soldier", Parser::Uuid, true);
    let order = t.literal(army, "order", false);
    let target = t.argument(order, "target", word.clone(), false);
    // The target word is `all` or a soldier UUID; the parser validates it.
    // Order verbs are executable leaves under the target.
    t.literals(target, &["follow", "guard", "patrol", "return"]);
}

pub(super) fn team_tree(t: &mut Tree, root: i32) {
    let list = t.literal(root, "list", true);
    t.argument(list, "team", Parser::Team, true);
    let add = t.literal(root, "add", false);
    let name = t.argument(
        add,
        "team",
        Parser::String(CommandStringKind::SingleWord),
        true,
    );
    t.argument(name, "displayName", Parser::Component, true);
    for name in ["remove", "empty", "join"] {
        let operation = t.literal(root, name, false);
        let team = t.argument(operation, "team", Parser::Team, true);
        if name == "join" {
            t.suggested(
                team,
                "members",
                Parser::ScoreHolder { multiple: true },
                true,
                "ask_server",
            );
        }
    }
    let leave = t.literal(root, "leave", false);
    t.suggested(
        leave,
        "members",
        Parser::ScoreHolder { multiple: true },
        true,
        "ask_server",
    );
    let modify = t.literal(root, "modify", false);
    let team = t.argument(modify, "team", Parser::Team, false);
    for name in ["displayName", "prefix", "suffix"] {
        let option = t.literal(team, name, false);
        t.argument(option, name, Parser::Component, true);
    }
    let color = t.literal(team, "color", false);
    t.argument(color, "value", Parser::Color, true);
    for name in ["friendlyFire", "seeFriendlyInvisibles"] {
        let option = t.literal(team, name, false);
        t.argument(option, "allowed", Parser::Bool, true);
    }
    let visibility = t.literal(team, "nametagVisibility", false);
    t.literals(
        visibility,
        &["always", "never", "hideForOtherTeams", "hideForOwnTeam"],
    );
    let collision = t.literal(team, "collisionRule", false);
    t.literals(
        collision,
        &["always", "never", "pushOtherTeams", "pushOwnTeam"],
    );
}

pub(super) fn scoreboard_tree(t: &mut Tree, root: i32) {
    let objectives = t.literal(root, "objectives", false);
    t.literal(objectives, "list", true);
    let add = t.literal(objectives, "add", false);
    let objective = t.argument(
        add,
        "objective",
        Parser::String(CommandStringKind::SingleWord),
        false,
    );
    let criteria = t.argument(objective, "criteria", Parser::ObjectiveCriteria, true);
    t.argument(criteria, "displayName", Parser::Component, true);
    let remove = t.literal(objectives, "remove", false);
    t.argument(remove, "objective", Parser::Objective, true);
    let setdisplay = t.literal(objectives, "setdisplay", false);
    let slot = t.argument(setdisplay, "slot", Parser::ScoreboardSlot, true);
    t.argument(slot, "objective", Parser::Objective, true);
    let players = t.literal(root, "players", false);
    let list = t.literal(players, "list", true);
    t.suggested(
        list,
        "target",
        Parser::ScoreHolder { multiple: false },
        true,
        "ask_server",
    );
    for (name, min) in [("set", None), ("add", Some(0)), ("remove", Some(0))] {
        let operation = t.literal(players, name, false);
        let targets = t.suggested(
            operation,
            "targets",
            Parser::ScoreHolder { multiple: true },
            false,
            "ask_server",
        );
        let objective = t.argument(targets, "objective", Parser::Objective, false);
        t.argument(objective, "score", integer(min, None), true);
    }
    let reset = t.literal(players, "reset", false);
    let targets = t.suggested(
        reset,
        "targets",
        Parser::ScoreHolder { multiple: true },
        true,
        "ask_server",
    );
    t.argument(targets, "objective", Parser::Objective, true);
    let enable = t.literal(players, "enable", false);
    let targets = t.suggested(
        enable,
        "targets",
        Parser::ScoreHolder { multiple: true },
        false,
        "ask_server",
    );
    t.argument(targets, "objective", Parser::Objective, true);
    let get = t.literal(players, "get", false);
    let target = t.suggested(
        get,
        "target",
        Parser::ScoreHolder { multiple: false },
        false,
        "ask_server",
    );
    t.argument(target, "objective", Parser::Objective, true);
    let operation = t.literal(players, "operation", false);
    let targets = t.suggested(
        operation,
        "targets",
        Parser::ScoreHolder { multiple: true },
        false,
        "ask_server",
    );
    let objective = t.argument(targets, "targetObjective", Parser::Objective, false);
    let operator = t.argument(objective, "operation", Parser::Operation, false);
    let source = t.suggested(
        operator,
        "source",
        Parser::ScoreHolder { multiple: true },
        false,
        "ask_server",
    );
    t.argument(source, "sourceObjective", Parser::Objective, true);
}

/// LocateCommand.java:56-87: `locate structure|biome|poi`, each with one
/// resource-or-tag argument. The structure argument is a *key* (registry
/// `minecraft:worldgen/structure`); biome and POI are resolved values
/// (registries `minecraft:worldgen/biome`, `minecraft:point_of_interest_type`).
/// Vanilla gives the structure argument server suggestions (the client has no
/// structure registry); we advertise `ask_server` for it the same way.
pub(super) fn locate_tree(t: &mut Tree) {
    let Some(root) = t.root("locate", false) else {
        return;
    };
    let structure = t.literal(root, "structure", false);
    t.suggested(
        structure,
        "structure",
        Parser::ResourceOrTagKey {
            registry: identifier("worldgen/structure"),
        },
        true,
        "ask_server",
    );
    let biome = t.literal(root, "biome", false);
    t.argument(
        biome,
        "biome",
        Parser::ResourceOrTag {
            registry: identifier("worldgen/biome"),
        },
        true,
    );
    let poi = t.literal(root, "poi", false);
    t.argument(
        poi,
        "poi",
        Parser::ResourceOrTag {
            registry: identifier("point_of_interest_type"),
        },
        true,
    );
}

/// TickCommand.java:20-55: query/rate (float 1..10000, suggests 20)/
/// step (bare 1, stop, or time>=1 suggesting 1t/1s)/sprint (stop or time>=1
/// suggesting 60s/1d/3d)/unfreeze/freeze. `tick` is LEVEL_ADMINS (op 3).
pub(super) fn tick_tree(t: &mut Tree) {
    let Some(root) = t.root("tick", false) else {
        return;
    };
    t.literal(root, "query", true);
    let rate = t.literal(root, "rate", false);
    t.suggested(
        rate,
        "rate",
        Parser::Float {
            min: Some(1.0f32.to_bits()),
            max: Some(10000.0f32.to_bits()),
        },
        true,
        "ask_server",
    );
    let step = t.literal(root, "step", true);
    t.literal(step, "stop", true);
    t.suggested(step, "time", Parser::Time { min: 1 }, true, "ask_server");
    let sprint = t.literal(root, "sprint", false);
    t.literal(sprint, "stop", true);
    t.suggested(sprint, "time", Parser::Time { min: 1 }, true, "ask_server");
    t.literal(root, "unfreeze", true);
    t.literal(root, "freeze", true);
}

/// TellRawCommand.java:12-30, TitleCommand.java:26-102,
/// BossBarCommands.java:65-176. All three are LEVEL_GAMEMASTERS (op 2).
/// BossBar id suggestions use `ask_server` exactly where vanilla attaches
/// `SUGGEST_BOSS_BAR`.
pub(super) fn tellraw_title_bossbar_trees(t: &mut Tree) {
    if let Some(root) = t.root("tellraw", false) {
        let targets = t.argument(root, "targets", players(false), false);
        t.argument(targets, "message", Parser::Component, true);
    }
    if let Some(root) = t.root("title", false) {
        let targets = t.argument(root, "targets", players(false), false);
        t.literal(targets, "clear", true);
        t.literal(targets, "reset", true);
        for name in ["title", "subtitle", "actionbar"] {
            let section = t.literal(targets, name, false);
            t.argument(section, "title", Parser::Component, true);
        }
        let times = t.literal(targets, "times", false);
        let fade_in = t.argument(times, "fadeIn", Parser::Time { min: 0 }, false);
        let stay = t.argument(fade_in, "stay", Parser::Time { min: 0 }, false);
        t.argument(stay, "fadeOut", Parser::Time { min: 0 }, true);
    }
    let Some(root) = t.root("bossbar", false) else {
        return;
    };
    let add = t.literal(root, "add", false);
    let id = t.argument(add, "id", Parser::ResourceLocation, false);
    t.argument(id, "name", Parser::Component, true);
    let remove = t.literal(root, "remove", false);
    t.suggested(remove, "id", Parser::ResourceLocation, true, "ask_server");
    t.literal(root, "list", true);
    let set = t.literal(root, "set", false);
    let id = t.suggested(set, "id", Parser::ResourceLocation, false, "ask_server");
    let name = t.literal(id, "name", false);
    t.argument(name, "name", Parser::Component, true);
    let color = t.literal(id, "color", false);
    t.literals(
        color,
        &["pink", "blue", "red", "green", "yellow", "purple", "white"],
    );
    let style = t.literal(id, "style", false);
    t.literals(
        style,
        &[
            "progress",
            "notched_6",
            "notched_10",
            "notched_12",
            "notched_20",
        ],
    );
    let value = t.literal(id, "value", false);
    t.argument(value, "value", integer(Some(0), None), true);
    let max = t.literal(id, "max", false);
    t.argument(max, "max", integer(Some(1), None), true);
    let visible = t.literal(id, "visible", false);
    t.argument(visible, "visible", Parser::Bool, true);
    let players = t.literal(id, "players", true);
    t.argument(players, "targets", entities(false), true);
    let get = t.literal(root, "get", false);
    let id = t.suggested(get, "id", Parser::ResourceLocation, false, "ask_server");
    t.literals(id, &["value", "max", "visible", "players"]);
}
