//! Directory datapacks. Compilation is startup/worker work, never tick filesystem I/O.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::boundary::{ChangeSet, ConnectionId, EntityId, PlayerChange};
use crate::tick::TickState;

const COMMAND_BUDGET: usize = crate::command_context::LIMIT;
const MAX_RESOURCES: usize = 16384;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Default)]
pub(crate) struct PackSet {
    pub names: Vec<String>,
    functions: BTreeMap<String, Arc<[String]>>,
    tags: BTreeMap<String, Arc<[String]>>,
    resources: BTreeMap<String, String>,
}

fn identifier(value: &str) -> Result<String, String> {
    let value = if value.contains(':') {
        value.to_owned()
    } else {
        format!("minecraft:{value}")
    };
    mc_data::Identifier::parse(value)
        .map(|id| id.to_string())
        .map_err(|e| e.to_string())
}

fn files(
    root: &Path,
    out: &mut Vec<PathBuf>,
    depth: usize,
    visited: &mut usize,
) -> Result<(), String> {
    if depth > 32 {
        return Err("Datapack directory nesting limit exceeded".into());
    }
    if !root.exists() {
        return Ok(());
    }
    if std::fs::symlink_metadata(root)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "Datapack symlinks are unsupported: {}",
            root.display()
        ));
    }
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        *visited += 1;
        if *visited > MAX_RESOURCES {
            return Err("Datapack resource limit exceeded".into());
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "Datapack symlinks are unsupported: {}",
                entry.path().display()
            ));
        }
        if kind.is_dir() {
            files(&entry.path(), out, depth + 1, visited)?;
        } else if kind.is_file() {
            out.push(entry.path());
        }
        if out.len() > MAX_RESOURCES {
            return Err("Datapack resource limit exceeded".into());
        }
    }
    out.sort();
    Ok(())
}

fn read_text(path: &Path, remaining: &mut usize) -> Result<String, String> {
    use std::io::Read;
    if !std::fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_file()
    {
        return Err(format!(
            "Datapack resource must be a regular file: {}",
            path.display()
        ));
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((*remaining + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    *remaining = remaining
        .checked_sub(text.len())
        .ok_or("Datapack byte limit exceeded")?;
    Ok(text)
}

impl PackSet {
    pub(crate) fn load(root: &Path) -> Result<Self, String> {
        let mut result = Self::default();
        if !root.exists() {
            return Ok(result);
        }
        if std::fs::symlink_metadata(root)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Datapack root cannot be a symlink".into());
        }
        let mut packs = Vec::new();
        for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
            if packs.len() >= MAX_RESOURCES {
                return Err("Datapack resource limit exceeded".into());
            }
            packs.push(entry.map_err(|e| e.to_string())?);
        }
        packs.sort_by_key(|entry| entry.file_name());
        let mut remaining = MAX_BYTES;
        let mut visited = 0;
        let mut raw_tags = BTreeMap::<String, Vec<(String, bool)>>::new();
        for pack in packs {
            if !pack.file_type().map_err(|e| e.to_string())?.is_dir() {
                return Err(format!(
                    "Only directory datapacks are supported: {}",
                    pack.path().display()
                ));
            }
            let metadata: serde_json::Value = serde_json::from_str(&read_text(
                &pack.path().join("pack.mcmeta"),
                &mut remaining,
            )?)
            .map_err(|e| e.to_string())?;
            if !metadata
                .get("pack")
                .is_some_and(serde_json::Value::is_object)
            {
                return Err("Datapack pack.mcmeta requires a pack object".into());
            }
            result
                .names
                .push(pack.file_name().to_string_lossy().into_owned());
            let data = pack.path().join("data");
            let mut paths = Vec::new();
            files(&data, &mut paths, 0, &mut visited)?;
            for path in paths {
                let relative = path
                    .strip_prefix(&data)
                    .map_err(|e| e.to_string())?
                    .to_str()
                    .ok_or("Non-UTF8 datapack path")?
                    .to_owned();
                let (namespace, tail) = relative
                    .split_once('/')
                    .ok_or("Resource must be namespaced")?;
                let (kind, resource) = tail.split_once('/').ok_or("Resource must have a type")?;
                if matches!(kind, "worldgen" | "dimension" | "dimension_type") {
                    return Err(format!("Worldgen datapacks are not supported: {relative}"));
                }
                let text = read_text(&path, &mut remaining)?;
                if kind == "function" && resource.ends_with(".mcfunction") {
                    let id = identifier(&format!(
                        "{namespace}:{}",
                        resource
                            .strip_suffix(".mcfunction")
                            .expect("function extension checked")
                    ))?;
                    let lines = text
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty() && !line.starts_with('#'))
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    if lines.iter().any(|line| {
                        line.len() > 32767 || line.starts_with('/') || line.starts_with('$')
                    }) {
                        return Err(format!(
                            "Unsupported function line in {id} (macros and leading slashes are unsupported)"
                        ));
                    }
                    result.functions.insert(id, lines.into());
                } else if kind == "tags"
                    && resource.starts_with("function/")
                    && resource.ends_with(".json")
                {
                    let id = identifier(&format!(
                        "{namespace}:{}",
                        resource
                            .strip_prefix("function/")
                            .expect("function tag directory checked")
                            .strip_suffix(".json")
                            .expect("tag extension checked")
                    ))?;
                    let tag: serde_json::Value =
                        serde_json::from_str(&text).map_err(|e| e.to_string())?;
                    let values = raw_tags.entry(id).or_default();
                    if tag.get("replace").and_then(serde_json::Value::as_bool) == Some(true) {
                        values.clear();
                    }
                    for value in tag
                        .get("values")
                        .and_then(serde_json::Value::as_array)
                        .ok_or("Function tag requires values")?
                    {
                        let name = value
                            .as_str()
                            .or_else(|| value.get("id").and_then(serde_json::Value::as_str))
                            .ok_or("Invalid function tag entry")?;
                        let required = value
                            .get("required")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true);
                        values.push((name.to_owned(), required));
                    }
                } else if matches!(kind, "recipe" | "loot_table") && resource.ends_with(".json") {
                    result.resources.insert(relative, text);
                } else {
                    return Err(format!("Unsupported datapack resource: {relative}"));
                }
            }
        }
        fn expand(
            id: &str,
            raw: &BTreeMap<String, Vec<(String, bool)>>,
            functions: &BTreeMap<String, Arc<[String]>>,
            visiting: &mut BTreeSet<String>,
            output: &mut Vec<String>,
            work: &mut usize,
        ) -> Result<(), String> {
            *work += 1;
            if visiting.len() >= 32 || *work > MAX_RESOURCES {
                return Err("Function tag expansion limit exceeded".into());
            }
            if !visiting.insert(id.to_owned()) {
                return Err(format!("Cyclic function tag: {id}"));
            }
            for (name, required) in raw
                .get(id)
                .ok_or_else(|| format!("Unknown function tag {id}"))?
            {
                let nested = name.strip_prefix('#');
                let name = identifier(nested.unwrap_or(name))?;
                if nested.is_some() {
                    if raw.contains_key(&name) {
                        expand(&name, raw, functions, visiting, output, work)?;
                    } else if *required {
                        return Err(format!("Unknown function tag {name}"));
                    }
                } else if functions.contains_key(&name) {
                    if !output.contains(&name) {
                        output.push(name);
                    }
                } else if *required {
                    return Err(format!("Unknown function {name}"));
                }
            }
            visiting.remove(id);
            Ok(())
        }
        for id in raw_tags.keys() {
            let mut output = Vec::new();
            expand(
                id,
                &raw_tags,
                &result.functions,
                &mut BTreeSet::new(),
                &mut output,
                &mut 0,
            )?;
            result.tags.insert(id.clone(), output.into());
        }
        Ok(result)
    }

    pub(crate) fn install_content(
        &self,
        registry: &mut crate::registry::CoreRegistry,
    ) -> Result<(), String> {
        for (path, text) in &self.resources {
            let (namespace, tail) = path.split_once('/').ok_or("Missing namespace")?;
            let (kind, resource) = tail.split_once('/').ok_or("Missing resource kind")?;
            let id = mc_data::Identifier::parse(format!(
                "{namespace}:{}",
                resource
                    .strip_suffix(".json")
                    .ok_or("Resource must be JSON")?
            ))
            .map_err(|e| e.to_string())?;
            if kind == "recipe" {
                let recipe = mc_data::recipes::parse_datapack_recipe(id, text)?;
                if registry.items.id_of(&recipe.result.item).is_none() {
                    return Err(format!("Unknown recipe output {}", recipe.result.item));
                }
                if let Some(existing) = registry.recipes.iter_mut().find(|old| old.id == recipe.id)
                {
                    *existing = recipe;
                } else {
                    registry.recipes.push(recipe);
                }
            } else if let Some(block) = id.path().strip_prefix("blocks/") {
                let block = mc_data::Identifier::parse(format!("{namespace}:{block}"))
                    .map_err(|e| e.to_string())?;
                registry.loot.override_datapack_block(block, text)?;
            } else {
                return Err(format!(
                    "Unsupported datapack loot domain: {id}; currently block loot tables only"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Runtime {
    packs: Arc<PackSet>,
    root: Option<PathBuf>,
    reload: Option<std::sync::mpsc::Receiver<Result<PackSet, String>>>,
    load_pending: bool,
    budget_tick: u64,
    used: usize,
    pending: Vec<Frame>,
    running: bool,
    scheduled: BTreeMap<(u64, String), ()>,
    dirty: bool,
    pub(crate) origin: Option<[f64; 3]>,
}
struct Frame {
    lines: Arc<[String]>,
    next: usize,
    context: crate::command_context::Context,
}

impl Runtime {
    #[cfg(test)]
    pub(crate) fn new(packs: PackSet, root: PathBuf) -> Self {
        Self {
            packs: Arc::new(packs),
            root: Some(root),
            load_pending: true,
            ..Self::default()
        }
    }
    pub(crate) fn install(&mut self, packs: PackSet, root: PathBuf) {
        self.packs = Arc::new(packs);
        self.root = Some(root);
        self.load_pending = true;
    }
    pub(crate) fn image(&self) -> Vec<(u64, String)> {
        self.scheduled.keys().cloned().collect()
    }
    pub(crate) fn restore(&mut self, image: Vec<(u64, String)>) -> Result<(), crate::CoreError> {
        if image.len() > MAX_RESOURCES {
            return Err(crate::CoreError::Storage(
                "Scheduled function limit exceeded".into(),
            ));
        }
        let mut scheduled = BTreeMap::new();
        for (due, name) in image {
            let raw = name.strip_prefix('#').unwrap_or(&name);
            if identifier(raw).map_err(crate::CoreError::Storage)? != raw {
                return Err(crate::CoreError::Storage(
                    "Noncanonical scheduled function identifier".into(),
                ));
            }
            if scheduled.insert((due, name), ()).is_some() {
                return Err(crate::CoreError::Storage(
                    "Duplicate scheduled function".into(),
                ));
            }
        }
        self.scheduled = scheduled;
        self.dirty = false;
        Ok(())
    }
    pub(crate) fn journal(&mut self, entries: &mut Vec<crate::journal::JournalEntry>) {
        if std::mem::take(&mut self.dirty) {
            entries.push(crate::journal::JournalEntry::ScheduledFunctions {
                value: self.image(),
            });
        }
    }
}

fn feedback(state: &mut TickState, connection: ConnectionId, message: impl Into<String>) {
    state.push_player(
        connection,
        PlayerChange::Chat {
            message: message.into(),
        },
    );
}

fn enqueue(
    state: &mut TickState,
    name: &str,
    connection: ConnectionId,
    executor: EntityId,
) -> Result<(), String> {
    let context = crate::command_context::Context::source(state, connection, executor, 2);
    let tag = name.strip_prefix('#');
    let id = identifier(tag.unwrap_or(name))?;
    let runtime = &mut state.datapacks;
    if runtime.pending.len() >= COMMAND_BUDGET {
        return Err("Function stack budget exceeded".into());
    }
    if tag.is_some() {
        let names = runtime
            .packs
            .tags
            .get(&id)
            .ok_or_else(|| format!("Unknown function tag {id}"))?;
        if runtime.pending.len() + names.len() > COMMAND_BUDGET {
            return Err("Function stack budget exceeded".into());
        }
        for name in names.iter().rev() {
            runtime.pending.push(Frame {
                lines: runtime.packs.functions[name].clone(),
                next: 0,
                context,
            });
        }
    } else {
        let lines = runtime
            .packs
            .functions
            .get(&id)
            .ok_or_else(|| format!("Unknown function {id}"))?
            .clone();
        runtime.pending.push(Frame {
            lines,
            next: 0,
            context,
        });
    }
    Ok(())
}
pub(crate) fn execute_block(state: &mut TickState, text: &str, changes: &mut ChangeSet) {
    if state.datapacks.budget_tick != state.tick {
        state.datapacks.budget_tick = state.tick;
        state.datapacks.used = 0;
    }
    if state.datapacks.used >= COMMAND_BUDGET {
        return;
    }
    state.datapacks.used += 1;
    let connection = crate::command_console::CONNECTION;
    let executor = crate::command_console::EXECUTOR;
    let text = text.trim_start_matches('/');
    let context = crate::command_context::Context::source(state, connection, executor, 2);
    crate::command_context::submit(state, text, context, changes);
}

fn drain(state: &mut TickState, changes: &mut ChangeSet) {
    if state.datapacks.running {
        return;
    }
    state.datapacks.running = true;
    if state.datapacks.budget_tick != state.tick {
        state.datapacks.budget_tick = state.tick;
        state.datapacks.used = 0;
    }
    while let Some(mut frame) = state.datapacks.pending.pop() {
        if frame.next == frame.lines.len() {
            continue;
        }
        if state.datapacks.used >= COMMAND_BUDGET {
            state.datapacks.pending.clear();
            feedback(
                state,
                frame.context.connection,
                "Datapack command budget exceeded (65536 commands per tick)",
            );
            break;
        }
        state.datapacks.used += 1;
        let lines = frame.lines.clone();
        let index = frame.next;
        let context = frame.context;
        frame.next += 1;
        state.datapacks.pending.push(frame);
        crate::command_context::submit(state, &lines[index], context, changes);
    }
    state.datapacks.running = false;
}

pub(crate) fn command(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    text: &str,
    changes: &mut ChangeSet,
) -> crate::command_execution::CommandResult {
    use crate::command_execution::CommandResult::{Failed, Unavailable};
    let Some(root) = text.split_whitespace().next() else {
        return Unavailable;
    };
    if root == "data" {
        return crate::command_data::command(state, connection, executor, text);
    }
    if !matches!(root, "function" | "schedule" | "reload" | "datapack") {
        return Unavailable;
    }
    let words = text.split_whitespace().collect::<Vec<_>>();
    let result = match words.as_slice() {
        ["function", name] => enqueue(state, name, connection, executor),
        ["schedule", "clear", name] => {
            let tag = name.starts_with('#');
            identifier(name.strip_prefix('#').unwrap_or(name)).map(|name| {
                let name = if tag { format!("#{name}") } else { name };
                state.datapacks.scheduled.retain(|(_, scheduled), _| scheduled != &name);
                state.datapacks.dirty = true;
            })
        }
        ["schedule", "function", name, time] | ["schedule", "function", name, time, "replace" | "append"] => {
            (|| {
                let multiplier = if time.ends_with('d') { 24000.0 } else if time.ends_with('s') { 20.0 } else { 1.0 };
                let number = time.strip_suffix(['d', 's', 't']).unwrap_or(time);
                let delay = number.parse::<f64>().map_err(|_| "Invalid schedule time")? * multiplier;
                if !delay.is_finite() || delay < 1.0 || delay > i32::MAX as f64 { return Err("Schedule delay must be 1..2147483647 ticks".into()); }
                let id = identifier(name.strip_prefix('#').unwrap_or(name))?;
                let exists = if name.starts_with('#') { state.datapacks.packs.tags.contains_key(&id) } else { state.datapacks.packs.functions.contains_key(&id) };
                if !exists { return Err(format!("Unknown function {name}")); }
                let id = if name.starts_with('#') { format!("#{id}") } else { id };
                if words.last() != Some(&"append") { state.datapacks.scheduled.retain(|(_, name), _| name != &id); }
                if state.datapacks.scheduled.len() >= MAX_RESOURCES { return Err("Scheduled function limit exceeded".into()); }
                let due = crate::command_context::execution_tick(state).saturating_add(delay.round() as u64);
                state.datapacks.scheduled.insert((due, id), ());
                state.datapacks.dirty = true;
                Ok(())
            })()
        }
        ["reload"] => {
            if state.datapacks.reload.is_some() { Err("Datapack reload already in progress".into()) }
            else if let Some(root) = state.datapacks.root.clone() {
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                match std::thread::Builder::new().name("datapack-reload".into()).spawn(move || { let _ = tx.send(PackSet::load(&root)); }) {
                    Ok(_) => { state.datapacks.reload = Some(rx); Ok(()) }
                    Err(error) => Err(error.to_string()),
                }
            } else { Err("No datapack directory configured".into()) }
        }
        ["datapack", "list"] | ["datapack", "list", "enabled"] => {
            feedback(state, connection, format!("Enabled directory datapacks: {}", state.datapacks.packs.names.join(", ")));
            Ok(())
        }
        _ => Err("Supported: function <id|#tag>; schedule function <id> <time> [append|replace]; schedule clear <id>; reload; datapack list".into()),
    };
    let outcome = if let Err(error) = result {
        feedback(state, connection, error);
        Failed
    } else {
        Unavailable
    };
    drain(state, changes);
    outcome
}

pub(crate) fn tick(state: &mut TickState, changes: &mut ChangeSet) {
    if let Some(receiver) = &state.datapacks.reload {
        match receiver.try_recv() {
            Ok(result) => {
                state.datapacks.reload = None;
                match result {
                    Ok(packs) if packs.resources == state.datapacks.packs.resources => {
                        state.datapacks.packs = Arc::new(packs);
                        state.datapacks.load_pending = true;
                        feedback(
                            state,
                            crate::command_console::CONNECTION,
                            "Datapack functions reloaded",
                        );
                    }
                    Ok(_) => feedback(
                        state,
                        crate::command_console::CONNECTION,
                        "Recipe/loot changes require restart; reload refused, old packs retained",
                    ),
                    Err(error) => feedback(
                        state,
                        crate::command_console::CONNECTION,
                        format!("Datapack reload refused: {error}"),
                    ),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                state.datapacks.reload = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
    let connection = crate::command_console::CONNECTION;
    let executor = crate::command_console::EXECUTOR;
    if std::mem::take(&mut state.datapacks.load_pending)
        && state.datapacks.packs.tags.contains_key("minecraft:load")
    {
        let _ = enqueue(state, "#minecraft:load", connection, executor);
        drain(state, changes);
    }
    for _ in 0..COMMAND_BUDGET {
        if !state
            .datapacks
            .scheduled
            .first_key_value()
            .is_some_and(|((due, _), _)| *due <= state.tick)
        {
            break;
        }
        let ((_, name), ()) = state.datapacks.scheduled.pop_first().expect("due entry");
        state.datapacks.dirty = true;
        if let Err(error) = enqueue(state, &name, connection, executor) {
            feedback(state, connection, error);
        }
        drain(state, changes);
    }
    if state.datapacks.packs.tags.contains_key("minecraft:tick") {
        let _ = enqueue(state, "#minecraft:tick", connection, executor);
        drain(state, changes);
    }
}

#[cfg(test)]
#[path = "datapacks_tests.rs"]
mod tests;
