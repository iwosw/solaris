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
    ("say", 2),
    ("seed", 2),
    ("enchant", 2),
    ("effect", 2),
    ("advancement", 2),
    ("debug", 2),
    ("team", 2),
    ("scoreboard", 2),
    ("kick", 3),
    ("ban", 3),
    ("ban-ip", 3),
    ("pardon", 3),
    ("pardon-ip", 3),
    ("op", 3),
    ("deop", 3),
    ("stop", 4),
    ("save-all", 4),
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
    let mut usages = Vec::with_capacity(parent.children.len());
    for &child in &parent.children {
        let mut line = String::from("/");
        if let Some(path) = path {
            line.push_str(path);
            line.push(' ');
        }
        smart_usage(&packet, child, parent.executable, false, &mut line);
        usages.push(line);
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
            16
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
        if let Some(execute) = t.root("execute", false) {
            let in_dimension = t.literal(execute, "in", false);
            // ExecuteCommand.java:258-261 / ArgumentTypeInfos.java:133:
            // minecraft:dimension is singleton argument registry id 41.
            let dimension = t.argument(in_dimension, "dimension", Parser::Dimension, false);
            let run = t.literal(dimension, "run", false);
            for name in ["tp", "teleport"] {
                let teleport = t.literal(run, name, false);
                t.nodes[teleport as usize].redirect = Some(root);
            }
        }
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
    if let Some(root) = t.root("say", false) {
        t.argument(root, "message", Parser::Message, true);
    }
    let _ = t.root("seed", true);
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
    for name in ["ban", "pardon", "op", "deop"] {
        if let Some(root) = t.root(name, false) {
            let targets = if name == "ban" {
                t.argument(root, "targets", Parser::GameProfile, true)
            } else {
                t.suggested(root, "targets", Parser::GameProfile, true, "ask_server")
            };
            if name == "ban" {
                t.argument(targets, "reason", Parser::Message, true);
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
            suggestions(1, command, level, &names, &[], &[], &[], &[])
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
                .expect("advertised command path");
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
                assert_eq!(roots, ["help", "list", "me", "msg", "tell", "w"]);
            }
            for &index in &packet.nodes[0].children {
                let root = &packet.nodes[index as usize];
                assert_eq!(
                    root.restricted,
                    level >= 2
                        && !matches!(&root.kind, CommandNodeKind::Literal(name) if ["help", "list", "me", "msg", "tell", "w"].contains(&name.as_str()))
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
        let response = suggestions(7, "/s", 0, &names, &items, &[], &[], &[]);
        assert_eq!((response.id, response.start, response.length), (7, 1, 1));
        assert!(response.suggestions.is_empty());
        let response = suggestions(8, "/give @s st", 2, &names, &items, &[], &[], &[]);
        assert_eq!((response.start, response.length), (9, 2));
        assert_eq!(response.suggestions.len(), 2);
        let response = suggestions(9, "/msg 😀B", 0, &names, &items, &[], &[], &[]);
        assert_eq!((response.start, response.length), (5, 3));
        assert_eq!(response.suggestions[0].text, "😀Bob");
        assert!(
            suggestions(10, "give @s ", 0, &names, &items, &[], &[], &[])
                .suggestions
                .is_empty()
        );
        let response = suggestions(11, "msg @", 0, &names, &items, &[], &[], &[]);
        assert!(
            !response
                .suggestions
                .iter()
                .any(|entry| entry.text == "@e" || entry.text == "@n")
        );
        let response = suggestions(12, "kill @", 2, &names, &items, &[], &[], &[]);
        assert!(response.suggestions.iter().any(|entry| entry.text == "@e"));
    }

    #[test]
    fn help_derives_smart_optional_and_alternative_usages_from_the_tree() {
        let public = help(None, 0, &[]).unwrap();
        assert_eq!(public.len(), 6);
        assert!(public.contains(&"/help [<command>]".to_owned()));
        assert!(public.contains(&"/list [uuids]".to_owned()));
        assert!(public.contains(&"/msg <targets> <message>".to_owned()));
        assert!(public.contains(&"/tell -> msg".to_owned()));
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
