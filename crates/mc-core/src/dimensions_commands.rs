//! Resolve global player commands once, then mutate their native owning worlds.
use super::Dimension;
use super::travel::{Request, locate, world};
use crate::boundary::{ChangeSet, EntityId};
use crate::commands::{Command, SelectorKind, SelectorSort, Target};
use crate::tick::TickState;
use std::collections::BTreeMap;

pub(crate) fn target(command: &Command) -> Option<&Target> {
    match command {
        Command::EffectGive { target, .. }
        | Command::EffectClear { target, .. }
        | Command::GameMode { target, .. }
        | Command::Give { target, .. }
        | Command::Clear { target, .. }
        | Command::Kill { target, .. }
        | Command::Experience { target, .. }
        | Command::ExperienceSet { target, .. }
        | Command::ExperienceQuery { target, .. }
        | Command::Advancement { target, .. }
        | Command::Message { target, .. } => Some(target),
        Command::Enchant(command) => Some(&command.target),
        Command::World(crate::command_world::WorldCommand::SpawnPoint { target, .. }) => {
            Some(target)
        }
        _ => None,
    }
}

fn target_mut(command: &mut Command) -> &mut Target {
    match command {
        Command::EffectGive { target, .. }
        | Command::EffectClear { target, .. }
        | Command::GameMode { target, .. }
        | Command::Give { target, .. }
        | Command::Clear { target, .. }
        | Command::Kill { target, .. }
        | Command::Experience { target, .. }
        | Command::ExperienceSet { target, .. }
        | Command::ExperienceQuery { target, .. }
        | Command::Advancement { target, .. }
        | Command::Message { target, .. } => target,
        Command::Enchant(command) => &mut command.target,
        _ => unreachable!("target-bearing native command"),
    }
}

pub(crate) fn is_global(command: &Command) -> bool {
    target(command).is_some_and(global_target)
}

pub(super) fn global_target(target: &Target) -> bool {
    match target {
        Target::Player(_) | Target::AllPlayers => true,
        Target::Selector(selector) => {
            matches!(
                selector.kind,
                SelectorKind::AllPlayers | SelectorKind::Random
            ) && selector.distance.is_none()
                && selector.position.iter().all(Option::is_none)
                && selector.extent.iter().all(Option::is_none)
        }
        _ => false,
    }
}

pub(super) fn resolve(
    state: &TickState,
    request: &Request,
    target: &Target,
    origin: [f64; 3],
) -> Vec<EntityId> {
    if !global_target(target) {
        return world(state, request.dimension)
            .map(|world| crate::command_execution::resolve_target(world, request.executor, target))
            .unwrap_or_default();
    }
    let mut selected = Vec::new();
    let selector = if let Target::Selector(selector) = target {
        let mut selector = selector.clone();
        selector.position = origin.map(Some);
        selector.limit = Some(usize::MAX);
        selector.sort = Some(SelectorSort::Arbitrary);
        Some(Target::Selector(selector))
    } else {
        None
    };
    for world in std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        selected.extend(crate::command_execution::resolve_target(
            world,
            request.executor,
            selector.as_ref().unwrap_or(target),
        ));
    }
    if let Target::Selector(selector) = target {
        let distance = |entity| {
            let (dimension, slot) = locate(state, entity).expect("selected entity");
            let position = world(state, dimension)
                .expect("selected world")
                .entities
                .pos(slot);
            position
                .iter()
                .zip(origin)
                .map(|(coordinate, origin)| (coordinate - origin).powi(2))
                .sum::<f64>()
        };
        match selector
            .sort
            .unwrap_or(if selector.kind == SelectorKind::Random {
                SelectorSort::Random
            } else {
                SelectorSort::Arbitrary
            }) {
            SelectorSort::Nearest => {
                selected.sort_by(|a, b| distance(*a).total_cmp(&distance(*b)).then(a.cmp(b)))
            }
            SelectorSort::Furthest => {
                selected.sort_by(|a, b| distance(*b).total_cmp(&distance(*a)).then(a.cmp(b)))
            }
            SelectorSort::Random => selected.sort_by_key(|id| {
                let mut value = (id.0 as u32 as u64)
                    ^ state.tick.wrapping_add(request.executor.0 as u32 as u64);
                value = value.wrapping_add(0x9e3779b97f4a7c15);
                value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
                value ^ (value >> 31)
            }),
            SelectorSort::Arbitrary => selected.sort_unstable(),
        }
        selected.truncate(
            selector
                .limit
                .unwrap_or(if selector.kind == SelectorKind::Random {
                    1
                } else {
                    usize::MAX
                }),
        );
    } else {
        selected.sort_unstable();
    }
    selected
}

fn commit(world: &mut TickState, changes: &mut ChangeSet) {
    if !world.merge_damage.is_empty() {
        let (_, mut events, mut spawned, mut removed) = world.commit_merge_effects();
        changes.entity_events.append(&mut events);
        changes.entities_spawned.append(&mut spawned);
        changes.entities_removed.append(&mut removed);
        crate::entity_life::commit_drops(world, &mut changes.entities_spawned);
    }
    changes
        .entities_appearance_changed
        .append(&mut world.appearance_changed);
}

pub(crate) fn execute(
    state: &mut TickState,
    mut request: Request,
    changes: &mut BTreeMap<Dimension, ChangeSet>,
) -> Result<(), String> {
    let source_dimension = locate(state, request.executor)
        .map(|(dimension, _)| dimension)
        .unwrap_or(state.dimension);
    let source = world(state, source_dimension).expect("executor world");
    let (origin, _) = crate::command_console::pose(source, request.executor)
        .ok_or("argument.entity.notfound.entity")?;
    let selected = resolve(
        state,
        &request,
        target(&request.command).expect("global command target"),
        origin,
    );
    if selected.is_empty() {
        return Err("argument.entity.notfound.entity".into());
    }
    if matches!(request.command, Command::ExperienceQuery { .. }) && selected.len() != 1 {
        return Err("argument.player.toomany".into());
    }
    if let Command::World(crate::command_world::WorldCommand::SpawnPoint {
        position,
        rotation,
        ..
    }) = &request.command
    {
        let location = crate::command_world::spawnpoint_location(
            source,
            request.executor,
            *position,
            *rotation,
        )
        .map_err(|error| error.message())?;
        let mut count = 0;
        for entity in selected {
            let (dimension, _) = locate(state, entity).expect("selected entity");
            let world = if dimension == state.dimension {
                &mut *state
            } else {
                state
                    .other_dimensions
                    .get_mut(&dimension)
                    .expect("selected world")
            };
            if let Some(player) = world.players.get_mut(&entity) {
                player.respawn_point = Some(location.0);
                player.respawn_dimension = source_dimension;
                player.respawn_forced = true;
                player.respawn_yaw = location.1[0];
                player.respawn_pitch = location.1[1];
                count += 1;
            }
        }
        if count == 0 {
            return Err("argument.entity.notfound.player".into());
        }
        state.push_player(
            request.connection,
            crate::boundary::PlayerChange::Chat {
                message: format!(
                    "Set spawn point for {count} player(s) to {}, {}, {}",
                    location.0[0], location.0[1], location.0[2]
                ),
            },
        );
        return Ok(());
    }
    for entity in selected {
        let (dimension, _) = locate(state, entity).expect("selected entity");
        *target_mut(&mut request.command) = Target::Entity(entity);
        let changes = changes.entry(dimension).or_default();
        if dimension == state.dimension {
            crate::command_execution::execute(
                state,
                request.connection,
                request.executor,
                &request.command,
                changes,
            );
            commit(state, changes);
        } else {
            let mut world = state
                .other_dimensions
                .remove(&dimension)
                .expect("selected world");
            super::runtime::swap_globals(state, &mut world);
            crate::command_execution::execute(
                &mut world,
                request.connection,
                request.executor,
                &request.command,
                changes,
            );
            commit(&mut world, changes);
            super::runtime::swap_globals(state, &mut world);
            state.other_dimensions.insert(dimension, world);
        }
        super::travel::sync_wire_ids(state);
    }
    Ok(())
}
