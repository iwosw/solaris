//! Supported 26.1.2 command syntax, filtered before it reaches the client.
//!
//! Source: ../solaris-loader/loader-neoforge/build/moddev/artifacts/
//! minecraft-patched-26.1.2.76-sources.jar, net/minecraft/server/commands/.
//! Registration evidence: GameModeCommand.java:22-34, TeleportCommand.java:38-143,
//! GiveCommand.java:26-48, ClearInventoryCommands.java:29-58,
//! ExperienceCommand.java:27-130, TimeCommand.java:48-113,
//! WeatherCommand.java:20-50, DifficultyCommand.java:22-33,
//! GameRuleCommand.java:15-33, SummonCommand.java:36-75,
//! SetBlockCommand.java:32-102, FillCommand.java:40-155,
//! SetSpawnCommand.java:24-69, SetWorldSpawnCommand.java:18-31,
//! EnchantCommand.java:40-67, TeamCommand.java:64-258,
//! ScoreboardCommand.java:70-380, MsgCommand.java:19-33,
//! HelpCommand.java:18-48, ListPlayersCommand.java:16-21,
//! EmoteCommands.java:15-25, BanPlayerCommands.java:22-37,
//! KickCommand.java:22-36, SaveAllCommand.java:15-21.
//! BanIpCommands.java:26-38, PardonIpCommand.java:22-31.
//! KillCommand.java:17-23, SayCommand.java:15-28, SeedCommand.java:12-21,
//! OpCommand.java:23-38, DeOpCommands.java:21-31, PardonCommand.java:21-31,
//! StopCommand.java:11-18. Debug give is a retained Solaris fixture.
//! EffectCommands.java:32-151.
//! AdvancementCommands.java:33-229 (resource-key advancement and greedy criterion).
//! LocateCommand.java:56-87, TickCommand.java:20-55,
//! TellRawCommand.java:12-30, TitleCommand.java:26-102,
//! BossBarCommands.java:65-176.
//! Suggestion provider names: net/minecraft/commands/synchronization/
//! SuggestionProviders.java:19-40. Unsupported vanilla branches (such as
//! teleport facing and fill strict) are intentionally not advertised.

use mc_core::command_rules::{RuleKind, rules};
use mc_data::Identifier;
use mc_protocol::packets::play::{
    ClientboundCommandSuggestions, ClientboundCommands, CommandArgumentParser as Parser,
    CommandNode, CommandNodeKind, CommandStringKind, CommandSuggestionEntry,
};

const ROOTS: &[(&str, u8)] = &[
    ("help", 0),
    ("list", 0),
    ("me", 0),
    ("msg", 0),
    ("tell", 0),
    ("w", 0),
    ("teammsg", 0),
    ("tm", 0),
    ("settlement", 0),
    ("gamemode", 2),
    ("teleport", 2),
    ("tp", 2),
    ("execute", 2),
    ("give", 2),
    ("clear", 2),
    ("kill", 2),
    ("time", 2),
    ("weather", 2),
    ("difficulty", 2),
    ("gamerule", 2),
    ("summon", 2),
    ("worldborder", 2),
    ("setblock", 2),
    ("fill", 2),
    ("experience", 2),
    ("xp", 2),
    ("spawnpoint", 2),
    ("setworldspawn", 2),
    ("forceload", 2),
    ("say", 2),
    ("seed", 2),
    ("enchant", 2),
    ("effect", 2),
    ("advancement", 2),
    ("debug", 2),
    ("team", 2),
    ("tps", 2),
    ("mspt", 2),
    ("scoreboard", 2),
    ("locate", 2),
    ("tellraw", 2),
    ("title", 2),
    ("bossbar", 2),
    ("function", 2),
    ("schedule", 2),
    ("datapack", 2),
    ("reload", 2),
    ("data", 2),
    ("tick", 3),
    ("kick", 3),
    ("mute", 3),
    ("unmute", 3),
    ("warn", 3),
    ("transfer", 3),
    ("ban", 3),
    ("ban-ip", 3),
    ("pardon", 3),
    ("pardon-ip", 3),
    ("op", 3),
    ("deop", 3),
    ("whitelist", 3),
    ("banlist", 3),
    ("co", 3),
    ("stop", 4),
    ("save-all", 4),
];

const SETTLEMENT_HELP: &[(&str, &str)] = &[
    ("guide", "Step-by-step in-game settlement tutorial."),
    (
        "menu",
        "Open the settlement dialogs (also: /settlement alone). Pages: main, buildings, certify, residents, production, army, projects, settings.",
    ),
    (
        "building",
        "Certify or upgrade player-built buildings: list | certify <kind> <x1 y1 z1 x2 y2 z2> | upgrade <id>.",
    ),
    (
        "build",
        "Start a fully funded blueprint building or native-built upgrade: <blueprint> | upgrade <buildingId>. Owner/operator of the claimed settlement here; one active project, real materials from storage.",
    ),
    (
        "create",
        "Found a settlement here; optional radius 1-256 (default 48).",
    ),
    (
        "claim",
        "Claim the autonomous settlement you stand in (ownerless villages refuse other mutations).",
    ),
    ("list", "List all settlements."),
    ("info", "Show owner, centre, radius and resident count."),
    ("rename", "Rename a settlement (owner/operator)."),
    (
        "remove",
        "Dissolve a settlement and release membership (owner/operator).",
    ),
    (
        "bind",
        "Bind the nearest villager within 8 blocks, or supply a UUID (owner/operator).",
    ),
    (
        "unbind",
        "Release the nearest villager within 8 blocks, or supply a UUID (owner/operator).",
    ),
    (
        "relocate",
        "Move a household; requires ownership of both towns or operator.",
    ),
    (
        "production",
        "Run real worker recipes: status/start/pause/resume/cancel.",
    ),
    (
        "commerce",
        "Trade and escrow: status/quote/accept/cancel/reserve/payout.",
    ),
    (
        "payroll",
        "Fund worker contracts: status/employ/stop/dismiss/pay.",
    ),
    (
        "caravan",
        "Physical transport: status/open/pickup/unload/cancel/block.",
    ),
    (
        "army",
        "Native soldiers: list | recruit | dismiss <uuid> | order <all|uuid> follow|guard|patrol|return (owner/operator for mutations).",
    ),
];

pub(crate) const SETTLEMENT_GUIDE: &str = concat!(
    "Settlements / Поселения — quick start:\n",
    "1. Stand at your town centre: /settlement create Oak (optional radius 1-256; default 48).\n",
    "   Then /settlement (or /settlement menu) opens the dialogs: buildings (certify/upgrade), residents & jobs, production orders, settings.\n",
    "2. Place beds and workstations: composter=Farmer, lectern=Librarian, barrel=Fisherman; /help settlement lists all jobs.\n",
    "3. Keep the village loaded: adults take free stations during day ticks 2000-8999, within 48 blocks via a walkable path. Nitwits never work; traded villagers keep their profession.\n",
    "4. Stand next to a villager: /settlement bind Oak (nearest within 8 blocks). /settlement unbind releases the nearest; optional native UUID targets one explicitly. Villagers work without binding; bound villagers search inside the town.\n",
    "5. Read villagers' nameplates/status labels; /settlement info Oak shows population and jobs, /settlement list shows towns. Changes require owner/operator; reads and founding are public.\n",
    "6. /settlement production Oak status — real worker recipes (start/pause/resume/cancel).\n",
    "7. /settlement commerce Oak status — emerald trades and escrow (quote/accept/cancel/reserve/payout).\n",
    "8. /settlement payroll Oak status — funded worker contracts (employ/stop/dismiss/pay).\n",
    "9. /settlement caravan Oak status — physical deliveries (open/pickup/unload/cancel/block).\n",
    "Action syntax: /help settlement <domain> Oak. Short bilingual guide: docs/SETTLEMENTS_GUIDE.md",
);

// Native economy uses compact endpoint/coordinate/route tokens (":" / "," /
// ";") that Brigadier's unquoted word parser rejects. Advertise each action's
// native tail as a greedy string, leaving exact token validation to the tick.
const SETTLEMENT_ECONOMY: &[(&str, &str, &str)] = &[
    (
        "production",
        "start",
        "job worker-UUID recipe count building station-x,y,z stand-at-x,y,z",
    ),
    ("production", "pause", "job"),
    ("production", "resume", "job"),
    ("production", "cancel", "job"),
    (
        "commerce",
        "quote",
        "quote seller-endpoint buyer-endpoint slot quantity price",
    ),
    ("commerce", "accept", "quote"),
    ("commerce", "cancel", "quote"),
    (
        "commerce",
        "reserve",
        "mandate shipment payer-endpoint escrow-endpoint payee-endpoint price",
    ),
    ("commerce", "payout", "mandate"),
    (
        "payroll",
        "employ",
        "contract worker-UUID treasury-endpoint wage period budget",
    ),
    ("payroll", "stop", "contract"),
    ("payroll", "dismiss", "contract"),
    ("payroll", "pay", "contract"),
    (
        "caravan",
        "open",
        "shipment carrier-UUID src-building src-x,y,z dst-building dst-x,y,z slot quantity ticks waypoints [paid-mandate]",
    ),
    ("caravan", "pickup", "shipment"),
    ("caravan", "unload", "shipment"),
    ("caravan", "cancel", "shipment"),
    ("caravan", "block", "shipment reason"),
];

pub(crate) fn permitted(command: &str, op_level: u8, plugins: &[(String, u8)]) -> bool {
    command
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .is_some_and(|root| {
            if let Some((_, required)) = plugins.iter().find(|(name, _)| root == name) {
                op_level >= *required
            } else {
                ROOTS
                    .iter()
                    .any(|&(name, required)| root == name && op_level >= required)
            }
        })
}

/// Permission-filtered, tree-derived usage for actual command arguments and
/// optional named argument placeholders, including alias redirects.
///
/// Rendering follows `CommandDispatcher.getSmartUsage`:
/// https://github.com/Mojang/brigadier/blob/master/src/main/java/com/mojang/brigadier/CommandDispatcher.java
/// The full path prefix follows vanilla `HelpCommand.java:18-50`.
pub(crate) fn help(
    command: Option<&str>,
    op_level: u8,
    plugins: &[(String, u8)],
) -> Result<Vec<String>, String> {
    let packet = tree(op_level, plugins);
    let path = command.map(|path| path.trim().trim_start_matches('/'));
    let mut index = packet.root_index;
    if let Some(path) = path {
        if path.is_empty() {
            return Err("Unknown or unavailable command path.".to_owned());
        }
        let (parsed, consumed) = help_path(&packet, index, path);
        if consumed == 0 {
            return Err(format!("Unknown or unavailable command path: {path}"));
        }
        index = parsed;
    }
    if let Some(redirect) = packet.nodes[index as usize].redirect {
        index = redirect;
    }
    let parent = &packet.nodes[index as usize];
    let settlement_help =
        path == Some("settlement") && !plugins.iter().any(|(root, _)| root == "settlement");
    let mut usages =
        Vec::with_capacity(parent.children.len() + if settlement_help { 4 } else { 0 });
    for &child in &parent.children {
        let mut line = String::from("/");
        if let Some(path) = path {
            line.push_str(path);
            line.push(' ');
        }
        smart_usage(&packet, child, parent.executable, false, &mut line);
        if settlement_help
            && let CommandNodeKind::Literal(name) = &packet.nodes[child as usize].kind
            && let Some((_, description)) = SETTLEMENT_HELP.iter().find(|entry| entry.0 == name)
        {
            line.push_str(" — ");
            line.push_str(description);
        }
        usages.push(line);
    }
    if settlement_help {
        usages.push("Jobs: adults use free workstations during day ticks 2000-8999, within 48 blocks with a walkable path; keep the village loaded.".into());
        usages.push("composter=Farmer, lectern=Librarian, barrel=Fisherman, smoker=Butcher, blast_furnace=Armorer, brewing_stand=Cleric.".into());
        usages.push("cartography_table=Cartographer, fletching_table=Fletcher, cauldron=Leatherworker, stonecutter=Mason, loom=Shepherd, smithing_table=Toolsmith, grindstone=Weaponsmith.".into());
        usages.push("Unbound villagers need no command; bound villagers search inside their town. Nitwits never work; traded villagers keep their profession. Guide: docs/SETTLEMENTS_GUIDE.md".into());
    }
    Ok(usages)
}

/// Follow the branch that parses the most input. Literal matches take priority,
/// and an incomplete suffix retains its last parsed node, as vanilla help does.
fn help_path(packet: &ClientboundCommands, mut index: i32, input: &str) -> (i32, usize) {
    if let Some(redirect) = packet.nodes[index as usize].redirect {
        index = redirect;
    }
    let tail = input.trim_start();
    let leading = input.len() - tail.len();
    if tail.is_empty() {
        return (index, leading);
    }
    let word_end = tail.find(char::is_whitespace).unwrap_or(tail.len());
    let word = &tail[..word_end];
    let node = &packet.nodes[index as usize];
    if let Some(&literal) = node.children.iter().find(|&&child| {
        matches!(&packet.nodes[child as usize].kind, CommandNodeKind::Literal(name) if name == word)
    }) {
        let (parsed, consumed) = help_path(packet, literal, &tail[word_end..]);
        return (parsed, leading + word_end + consumed);
    }
    let mut best = (index, 0);
    for &child in &node.children {
        let CommandNodeKind::Argument { name, parser, .. } = &packet.nodes[child as usize].kind
        else {
            continue;
        };
        let placeholder = name == word
            || word
                .strip_prefix('<')
                .and_then(|word| word.strip_suffix('>'))
                == Some(name.as_str());
        let Some(length) = (if placeholder {
            Some(word_end)
        } else {
            help_argument_length(parser, tail)
        }) else {
            continue;
        };
        let (parsed, consumed) = help_path(packet, child, &tail[length..]);
        let consumed = leading + length + consumed;
        if consumed > best.1 {
            best = (parsed, consumed);
        }
    }
    best
}

/// Lexical argument traversal, not execution or registry/target resolution.
/// Balanced component/item/block/selector text stays a single argument.
fn help_token_length(input: &str) -> Option<usize> {
    let mut square = 0usize;
    let mut curly = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, c) in input.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == delimiter {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '[' => square += 1,
            ']' => square = square.checked_sub(1)?,
            '{' => curly += 1,
            '}' => curly = curly.checked_sub(1)?,
            _ if c.is_whitespace() && square == 0 && curly == 0 => return Some(index),
            _ => {}
        }
    }
    (quote.is_none() && square == 0 && curly == 0 && !input.is_empty()).then_some(input.len())
}

fn help_coordinates(input: &str, count: usize, integer: bool) -> Option<usize> {
    let mut consumed = 0;
    for _ in 0..count {
        let remaining = &input[consumed..];
        let tail = remaining.trim_start();
        consumed += remaining.len() - tail.len();
        let length = tail.find(char::is_whitespace).unwrap_or(tail.len());
        let coordinate = &tail[..length];
        let relative = coordinate.starts_with(['~', '^']);
        let number = if relative {
            &coordinate[1..]
        } else {
            coordinate
        };
        if !(relative && number.is_empty()) {
            if integer && !relative {
                number.parse::<i32>().ok()?;
            } else if !number.parse::<f64>().ok()?.is_finite() {
                return None;
            }
        }
        consumed += length;
    }
    Some(consumed)
}

fn help_argument_length(parser: &Parser, input: &str) -> Option<usize> {
    match parser {
        Parser::Message | Parser::String(CommandStringKind::GreedyPhrase) => {
            return Some(input.len());
        }
        Parser::Vec3 => return help_coordinates(input, 3, false),
        Parser::BlockPos => return help_coordinates(input, 3, true),
        Parser::Vec2 | Parser::Rotation => return help_coordinates(input, 2, false),
        Parser::ColumnPos => return help_coordinates(input, 2, true),
        Parser::Angle => return help_coordinates(input, 1, false),
        _ => {}
    }
    let length = help_token_length(input)?;
    let word = &input[..length];
    let valid = match parser {
        Parser::Bool => matches!(word, "true" | "false"),
        Parser::Integer { min, max } => word.parse::<i32>().ok().is_some_and(|value| {
            min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
        }),
        Parser::Long { min, max } => word.parse::<i64>().ok().is_some_and(|value| {
            min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
        }),
        Parser::Float { min, max } => word.parse::<f32>().ok().is_some_and(|value| {
            value.is_finite()
                && min.is_none_or(|min| value >= f32::from_bits(min))
                && max.is_none_or(|max| value <= f32::from_bits(max))
        }),
        Parser::Double { min, max } => word.parse::<f64>().ok().is_some_and(|value| {
            value.is_finite()
                && min.is_none_or(|min| value >= f64::from_bits(min))
                && max.is_none_or(|max| value <= f64::from_bits(max))
        }),
        Parser::Time { min } => {
            let (number, multiplier) = match word.as_bytes().last() {
                Some(b'd') => (&word[..word.len() - 1], 24_000.0),
                Some(b's') => (&word[..word.len() - 1], 20.0),
                Some(b't') => (&word[..word.len() - 1], 1.0),
                _ => (word, 1.0),
            };
            number.parse::<f64>().ok().is_some_and(|number| {
                let ticks = (number * multiplier).round();
                ticks.is_finite() && ticks >= f64::from(*min) && ticks <= f64::from(i32::MAX)
            })
        }
        Parser::GameMode => matches!(word, "survival" | "creative" | "adventure" | "spectator"),
        Parser::String(CommandStringKind::SingleWord) => word
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'+')),
        Parser::Entity {
            single,
            players_only,
        } => {
            if word.starts_with('@') {
                let selector = word.as_bytes().get(1).copied();
                matches!(selector, Some(b'a' | b'e' | b'n' | b'p' | b'r' | b's'))
                    && (!single
                        || !matches!(selector, Some(b'a' | b'e'))
                        || word.contains("limit=1"))
                    && (!players_only
                        || !matches!(selector, Some(b'e' | b'n'))
                        || word.contains("type=player")
                        || word.contains("type=minecraft:player"))
            } else {
                word.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            }
        }
        Parser::ResourceLocation | Parser::Resource { .. } | Parser::ResourceKey { .. } => {
            word.bytes().all(|c| {
                c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || matches!(c, b'_' | b'-' | b'.' | b'/' | b':')
            })
        }
        Parser::Operation => matches!(
            word,
            "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "<" | ">" | "><"
        ),
        _ => true,
    };
    valid.then_some(length)
}

fn usage_text(node: &CommandNode, output: &mut String) {
    match &node.kind {
        CommandNodeKind::Root => {}
        CommandNodeKind::Literal(name) => output.push_str(name),
        CommandNodeKind::Argument { name, .. } => {
            output.push('<');
            output.push_str(name);
            output.push('>');
        }
    }
}

fn smart_usage(
    packet: &ClientboundCommands,
    index: i32,
    optional: bool,
    deep: bool,
    output: &mut String,
) {
    let node = &packet.nodes[index as usize];
    if optional {
        output.push('[');
    }
    usage_text(node, output);
    if optional {
        output.push(']');
    }
    if deep {
        return;
    }
    if let Some(redirect) = node.redirect {
        if redirect == packet.root_index {
            output.push_str(" ...");
        } else {
            output.push_str(" -> ");
            usage_text(&packet.nodes[redirect as usize], output);
        }
        return;
    }
    match node.children.as_slice() {
        [] => {}
        [child] => {
            output.push(' ');
            smart_usage(packet, *child, node.executable, node.executable, output);
        }
        children => {
            // This tree has distinct sibling usage texts, so Brigadier's
            // alternative deduplication never needs an intermediate set.
            output.push(' ');
            output.push(if node.executable { '[' } else { '(' });
            for (position, &child) in children.iter().enumerate() {
                if position > 0 {
                    output.push('|');
                }
                usage_text(&packet.nodes[child as usize], output);
            }
            output.push(if node.executable { ']' } else { ')' });
        }
    }
}

fn identifier(path: &str) -> Identifier {
    Identifier::parse(format!("minecraft:{path}")).expect("static vanilla identifier")
}

/// Vanilla data-pack structure ids (plus every `#tag`) backing
/// `locate structure` server suggestions: the client has no structure
/// registry, so the tree advertises `ask_server` for it (LocateCommand
/// suggests through the command context, not a synced registry).
/// Filesystem layout is `data/minecraft/worldgen/structure/*.json` and
/// `data/minecraft/tags/worldgen/structure/*.json`; entries load the same
/// way startup suggestion lists do. Missing/unreadable sidecar data yields
/// an empty list rather than failing startup.
pub(crate) fn structure_suggestion_ids(vanilla_data_dir: &std::path::Path) -> Vec<String> {
    use std::collections::BTreeSet;
    let root = vanilla_data_dir.join("data").join("minecraft");
    let mut ids = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(root.join("worldgen").join("structure")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                ids.insert(format!("minecraft:{stem}"));
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(root.join("tags").join("worldgen").join("structure")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                ids.insert(format!("#minecraft:{stem}"));
            }
        }
    }
    ids.into_iter().collect()
}

fn integer(min: Option<i32>, max: Option<i32>) -> Parser {
    Parser::Integer {
        min: min.filter(|&value| value != i32::MIN),
        max: max.filter(|&value| value != i32::MAX),
    }
}

fn players(single: bool) -> Parser {
    Parser::Entity {
        single,
        players_only: true,
    }
}

fn entities(single: bool) -> Parser {
    Parser::Entity {
        single,
        players_only: false,
    }
}

struct Tree<'a> {
    nodes: Vec<CommandNode>,
    op_level: u8,
    plugins: &'a [(String, u8)],
}

impl<'a> Tree<'a> {
    fn new(op_level: u8, plugins: &'a [(String, u8)]) -> Self {
        let capacity = if op_level >= 2 {
            rules().len() * 4 + 256
        } else {
            256
        };
        let mut nodes = Vec::with_capacity(capacity);
        nodes.push(CommandNode::root(Vec::with_capacity(ROOTS.len())));
        Self {
            nodes,
            op_level,
            plugins,
        }
    }

    fn root(&mut self, name: &str, executable: bool) -> Option<i32> {
        if self.plugins.iter().any(|(root, _)| root == name) {
            return None;
        }
        let level = ROOTS.iter().find(|(root, _)| *root == name)?.1;
        if self.op_level < level {
            return None;
        }
        let node = self.literal(0, name, executable);
        self.nodes[node as usize].restricted = level > 0;
        Some(node)
    }

    /// Keep the native schema reachable for its independently owned aliases,
    /// without advertising a second root when a plugin owns the literal.
    fn aliased_root(&mut self, name: &str, executable: bool) -> Option<i32> {
        if !self.plugins.iter().any(|(root, _)| root == name) {
            return self.root(name, executable);
        }
        let level = ROOTS.iter().find(|(root, _)| *root == name)?.1;
        if self.op_level < level {
            return None;
        }
        let index = self.nodes.len() as i32;
        let mut node = CommandNode::literal(name, Vec::new(), executable);
        node.restricted = level > 0;
        self.nodes.push(node);
        Some(index)
    }

    fn push(&mut self, parent: i32, node: CommandNode) -> i32 {
        let index = self.nodes.len() as i32;
        self.nodes.push(node);
        self.nodes[parent as usize].children.push(index);
        index
    }

    fn literal(&mut self, parent: i32, name: &str, executable: bool) -> i32 {
        self.push(parent, CommandNode::literal(name, Vec::new(), executable))
    }

    fn argument(&mut self, parent: i32, name: &str, parser: Parser, executable: bool) -> i32 {
        self.push(
            parent,
            CommandNode::argument(name, parser, Vec::new(), executable),
        )
    }

    fn suggested(
        &mut self,
        parent: i32,
        name: &str,
        parser: Parser,
        executable: bool,
        provider: &str,
    ) -> i32 {
        let index = self.argument(parent, name, parser, executable);
        if let CommandNodeKind::Argument { suggestions, .. } = &mut self.nodes[index as usize].kind
        {
            *suggestions = Some(identifier(provider));
        }
        index
    }

    fn literals(&mut self, parent: i32, names: &[&str]) {
        for name in names {
            self.literal(parent, name, true);
        }
    }

    fn alias(&mut self, name: &str, target: i32) {
        if let Some(index) = self.root(name, false) {
            self.nodes[index as usize].redirect = Some(target);
        }
    }
}

pub(crate) fn tree(op_level: u8, plugins: &[(String, u8)]) -> ClientboundCommands {
    let mut t = Tree::new(op_level, plugins);
    t.root("reload", true);
    if let Some(root) = t.root("data", false) {
        t.argument(
            root,
            "arguments",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
    if let Some(root) = t.root("function", false) {
        t.argument(
            root,
            "name",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
    if let Some(root) = t.root("schedule", false) {
        let function = t.literal(root, "function", false);
        t.argument(
            function,
            "arguments",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
        let clear = t.literal(root, "clear", false);
        t.argument(
            clear,
            "name",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
    if let Some(root) = t.root("datapack", false) {
        let list = t.literal(root, "list", true);
        t.literal(list, "enabled", true);
    }
    if let Some(root) = t.root("help", true) {
        t.argument(
            root,
            "command",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
    if let Some(root) = t.root("list", true) {
        t.literal(root, "uuids", true);
    }
    if let Some(root) = t.root("me", false) {
        t.argument(root, "action", Parser::Message, true);
    }
    if let Some(root) = t.aliased_root("msg", false) {
        let targets = t.argument(root, "targets", players(false), false);
        t.argument(targets, "message", Parser::Message, true);
        t.alias("tell", root);
        t.alias("w", root);
    }
    if let Some(root) = t.aliased_root("teammsg", false) {
        t.argument(root, "message", Parser::Message, true);
        t.alias("tm", root);
    }
    if let Some(root) = t.root("settlement", false) {
        settlement_tree(&mut t, root);
    }
    if let Some(root) = t.root("gamemode", false) {
        let mode = t.argument(root, "gamemode", Parser::GameMode, true);
        t.argument(mode, "target", players(false), true);
    }
    if let Some(root) = t.aliased_root("teleport", false) {
        let location = t.argument(root, "location", Parser::Vec3, true);
        t.argument(location, "rotation", Parser::Rotation, true);
        t.argument(root, "destination", entities(true), true);
        let targets = t.argument(root, "targets", entities(false), false);
        let location = t.argument(targets, "location", Parser::Vec3, true);
        t.argument(location, "rotation", Parser::Rotation, true);
        t.argument(targets, "destination", entities(true), true);
        t.alias("tp", root);
    }
    if let Some(root) = t.root("execute", false) {
        t.argument(
            root,
            "arguments",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
    if let Some(root) = t.root("give", false) {
        let targets = t.argument(root, "targets", players(false), false);
        let item = t.argument(targets, "item", Parser::ItemStack, true);
        t.argument(item, "count", integer(Some(1), None), true);
    }
    if let Some(root) = t.root("clear", true) {
        let targets = t.argument(root, "targets", players(false), true);
        let item = t.argument(targets, "item", Parser::ItemPredicate, true);
        t.argument(item, "maxCount", integer(Some(0), None), true);
    }
    if let Some(root) = t.root("kill", true) {
        t.argument(root, "targets", entities(false), true);
    }
    if let Some(root) = t.root("time", false) {
        let set = t.literal(root, "set", false);
        t.argument(set, "time", Parser::Time { min: 0 }, true);
        t.suggested(
            set,
            "timemarker",
            Parser::ResourceLocation,
            true,
            "ask_server",
        );
        let add = t.literal(root, "add", false);
        t.argument(add, "time", Parser::Time { min: i32::MIN }, true);
        let query = t.literal(root, "query", false);
        t.literals(query, &["time", "daytime", "day", "gametime"]);
    }
    if let Some(root) = t.root("weather", false) {
        for name in ["clear", "rain", "thunder"] {
            let kind = t.literal(root, name, true);
            t.argument(kind, "duration", Parser::Time { min: 1 }, true);
        }
    }
    if let Some(root) = t.root("difficulty", true) {
        t.literals(root, &["peaceful", "easy", "normal", "hard"]);
    }
    if let Some(root) = t.root("gamerule", false) {
        for rule in rules() {
            let parser = match rule.kind {
                RuleKind::Boolean => Parser::Bool,
                RuleKind::Integer { min, max } => integer(Some(min), Some(max)),
            };
            let short = t.literal(root, rule.name, true);
            t.argument(short, "value", parser.clone(), true);
            let qualified = t.literal(root, &format!("minecraft:{}", rule.name), true);
            t.argument(qualified, "value", parser, true);
        }
    }
    if let Some(root) = t.root("summon", false) {
        let entity = t.suggested(
            root,
            "entity",
            Parser::Resource {
                registry: identifier("entity_type"),
            },
            true,
            "summonable_entities",
        );
        let pos = t.argument(entity, "pos", Parser::Vec3, true);
        t.argument(pos, "nbt", Parser::NbtCompoundTag, true);
    }
    if let Some(root) = t.root("worldborder", false) {
        t.literal(root, "get", true);
        let center = t.literal(root, "center", false);
        t.argument(center, "pos", Parser::Vec2, true);
        let set = t.literal(root, "set", false);
        t.argument(
            set,
            "distance",
            Parser::Double {
                min: Some(1.0_f64.to_bits()),
                max: Some(59_999_968.0_f64.to_bits()),
            },
            true,
        );
        let add = t.literal(root, "add", false);
        t.argument(
            add,
            "distance",
            Parser::Double {
                min: None,
                max: None,
            },
            true,
        );
    }
    if let Some(root) = t.root("setblock", false) {
        let pos = t.argument(root, "pos", Parser::BlockPos, false);
        let block = t.argument(pos, "block", Parser::BlockState, true);
        t.literals(block, &["replace", "destroy", "keep"]);
    }
    if let Some(root) = t.root("fill", false) {
        let from = t.argument(root, "from", Parser::BlockPos, false);
        let to = t.argument(from, "to", Parser::BlockPos, false);
        let block = t.argument(to, "block", Parser::BlockState, true);
        t.literals(block, &["destroy", "keep", "hollow", "outline"]);
        let replace = t.literal(block, "replace", true);
        t.argument(replace, "filter", Parser::BlockPredicate, true);
    }
    if let Some(root) = t.root("advancement", false) {
        for action in ["grant", "revoke"] {
            let action = t.literal(root, action, false);
            let targets = t.argument(action, "targets", players(false), false);
            for mode in ["only", "from", "until", "through"] {
                let mode_node = t.literal(targets, mode, false);
                let advancement = t.argument(
                    mode_node,
                    "advancement",
                    Parser::ResourceKey {
                        registry: identifier("advancement"),
                    },
                    true,
                );
                if mode == "only" {
                    t.suggested(
                        advancement,
                        "criterion",
                        Parser::String(CommandStringKind::GreedyPhrase),
                        true,
                        "ask_server",
                    );
                }
            }
            t.literal(targets, "everything", true);
        }
    }
    if let Some(root) = t.aliased_root("experience", false) {
        for (name, min) in [("add", None), ("set", Some(0))] {
            let operation = t.literal(root, name, false);
            let target = t.argument(operation, "target", players(false), false);
            let amount = t.argument(target, "amount", integer(min, None), true);
            t.literals(amount, &["points", "levels"]);
        }
        let query = t.literal(root, "query", false);
        let target = t.argument(query, "target", players(true), false);
        t.literals(target, &["points", "levels"]);
        t.alias("xp", root);
    }
    if let Some(root) = t.root("spawnpoint", true) {
        let targets = t.argument(root, "targets", players(false), true);
        let pos = t.argument(targets, "pos", Parser::BlockPos, true);
        t.argument(pos, "rotation", Parser::Rotation, true);
    }
    if let Some(root) = t.root("setworldspawn", true) {
        let pos = t.argument(root, "pos", Parser::BlockPos, true);
        t.argument(pos, "rotation", Parser::Rotation, true);
    }
    if let Some(root) = t.root("forceload", false) {
        let add = t.literal(root, "add", false);
        let from = t.argument(add, "from", Parser::ColumnPos, true);
        t.argument(from, "to", Parser::ColumnPos, true);
        let remove = t.literal(root, "remove", false);
        let from = t.argument(remove, "from", Parser::ColumnPos, true);
        t.argument(from, "to", Parser::ColumnPos, true);
        t.literal(remove, "all", true);
        let query = t.literal(root, "query", true);
        t.argument(query, "pos", Parser::ColumnPos, true);
    }
    if let Some(root) = t.root("say", false) {
        t.argument(root, "message", Parser::Message, true);
    }
    let _ = t.root("seed", true);
    // MetricsMgmt: argument-less server pace reports (handled in server.rs).
    let _ = t.root("tps", true);
    let _ = t.root("mspt", true);
    if let Some(root) = t.root("enchant", false) {
        let targets = t.argument(root, "targets", entities(false), false);
        let enchantment = t.argument(
            targets,
            "enchantment",
            Parser::Resource {
                registry: identifier("enchantment"),
            },
            true,
        );
        t.argument(enchantment, "level", integer(Some(0), None), true);
    }
    if let Some(root) = t.root("effect", false) {
        let clear = t.literal(root, "clear", true);
        let targets = t.argument(clear, "targets", entities(false), true);
        t.argument(
            targets,
            "effect",
            Parser::Resource {
                registry: identifier("mob_effect"),
            },
            true,
        );
        let give = t.literal(root, "give", false);
        let targets = t.argument(give, "targets", entities(false), false);
        let effect = t.argument(
            targets,
            "effect",
            Parser::Resource {
                registry: identifier("mob_effect"),
            },
            true,
        );
        let seconds = t.argument(effect, "seconds", integer(Some(1), Some(1_000_000)), true);
        let infinite = t.literal(effect, "infinite", true);
        for duration in [seconds, infinite] {
            let amplifier = t.argument(duration, "amplifier", integer(Some(0), Some(255)), true);
            t.argument(amplifier, "hideParticles", Parser::Bool, true);
        }
    }
    if let Some(root) = t.root("debug", false) {
        // Solaris's retained fixture, not vanilla DebugCommand's profiling tree.
        let give = t.literal(root, "give", false);
        let item = t.argument(give, "item", Parser::ItemStack, true);
        let count = t.argument(item, "count", integer(None, None), true);
        t.argument(count, "hotbar_slot", integer(Some(0), Some(8)), true);
    }
    if let Some(root) = t.root("team", false) {
        team_tree(&mut t, root);
    }
    if let Some(root) = t.root("scoreboard", false) {
        scoreboard_tree(&mut t, root);
    }
    if let Some(root) = t.root("kick", false) {
        let targets = t.argument(root, "targets", players(false), true);
        t.argument(targets, "reason", Parser::Message, true);
    }
    if let Some(root) = t.root("transfer", false) {
        // Vanilla TransferCommand: /transfer <hostname> [port] [players].
        let hostname = t.argument(
            root,
            "hostname",
            Parser::String(CommandStringKind::QuotablePhrase),
            true,
        );
        let port = t.argument(hostname, "port", integer(Some(1), Some(65_535)), true);
        t.argument(port, "players", players(false), true);
    }
    for name in ["ban", "pardon", "op", "deop", "mute", "unmute", "warn"] {
        if let Some(root) = t.root(name, false) {
            let targets = match name {
                "ban" | "mute" | "warn" => t.argument(root, "targets", Parser::GameProfile, true),
                _ => t.suggested(root, "targets", Parser::GameProfile, true, "ask_server"),
            };
            match name {
                // Solaris extends vanilla `/ban` with an optional duration, so
                // the reason starts one argument later when one is present.
                "ban" | "mute" => {
                    let duration = t.argument(
                        targets,
                        "duration",
                        Parser::String(CommandStringKind::SingleWord),
                        true,
                    );
                    t.argument(duration, "reason", Parser::Message, true);
                }
                "warn" => {
                    t.argument(targets, "reason", Parser::Message, true);
                }
                _ => {}
            }
        }
    }
    for name in ["ban-ip", "pardon-ip"] {
        if let Some(root) = t.root(name, false) {
            let target = if name == "pardon-ip" {
                t.suggested(
                    root,
                    "target",
                    Parser::String(CommandStringKind::SingleWord),
                    true,
                    "ask_server",
                )
            } else {
                t.argument(
                    root,
                    "target",
                    Parser::String(CommandStringKind::SingleWord),
                    true,
                )
            };
            if name == "ban-ip" {
                t.argument(target, "reason", Parser::Message, true);
            }
        }
    }
    if let Some(root) = t.root("whitelist", false) {
        // WhitelistCommand.java: the on/off/list/reload literals run without
        // arguments, add/remove take a resolved game profile.
        t.literals(root, &["on", "off", "list", "reload"]);
        for name in ["add", "remove"] {
            let literal = t.literal(root, name, false);
            t.argument(literal, "targets", Parser::GameProfile, true);
        }
    }
    if let Some(root) = t.root("banlist", false) {
        // BanListCommands.java: `/banlist players|ips`, never bare.
        t.literals(root, &["players", "ips"]);
    }
    if let Some(root) = t.root("co", true) {
        // Solaris block log (CoreProtect-style `u:` `t:` `r:` parameters).
        t.literals(root, &["inspect", "i", "help"]);
        for name in ["lookup", "l", "rollback", "rb", "restore", "rs"] {
            let literal = t.literal(root, name, true);
            t.argument(
                literal,
                "parameters",
                Parser::String(CommandStringKind::GreedyPhrase),
                true,
            );
        }
    }
    locate_tree(&mut t);
    tick_tree(&mut t);
    tellraw_title_bossbar_trees(&mut t);
    let _ = t.root("stop", true);
    if let Some(root) = t.root("save-all", true) {
        t.literal(root, "flush", true);
    }
    for (name, level) in plugins {
        if op_level >= *level {
            let root = t.literal(0, name, true);
            t.nodes[root as usize].restricted = *level > 0;
            t.argument(
                root,
                "arguments",
                Parser::String(CommandStringKind::GreedyPhrase),
                true,
            );
        }
    }
    ClientboundCommands {
        nodes: t.nodes,
        root_index: 0,
    }
}

fn settlement_tree(t: &mut Tree, root: i32) {
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

fn team_tree(t: &mut Tree, root: i32) {
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

fn scoreboard_tree(t: &mut Tree, root: i32) {
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
fn locate_tree(t: &mut Tree) {
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
fn tick_tree(t: &mut Tree) {
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
fn tellraw_title_bossbar_trees(t: &mut Tree) {
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

/// Cheap completion for roots, player selectors/names and registry arguments.
/// Brigadier string ranges are UTF-16 code-unit offsets, including any slash.
#[allow(clippy::too_many_arguments)]
pub(crate) fn suggestions(
    request_id: i32,
    command: &str,
    op_level: u8,
    player_names: &[String],
    item_ids: &[String],
    block_ids: &[String],
    entity_ids: &[String],
    structure_ids: &[String],
    plugins: &[(String, u8)],
) -> ClientboundCommandSuggestions {
    let start = command
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(usize::from(command.starts_with('/')), |(index, c)| {
            index + c.len_utf8()
        });
    let prefix = &command[start..];
    let preceding = command[..start].trim_start_matches('/');
    let mut words = preceding.split_whitespace();
    let root = words.next();
    let second = words.next();
    let argument_index = preceding.split_whitespace().count();
    let mut result = ClientboundCommandSuggestions {
        id: request_id,
        start: command[..start].encode_utf16().count() as i32,
        length: prefix.encode_utf16().count() as i32,
        suggestions: Vec::new(),
    };
    let mut add = |candidate: &str| {
        if result.suggestions.len() < 256
            && (candidate.starts_with(prefix)
                || (!prefix.contains(':')
                    && candidate
                        .strip_prefix("minecraft:")
                        .is_some_and(|path| path.starts_with(prefix))))
        {
            result.suggestions.push(CommandSuggestionEntry {
                text: candidate.to_owned(),
                tooltip_nbt: None,
            });
        }
    };
    let Some(root) = root else {
        for &(name, level) in ROOTS {
            if op_level >= level && !plugins.iter().any(|(root, _)| root == name) {
                add(name);
            }
        }
        for (name, level) in plugins {
            if op_level >= *level {
                add(name);
            }
        }
        return result;
    };
    if plugins.iter().any(|(name, _)| name == root) {
        return result;
    }
    if !ROOTS
        .iter()
        .any(|&(name, level)| name == root && op_level >= level)
    {
        return result;
    }
    if root == "settlement" {
        if argument_index == 1 {
            for &(action, _) in SETTLEMENT_HELP {
                add(action);
            }
        } else if argument_index == 3
            && let Some(domain @ ("production" | "commerce" | "payroll" | "caravan")) = second
        {
            add("status");
            for &(_, action, _) in SETTLEMENT_ECONOMY.iter().filter(|entry| entry.0 == domain) {
                add(action);
            }
        }
        return result;
    }
    if root == "effect" {
        match (second, argument_index) {
            (_, 1) => {
                add("give");
                add("clear");
                return result;
            }
            (Some("give" | "clear"), 3) => {
                for id in mc_core::effects::EffectId::ALL {
                    add(mc_data::mob_effects_26_1_2::name_for_id(id as u32)
                        .expect("built-in mob-effect identifier"));
                }
                return result;
            }
            (Some("give"), 4) => {
                add("infinite");
                return result;
            }
            (Some("give"), 6) => {
                add("false");
                add("true");
                return result;
            }
            _ => {}
        }
    }
    if root == "tick" {
        match (second, argument_index) {
            (_, 1) => {
                for literal in ["query", "rate", "step", "sprint", "unfreeze", "freeze"] {
                    add(literal);
                }
                return result;
            }
            (Some("rate"), 2) => {
                add("20");
                return result;
            }
            (Some("step"), 2) => {
                // Vanilla merges the `stop` literal with the time argument's
                // `1t`/`1s` suggestions (TickCommand.java:35-44); `add`
                // filters by the typed prefix.
                add("stop");
                add("1t");
                add("1s");
                return result;
            }
            (Some("sprint"), 2) => {
                // TickCommand.java:45-53: `stop` plus `60s`/`1d`/`3d`.
                add("stop");
                add("60s");
                add("1d");
                add("3d");
                return result;
            }
            _ => return result,
        }
    }
    if root == "bossbar" {
        match (second, argument_index) {
            (_, 1) => {
                for literal in ["add", "remove", "list", "set", "get"] {
                    add(literal);
                }
                return result;
            }
            (Some("set"), 3) => {
                for literal in [
                    "name", "color", "style", "value", "max", "visible", "players",
                ] {
                    add(literal);
                }
                return result;
            }
            (Some("set"), 4) => {
                // Words are root,set,id,field: the field is the fourth word.
                let field = preceding.split_whitespace().nth(3);
                match field {
                    Some("color") => {
                        for color in ["pink", "blue", "red", "green", "yellow", "purple", "white"] {
                            add(color);
                        }
                        return result;
                    }
                    Some("style") => {
                        for style in [
                            "progress",
                            "notched_6",
                            "notched_10",
                            "notched_12",
                            "notched_20",
                        ] {
                            add(style);
                        }
                        return result;
                    }
                    Some("visible") => {
                        add("false");
                        add("true");
                        return result;
                    }
                    _ => {}
                }
            }
            (Some("get"), 3) => {
                for literal in ["value", "max", "visible", "players"] {
                    add(literal);
                }
                return result;
            }
            _ => {}
        }
    }
    if root == "locate" {
        match (second, argument_index) {
            (_, 1) => {
                for literal in ["structure", "biome", "poi"] {
                    add(literal);
                }
                return result;
            }
            (Some("structure"), 2) => {
                for id in structure_ids {
                    add(id);
                }
                return result;
            }
            _ => return result,
        }
    }
    if root == "title" {
        match (second, argument_index) {
            (_, 1) => {
                for selector in ["@a", "@p", "@r", "@s"] {
                    add(selector);
                }
                for name in player_names {
                    add(name);
                }
                return result;
            }
            (_, 2) => {
                for literal in ["clear", "reset", "title", "subtitle", "actionbar", "times"] {
                    add(literal);
                }
                return result;
            }
            _ => return result,
        }
    }
    if root == "tellraw" {
        if argument_index == 1 {
            for selector in ["@a", "@p", "@r", "@s"] {
                add(selector);
            }
            for name in player_names {
                add(name);
            }
        }
        return result;
    }
    let registry = match (root, second, argument_index) {
        ("give" | "clear", _, 2) | ("debug", Some("give"), 2) => Some(item_ids),
        ("summon", _, 1) => Some(entity_ids),
        ("setblock", _, 4) | ("fill", _, 7) | ("fill", _, 9) => Some(block_ids),
        _ => None,
    };
    if let Some(ids) = registry {
        for id in ids {
            add(id);
        }
    } else if (matches!(
        root,
        "msg"
            | "tell"
            | "w"
            | "give"
            | "clear"
            | "kill"
            | "kick"
            | "ban"
            | "mute"
            | "unmute"
            | "warn"
            | "pardon"
            | "op"
            | "deop"
            | "spawnpoint"
            | "enchant"
    ) && argument_index == 1)
        || (matches!(root, "tp" | "teleport") && (1..=2).contains(&argument_index))
        || (root == "gamemode" && argument_index == 2)
        || (matches!(root, "xp" | "experience") && argument_index == 2)
        || (root == "advancement" && argument_index == 2)
        || (root == "team" && second == Some("leave") && argument_index == 2)
        || (root == "team" && second == Some("join") && argument_index == 3)
        || (root == "scoreboard" && second == Some("players") && argument_index == 3)
        || (root == "effect" && matches!(second, Some("give" | "clear")) && argument_index == 2)
    {
        for selector in ["@a", "@p", "@r", "@s"] {
            add(selector);
        }
        if matches!(
            root,
            "kill" | "tp" | "teleport" | "enchant" | "team" | "scoreboard" | "effect"
        ) {
            add("@e");
            add("@n");
        }
        for name in player_names {
            add(name);
        }
    } else if root == "time" && second == Some("set") && argument_index == 2 {
        for marker in [
            "minecraft:day",
            "minecraft:noon",
            "minecraft:night",
            "minecraft:midnight",
        ] {
            add(marker);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_protocol::Packet;

    #[test]
    fn advancement_tree_covers_actions_scopes_and_greedy_criterion() {
        let packet = tree(2, &[]);
        assert!(!permitted("advancement grant @s everything", 1, &[]));
        for action in ["grant", "revoke"] {
            assert_eq!(
                parser(&packet, &["advancement", action, "targets"]),
                players(false)
            );
            assert!(node(&packet, &["advancement", action, "targets", "everything"]).executable);
            for mode in ["only", "from", "until", "through"] {
                let path = ["advancement", action, "targets", mode, "advancement"];
                assert!(node(&packet, &path).executable);
                assert_eq!(
                    parser(&packet, &path),
                    Parser::ResourceKey {
                        registry: identifier("advancement")
                    },
                );
            }
            assert_eq!(
                parser(
                    &packet,
                    &[
                        "advancement",
                        action,
                        "targets",
                        "only",
                        "advancement",
                        "criterion"
                    ]
                ),
                Parser::String(CommandStringKind::GreedyPhrase),
            );
        }
    }

    #[test]
    fn forceload_tree_matches_vanilla_column_syntax_and_is_operator_gated() {
        let packet = tree(2, &[]);
        for path in [
            &["forceload", "add", "from"][..],
            &["forceload", "add", "from", "to"][..],
            &["forceload", "remove", "from"][..],
            &["forceload", "remove", "from", "to"][..],
            &["forceload", "remove", "all"][..],
            &["forceload", "query"][..],
            &["forceload", "query", "pos"][..],
        ] {
            assert!(node(&packet, path).executable, "{path:?}");
        }
        assert!(!node(&packet, &["forceload"]).executable);
        assert!(!node(&packet, &["forceload", "add"]).executable);
        for path in [
            &["forceload", "add", "from"][..],
            &["forceload", "add", "from", "to"][..],
            &["forceload", "remove", "from", "to"][..],
            &["forceload", "query", "pos"][..],
        ] {
            assert_eq!(parser(&packet, path), Parser::ColumnPos, "{path:?}");
        }
        let roots = |level| {
            let packet = tree(level, &[]);
            packet.nodes[0].children.iter().any(|child| {
                matches!(&packet.nodes[*child as usize].kind,
                        CommandNodeKind::Literal(name) if name == "forceload")
            })
        };
        assert!(!roots(1) && roots(2));
    }

    #[test]
    fn effect_tree_matches_vanilla_optional_arguments_and_resource_registry() {
        let packet = tree(2, &[]);
        assert!(node(&packet, &["effect", "clear"]).executable);
        assert!(node(&packet, &["effect", "clear", "targets"]).executable);
        for operation in ["give", "clear"] {
            assert_eq!(
                parser(&packet, &["effect", operation, "targets"]),
                entities(false)
            );
            assert_eq!(
                parser(&packet, &["effect", operation, "targets", "effect"]),
                Parser::Resource {
                    registry: identifier("mob_effect")
                }
            );
            assert!(node(&packet, &["effect", operation, "targets", "effect"]).executable);
        }
        assert_eq!(
            parser(&packet, &["effect", "give", "targets", "effect", "seconds"]),
            integer(Some(1), Some(1_000_000))
        );
        for duration in ["seconds", "infinite"] {
            assert!(node(&packet, &["effect", "give", "targets", "effect", duration]).executable);
            assert_eq!(
                parser(
                    &packet,
                    &["effect", "give", "targets", "effect", duration, "amplifier"]
                ),
                integer(Some(0), Some(255))
            );
            assert_eq!(
                parser(
                    &packet,
                    &[
                        "effect",
                        "give",
                        "targets",
                        "effect",
                        duration,
                        "amplifier",
                        "hideParticles"
                    ]
                ),
                Parser::Bool
            );
        }
    }

    #[test]
    fn effect_suggestions_cover_actions_targets_ids_infinite_and_visibility() {
        let names = ["Alice".to_owned()];
        let texts = |command: &str, level| {
            suggestions(1, command, level, &names, &[], &[], &[], &[], &[])
                .suggestions
                .into_iter()
                .map(|entry| entry.text)
                .collect::<Vec<_>>()
        };
        assert_eq!(texts("effect ", 2), ["give", "clear"]);
        assert!(texts("effect ", 1).is_empty());
        assert!(texts("effect give @", 2).contains(&"@e".to_owned()));
        assert_eq!(texts("effect clear A", 2), ["Alice"]);
        for operation in ["give", "clear"] {
            assert_eq!(
                texts(&format!("effect {operation} @s dar"), 2),
                ["minecraft:darkness"]
            );
        }
        assert_eq!(texts("effect give @s speed inf", 2), ["infinite"]);
        assert_eq!(
            texts("effect give @s speed infinite 255 ", 2),
            ["false", "true"]
        );
    }

    fn node<'a>(packet: &'a ClientboundCommands, path: &[&str]) -> &'a CommandNode {
        let mut current = &packet.nodes[packet.root_index as usize];
        for name in path {
            if let Some(redirect) = current.redirect {
                current = &packet.nodes[redirect as usize];
            }
            current = current
                .children
                .iter()
                .map(|index| &packet.nodes[*index as usize])
                .find(|node| match &node.kind {
                    CommandNodeKind::Literal(literal) => literal == name,
                    CommandNodeKind::Argument { name: argument, .. } => argument == name,
                    CommandNodeKind::Root => false,
                })
                .unwrap_or_else(|| panic!("advertised command path {path:?} missing {name:?}"));
        }
        current
    }

    fn parser(packet: &ClientboundCommands, path: &[&str]) -> Parser {
        match &node(packet, path).kind {
            CommandNodeKind::Argument { parser, .. } => parser.clone(),
            _ => panic!("expected argument"),
        }
    }

    #[test]
    fn decoded_trees_round_trip_and_every_node_is_reachable() {
        for level in 0..=4 {
            let packet = tree(level, &[]);
            let mut bytes = Vec::new();
            packet.encode(&mut bytes).unwrap();
            let mut input = bytes.as_slice();
            let decoded = ClientboundCommands::decode(&mut input).unwrap();
            assert_eq!(decoded, packet);
            assert!(input.is_empty());
            let mut visited = vec![false; decoded.nodes.len()];
            let mut pending = vec![decoded.root_index];
            while let Some(index) = pending.pop() {
                assert!(index >= 0 && (index as usize) < decoded.nodes.len());
                if std::mem::replace(&mut visited[index as usize], true) {
                    continue;
                }
                let node = &decoded.nodes[index as usize];
                pending.extend_from_slice(&node.children);
                pending.extend(node.redirect);
                if node.children.is_empty() && node.redirect.is_none() {
                    assert!(node.executable, "unusable leaf: {:?}", node.kind);
                }
            }
            assert!(visited.into_iter().all(|reachable| reachable));
        }
    }

    #[test]
    fn nonoperators_only_receive_public_roots_and_thresholds_match_vanilla() {
        for level in 0..=4 {
            let packet = tree(level, &[]);
            let roots: Vec<&str> = packet.nodes[0]
                .children
                .iter()
                .map(|index| match &packet.nodes[*index as usize].kind {
                    CommandNodeKind::Literal(name) => name.as_str(),
                    _ => panic!("root child must be a literal"),
                })
                .collect();
            let expected: Vec<&str> = ROOTS
                .iter()
                .filter(|(_, required)| level >= *required)
                .map(|(name, _)| *name)
                .collect();
            assert_eq!(roots.len(), expected.len());
            for root in expected {
                assert!(roots.contains(&root));
            }
            if level < 2 {
                assert_eq!(
                    roots,
                    ["help", "list", "me", "msg", "tell", "w", "teammsg", "tm", "settlement"]
                );
            }
            for &index in &packet.nodes[0].children {
                let root = &packet.nodes[index as usize];
                assert_eq!(
                    root.restricted,
                    level >= 2
                        && !matches!(&root.kind, CommandNodeKind::Literal(name) if ["help", "list", "me", "msg", "tell", "w", "teammsg", "tm", "settlement"].contains(&name.as_str()))
                );
            }
        }
    }

    #[test]
    fn aliases_redirect_to_canonical_nodes_without_duplicate_subtrees() {
        let packet = tree(4, &[]);
        for (alias, canonical) in [
            ("tp", "teleport"),
            ("xp", "experience"),
            ("tell", "msg"),
            ("w", "msg"),
        ] {
            let alias = node(&packet, &[alias]);
            let canonical = node(&packet, &[canonical]);
            assert!(alias.children.is_empty());
            assert!(!alias.executable);
            assert_eq!(&packet.nodes[alias.redirect.unwrap() as usize], canonical);
        }
        assert!(node(&packet, &["tp", "location"]).executable);
        assert!(node(&packet, &["tp", "location", "rotation"]).executable);
        assert!(node(&packet, &["tp", "targets", "location", "rotation"]).executable);
        assert!(node(&packet, &["w", "targets", "message"]).executable);
    }

    #[test]
    fn parser_properties_and_executable_optional_arguments_are_exact() {
        let packet = tree(4, &[]);
        for (path, expected) in [
            (&["teleport", "location", "rotation"][..], Parser::Rotation),
            (
                &["teleport", "targets", "location", "rotation"][..],
                Parser::Rotation,
            ),
            (&["give", "targets"][..], players(false)),
            (&["give", "targets", "item"][..], Parser::ItemStack),
            (
                &["give", "targets", "item", "count"][..],
                integer(Some(1), None),
            ),
            (&["clear", "targets", "item"][..], Parser::ItemPredicate),
            (
                &["clear", "targets", "item", "maxCount"][..],
                integer(Some(0), None),
            ),
            (&["gamemode", "gamemode"][..], Parser::GameMode),
            (&["teleport", "destination"][..], entities(true)),
            (&["teleport", "targets"][..], entities(false)),
            (&["teleport", "location"][..], Parser::Vec3),
            (&["time", "set", "time"][..], Parser::Time { min: 0 }),
            (&["time", "add", "time"][..], Parser::Time { min: i32::MIN }),
            (
                &["weather", "rain", "duration"][..],
                Parser::Time { min: 1 },
            ),
            (
                &["spawnpoint", "targets", "pos", "rotation"][..],
                Parser::Rotation,
            ),
            (
                &["enchant", "targets", "enchantment", "level"][..],
                integer(Some(0), None),
            ),
            (&["experience", "query", "target"][..], players(true)),
            (
                &["scoreboard", "players", "get", "target"][..],
                Parser::ScoreHolder { multiple: false },
            ),
            (
                &[
                    "scoreboard",
                    "players",
                    "add",
                    "targets",
                    "objective",
                    "score",
                ][..],
                integer(Some(0), None),
            ),
        ] {
            assert_eq!(parser(&packet, path), expected, "{path:?}");
        }
        assert_eq!(
            parser(&packet, &["summon", "entity"]),
            Parser::Resource {
                registry: identifier("entity_type")
            }
        );
        assert_eq!(
            parser(&packet, &["enchant", "targets", "enchantment"]),
            Parser::Resource {
                registry: identifier("enchantment")
            }
        );
        assert_eq!(
            integer(Some(i32::MIN), Some(i32::MAX)),
            Parser::Integer {
                min: None,
                max: None
            }
        );
        for path in [
            &["give", "targets", "item"][..],
            &["clear"][..],
            &["difficulty"][..],
            &["summon", "entity"][..],
            &["spawnpoint"][..],
            &["scoreboard", "objectives", "setdisplay", "slot"][..],
        ] {
            assert!(node(&packet, path).executable);
        }
        for rule in rules() {
            let expected = match rule.kind {
                RuleKind::Boolean => Parser::Bool,
                RuleKind::Integer { min, max } => integer(Some(min), Some(max)),
            };
            assert_eq!(parser(&packet, &["gamerule", rule.name, "value"]), expected);
            assert_eq!(
                parser(
                    &packet,
                    &["gamerule", &format!("minecraft:{}", rule.name), "value"]
                ),
                expected
            );
        }
        for (path, expected) in [
            (&["summon", "entity"][..], "summonable_entities"),
            (&["time", "set", "timemarker"][..], "ask_server"),
            (&["team", "leave", "members"][..], "ask_server"),
        ] {
            match &node(&packet, path).kind {
                CommandNodeKind::Argument { suggestions, .. } => {
                    assert_eq!(*suggestions, Some(identifier(expected)))
                }
                _ => panic!("expected suggested argument"),
            }
        }
    }

    #[test]
    fn permission_helper_matches_advertisement() {
        for level in 0..=4 {
            for &(root, required) in ROOTS {
                assert_eq!(permitted(root, level, &[]), level >= required);
                assert_eq!(
                    permitted(&format!("/{root} arguments"), level, &[]),
                    level >= required
                );
            }
        }
        assert!(permitted("effect", 2, &[]));
        assert!(!permitted("effect", 1, &[]));
        assert!(!permitted("", 4, &[]));
    }

    #[test]
    fn suggestions_filter_permissions_registries_and_use_utf16_ranges() {
        let names = vec!["Alice".to_owned(), "😀Bob".to_owned()];
        let items = vec!["minecraft:stone".to_owned(), "minecraft:stick".to_owned()];
        let response = suggestions(7, "/s", 0, &names, &items, &[], &[], &[], &[]);
        assert_eq!((response.id, response.start, response.length), (7, 1, 1));
        assert_eq!(
            response
                .suggestions
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["settlement"]
        );
        let response = suggestions(8, "/give @s st", 2, &names, &items, &[], &[], &[], &[]);
        assert_eq!((response.start, response.length), (9, 2));
        assert_eq!(response.suggestions.len(), 2);
        let response = suggestions(9, "/msg 😀B", 0, &names, &items, &[], &[], &[], &[]);
        assert_eq!((response.start, response.length), (5, 3));
        assert_eq!(response.suggestions[0].text, "😀Bob");
        assert!(
            suggestions(10, "give @s ", 0, &names, &items, &[], &[], &[], &[])
                .suggestions
                .is_empty()
        );
        let response = suggestions(11, "msg @", 0, &names, &items, &[], &[], &[], &[]);
        assert!(
            !response
                .suggestions
                .iter()
                .any(|entry| entry.text == "@e" || entry.text == "@n")
        );
        let response = suggestions(12, "kill @", 2, &names, &items, &[], &[], &[], &[]);
        assert!(response.suggestions.iter().any(|entry| entry.text == "@e"));
    }

    #[test]
    fn help_derives_smart_optional_and_alternative_usages_from_the_tree() {
        let public = help(None, 0, &[]).unwrap();
        assert_eq!(public.len(), 9);
        assert!(public.contains(&"/help [<command>]".to_owned()));
        assert!(public.contains(&"/list [uuids]".to_owned()));
        assert!(public.contains(&"/msg <targets> <message>".to_owned()));
        assert!(public.contains(&"/tell -> msg".to_owned()));
        assert!(public.contains(&"/teammsg <message>".to_owned()));
        assert!(public.contains(&"/tm -> teammsg".to_owned()));
        assert!(public.iter().all(|line| !line.contains("gamemode")));
        assert_eq!(
            help(Some("give"), 2, &[]).unwrap(),
            ["/give <targets> <item> [<count>]"]
        );
        assert_eq!(
            help(Some("give <targets>"), 2, &[]).unwrap(),
            ["/give <targets> <item> [<count>]"]
        );
        assert_eq!(
            help(Some("time"), 2, &[]).unwrap(),
            [
                "/time set (<time>|<timemarker>)",
                "/time add <time>",
                "/time query (time|daytime|day|gametime)",
            ]
        );
        assert_eq!(
            help(Some("give targets item"), 2, &[]).unwrap(),
            ["/give targets item [<count>]"]
        );
        assert!(
            help(Some("give targets item count"), 2, &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn help_resolves_aliases_and_rejects_hidden_or_unknown_roots() {
        assert_eq!(help(Some("w"), 0, &[]).unwrap(), ["/w <targets> <message>"]);
        assert_eq!(
            help(Some("xp query"), 2, &[]).unwrap(),
            ["/xp query <target> (points|levels)"]
        );
        assert_eq!(
            help(Some("/scoreboard players get"), 2, &[]).unwrap(),
            ["/scoreboard players get <target> <objective>"]
        );
        for path in ["give", "giveaway", "op", "effect", ""] {
            assert!(help(Some(path), 0, &[]).is_err(), "{path}");
        }
        let partial = help(Some("scoreboard players missing"), 2, &[]).unwrap();
        assert!(!partial.is_empty());
        assert!(
            partial
                .iter()
                .all(|usage| usage.starts_with("/scoreboard players missing "))
        );
        assert_eq!(
            help(Some("give <missing>"), 2, &[]).unwrap(),
            ["/give <missing> <targets> <item> [<count>]"]
        );
        assert!(help(Some("stop"), 3, &[]).is_err());
        assert!(help(Some("stop"), 4, &[]).unwrap().is_empty());
    }

    #[test]
    fn help_traverses_actual_values_and_prefers_the_longest_argument_branch() {
        assert_eq!(
            help(Some("gamemode creative"), 2, &[]).unwrap(),
            ["/gamemode creative [<target>]"]
        );
        assert_eq!(
            help(Some("give Bob"), 2, &[]).unwrap(),
            ["/give Bob <item> [<count>]"]
        );
        assert_eq!(
            help(Some("give Bob minecraft:stone"), 2, &[]).unwrap(),
            ["/give Bob minecraft:stone [<count>]"]
        );
        assert_eq!(
            help(Some("teleport Bob 0 80 0"), 2, &[]).unwrap(),
            ["/teleport Bob 0 80 0 [<rotation>]"]
        );
        assert_eq!(
            help(Some("tp Bob ~ ~1 ~"), 2, &[]).unwrap(),
            ["/tp Bob ~ ~1 ~ [<rotation>]"]
        );
        assert_eq!(
            help(Some("teleport 0 80 0"), 2, &[]).unwrap(),
            ["/teleport 0 80 0 [<rotation>]"]
        );
        assert!(
            help(Some("teleport Bob 0 80 0 90 0"), 2, &[])
                .unwrap()
                .is_empty()
        );
        assert!(help(Some("tp 0 80 0 ~90 ~"), 2, &[]).unwrap().is_empty());
        assert_eq!(
            help(Some("scoreboard players set Bob"), 2, &[]).unwrap(),
            ["/scoreboard players set Bob <objective> <score>"]
        );
        assert_eq!(
            help(Some("weather rain"), 2, &[]).unwrap(),
            ["/weather rain [<duration>]"]
        );
        assert!(help(Some("weather rain 10s"), 2, &[]).unwrap().is_empty());
        assert_eq!(
            help(Some("spawnpoint Bob 0 80 0"), 2, &[]).unwrap(),
            ["/spawnpoint Bob 0 80 0 [<rotation>]"]
        );
        assert!(
            help(Some("spawnpoint Bob 0 80 0 90 0"), 2, &[])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            help(
                Some("give Bob minecraft:stone[custom_name='hello world']"),
                2,
                &[]
            )
            .unwrap(),
            ["/give Bob minecraft:stone[custom_name='hello world'] [<count>]"]
        );
        assert_eq!(
            help(Some("give Bob stone nope"), 2, &[]).unwrap(),
            ["/give Bob stone nope [<count>]"]
        );
        assert_eq!(
            help_argument_length(&Parser::Vec3, "0 80 0 remaining"),
            Some(6)
        );
        assert_eq!(help_argument_length(&Parser::Vec3, "0 80"), None);
        assert_eq!(help_argument_length(&integer(Some(1), None), "0"), None);
        assert_eq!(
            help_argument_length(&Parser::Time { min: 1 }, "2s"),
            Some(2)
        );
    }
}

#[cfg(test)]
#[path = "command_tree_parity_tests.rs"]
mod parity;
