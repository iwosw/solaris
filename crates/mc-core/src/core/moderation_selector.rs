//! Tick-owned selector resolution for edge moderation commands.
use super::*;
use crate::commands::{Command, SelectorKind, Target};

#[cfg(test)]
#[path = "moderation_selector_tests.rs"]
mod tests;

#[derive(Debug)]
pub(super) struct Request {
    pub source: ConnectionId,
    pub selector: String,
    pub reply: tokio::sync::oneshot::Sender<Result<Vec<ModerationPlayer>, String>>,
}

pub(super) fn answer(state: &TickState, request: Request) {
    let result = resolve(state, request.source, &request.selector);
    let _ = request.reply.send(result);
}

fn resolve(
    state: &TickState,
    source: ConnectionId,
    text: &str,
) -> Result<Vec<ModerationPlayer>, String> {
    // The shared parser currently represents @n and @p as the same nearest
    // target, but @n is entity-type and is not a player-profile argument.
    if text == "@n" || text.starts_with("@n[") {
        return Err("Only player selectors are allowed".into());
    }
    let target = crate::commands::target_of(text).map_err(|error| error.message())?;
    if let Target::Selector(selector) = &target
        && matches!(selector.kind, SelectorKind::Entities)
        && !selector
            .entity_types
            .iter()
            .any(|(id, tag, negated)| !tag && !negated && id.as_str() == "minecraft:player")
    {
        return Err("Only player selectors are allowed".into());
    }
    let (world, executor) = if source == crate::command_console::CONNECTION {
        (state, crate::command_console::EXECUTOR)
    } else {
        std::iter::once(state)
            .chain(state.other_dimensions.values().map(|world| &**world))
            .find_map(|world| {
                world
                    .connections
                    .entity_of(source)
                    .map(|entity| (world, entity))
            })
            .ok_or("Command source is unavailable")?
    };
    let origin = crate::command_console::pose(world, executor)
        .ok_or("Command source is unavailable")?
        .0;
    let query = crate::dimensions::travel::Request {
        connection: source,
        executor,
        dimension: world.dimension,
        command: Command::Kill {
            target: Target::Executor,
        },
    };
    let selected = crate::dimensions::commands::resolve(state, &query, &target, origin);
    Ok(selected
        .into_iter()
        .filter_map(|entity| {
            let (dimension, _) = crate::dimensions::travel::locate(state, entity)?;
            let world = crate::dimensions::travel::world(state, dimension)?;
            if !world.players.contains_key(&entity) {
                return None;
            }
            let connection = world.connections.entity_connection(entity)?;
            Some((
                connection,
                world.connections.uuid_of(connection)?,
                world.connections.name_of(entity)?.to_owned(),
            ))
        })
        .collect())
}
