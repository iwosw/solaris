//! Tick-owned vanilla command execution and target selection.
use crate::boundary::{ChangeSet, ConnectionId, EntityId, PlayerChange};
use crate::commands::{
    Command, Destination, Selector, SelectorKind, SelectorSort, Target, Weather,
};
use crate::entities::EntityKind;
use crate::tick::{DamagePiece, TickState};
use mc_nbt::{ListTag, Tag};

fn text(value: impl ToString) -> Tag {
    Tag::Compound(vec![("text".into(), Tag::String(value.to_string()))])
}

fn translated(key: impl Into<String>, arguments: Vec<Tag>) -> Tag {
    let mut fields = vec![("translate".into(), Tag::String(key.into()))];
    if !arguments.is_empty() {
        fields.push((
            "with".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: arguments,
            }),
        ));
    }
    Tag::Compound(fields)
}

pub(crate) fn feedback(
    state: &mut TickState,
    connection: ConnectionId,
    key: &str,
    arguments: Vec<Tag>,
) {
    state.push_player(
        connection,
        PlayerChange::StyledChat {
            content: translated(key, arguments),
        },
    );
}

fn error(state: &mut TickState, connection: ConnectionId, message: impl Into<String>) {
    state.push_player(
        connection,
        PlayerChange::Chat {
            message: message.into(),
        },
    );
}

fn entity_type_name(state: &TickState, entity: EntityId) -> Option<&'static str> {
    let body = state.entities.body(state.entities.slot_of(entity)?);
    match body.kind {
        EntityKind::Player { .. } => Some("minecraft:player"),
        EntityKind::Mob { type_id }
        | EntityKind::Item { type_id }
        | EntityKind::ExperienceOrb { type_id } => {
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .map(|contract| contract.name)
        }
    }
}

pub(crate) fn entity_name(state: &TickState, entity: EntityId) -> Tag {
    if entity == crate::command_console::EXECUTOR {
        return text("Server");
    }
    if let Some(name) = state.connections.name_of(entity) {
        return text(name);
    }
    if let Some(player) = state
        .dimension_globals
        .roster
        .iter()
        .find(|player| player.entity == entity)
    {
        return text(&player.name);
    }
    if let Some(slot) = state.entities.slot_of(entity)
        && let Some(name) = &state.entities.row(slot).appearance.custom_name
    {
        return text(name);
    }
    let name = entity_type_name(state, entity)
        .unwrap_or("minecraft:generic")
        .replace(':', ".");
    translated(format!("entity.{name}"), Vec::new())
}

fn selector(state: &TickState, executor: EntityId, selector: &Selector) -> Vec<EntityId> {
    let origin = state
        .entities
        .slot_of(executor)
        .map(|slot| state.entities.pos(slot))
        .unwrap_or_else(|| state.world.world_spawn_position());
    let origin: [f64; 3] =
        std::array::from_fn(|axis| selector.position[axis].unwrap_or(origin[axis]));
    let players_only = matches!(
        selector.kind,
        SelectorKind::Nearest | SelectorKind::AllPlayers | SelectorKind::Random
    );
    let mut candidates: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            if !body.alive
                || (players_only && !matches!(body.kind, EntityKind::Player { .. }))
                || (selector.kind == SelectorKind::Executor && body.id != executor)
            {
                return None;
            }
            if let Some((kind, inverted)) = &selector.entity_type {
                let matched = entity_type_name(state, body.id) == Some(kind.as_str());
                if matched == *inverted {
                    return None;
                }
            }
            if let Some((name, inverted)) = &selector.name {
                let actual = state
                    .connections
                    .name_of(body.id)
                    .or(row.appearance.custom_name.as_deref())
                    .unwrap_or_else(|| entity_type_name(state, body.id).unwrap_or(""));
                if (actual == name) == *inverted {
                    return None;
                }
            }
            let pos = state.entities.pos(slot);
            if selector.extent.iter().enumerate().any(|(axis, extent)| {
                extent.is_some_and(|extent| {
                    pos[axis] < origin[axis] + extent.min(0.0)
                        || pos[axis] >= origin[axis] + extent.max(0.0) + 1.0
                })
            }) {
                return None;
            }
            let distance = (pos[0] - origin[0]).powi(2)
                + (pos[1] - origin[1]).powi(2)
                + (pos[2] - origin[2]).powi(2);
            if let Some((min, max)) = selector.distance
                && (min.is_some_and(|min| distance < min * min)
                    || max.is_some_and(|max| distance > max * max))
            {
                return None;
            }
            Some((body.id, distance))
        })
        .collect();
    let sort = selector.sort.unwrap_or(match selector.kind {
        SelectorKind::Nearest => SelectorSort::Nearest,
        SelectorKind::Random => SelectorSort::Random,
        _ => SelectorSort::Arbitrary,
    });
    match sort {
        SelectorSort::Nearest => candidates.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0))),
        SelectorSort::Furthest => {
            candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)))
        }
        SelectorSort::Random => candidates.sort_by_key(|(id, _)| {
            let mut value =
                (id.0 as u32 as u64) ^ state.tick.wrapping_add(executor.0 as u32 as u64);
            value = value.wrapping_add(0x9e3779b97f4a7c15);
            value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
            value ^ (value >> 31)
        }),
        SelectorSort::Arbitrary => candidates.sort_by_key(|(id, _)| *id),
    }
    let default_limit = if matches!(
        selector.kind,
        SelectorKind::Nearest | SelectorKind::Random | SelectorKind::Executor
    ) {
        1
    } else {
        usize::MAX
    };
    candidates
        .into_iter()
        .take(selector.limit.unwrap_or(default_limit))
        .map(|(id, _)| id)
        .collect()
}

pub(crate) fn resolve_target(
    state: &TickState,
    executor: EntityId,
    target: &Target,
) -> Vec<EntityId> {
    match target {
        Target::Entity(entity) => state
            .entities
            .slot_of(*entity)
            .map(|_| vec![*entity])
            .unwrap_or_default(),
        Target::Executor => state
            .entities
            .slot_of(executor)
            .filter(|slot| state.entities.body(*slot).alive)
            .map(|_| vec![executor])
            .unwrap_or_default(),
        Target::Nearest | Target::AllPlayers => selector(
            state,
            executor,
            &Selector {
                kind: if matches!(target, Target::Nearest) {
                    SelectorKind::Nearest
                } else {
                    SelectorKind::AllPlayers
                },
                entity_type: None,
                name: None,
                distance: None,
                position: [None; 3],
                extent: [None; 3],
                limit: None,
                sort: None,
            },
        ),
        Target::Selector(value) => selector(state, executor, value),
        Target::Player(name) => {
            let uuid = uuid::Uuid::parse_str(name).ok();
            state
                .entities
                .slots()
                .filter_map(|slot| {
                    let body = state.entities.body(slot);
                    let row = state.entities.row(slot);
                    let actual = state
                        .connections
                        .name_of(body.id)
                        .or(row.appearance.custom_name.as_deref())
                        .unwrap_or_else(|| entity_type_name(state, body.id).unwrap_or(""));
                    (body.alive && (actual == name || uuid == Some(row.uuid))).then_some(body.id)
                })
                .collect()
        }
    }
}

pub(crate) fn player_targets(
    state: &TickState,
    executor: EntityId,
    target: &Target,
) -> Vec<EntityId> {
    let mut targets = resolve_target(state, executor, target);
    targets.retain(|id| state.players.contains_key(id));
    targets
}

fn targets_label(state: &TickState, targets: &[EntityId]) -> Tag {
    if targets.len() == 1 {
        entity_name(state, targets[0])
    } else {
        text(targets.len())
    }
}

fn item_name(state: &TickState, item: &mc_data::Identifier) -> Tag {
    let category = if state.registry.blocks.block(item).is_some() {
        "block"
    } else {
        "item"
    };
    translated(
        format!("{category}.{}", item.as_str().replace(':', ".")),
        Vec::new(),
    )
}

fn mode_name(mode: crate::commands::GameMode) -> Tag {
    translated(
        format!(
            "gameMode.{}",
            match mode {
                crate::commands::GameMode::Survival => "survival",
                crate::commands::GameMode::Creative => "creative",
                crate::commands::GameMode::Adventure => "adventure",
                crate::commands::GameMode::Spectator => "spectator",
            }
        ),
        Vec::new(),
    )
}

fn difficulty_name(difficulty: crate::survival::Difficulty) -> Tag {
    translated(
        format!(
            "options.difficulty.{}",
            match difficulty {
                crate::survival::Difficulty::Peaceful => "peaceful",
                crate::survival::Difficulty::Easy => "easy",
                crate::survival::Difficulty::Normal => "normal",
                crate::survival::Difficulty::Hard => "hard",
            }
        ),
        Vec::new(),
    )
}

fn advancement_ids(
    tree: &mc_data::advancements::AdvancementTree,
    scope: crate::commands::AdvancementScope,
    advancement: Option<&str>,
) -> Result<Vec<String>, String> {
    use crate::commands::AdvancementScope;
    if scope == AdvancementScope::Everything {
        return Ok(tree.definitions.keys().cloned().collect());
    }
    let id = advancement.ok_or("Missing advancement")?;
    let definition = tree
        .definitions
        .get(id)
        .ok_or_else(|| format!("Unknown advancement '{id}'"))?;
    let mut ids = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    seen.insert(id);
    if matches!(scope, AdvancementScope::Through | AdvancementScope::Until) {
        let mut parent = definition.parent.as_deref();
        while let Some(id) = parent {
            if !seen.insert(id) {
                break;
            }
            let Some(definition) = tree.definitions.get(id) else {
                break;
            };
            ids.push(id.to_owned());
            parent = definition.parent.as_deref();
        }
    }
    ids.push(id.to_owned());
    if matches!(scope, AdvancementScope::From | AdvancementScope::Through) {
        let mut children = std::collections::BTreeMap::<&str, Vec<&str>>::new();
        for (id, definition) in &tree.definitions {
            if let Some(parent) = definition.parent.as_deref() {
                children.entry(parent).or_default().push(id);
            }
        }
        let mut pending = vec![id];
        while let Some(parent) = pending.pop() {
            if let Some(children) = children.get(parent) {
                for &child in children.iter().rev() {
                    if seen.insert(child) {
                        ids.push(child.to_owned());
                        pending.push(child);
                    }
                }
            }
        }
    }
    Ok(ids)
}

pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &Command,
    changes: &mut ChangeSet,
) {
    match command {
        Command::Advancement {
            target,
            grant,
            scope,
            advancement,
            criterion,
        } => {
            let ids =
                match advancement_ids(&state.registry.advancements, *scope, advancement.as_deref())
                {
                    Ok(ids) => ids,
                    Err(message) => {
                        error(state, connection, message);
                        return;
                    }
                };
            if let (Some(id), Some(criterion)) = (advancement, criterion)
                && !state.registry.advancements.definitions[id]
                    .criteria
                    .contains_key(criterion)
            {
                error(
                    state,
                    connection,
                    format!("Unknown criterion '{criterion}' for advancement '{id}'"),
                );
                return;
            }
            let targets = player_targets(state, executor, target);
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.player",
                    Vec::new(),
                );
                return;
            }
            let mut count = 0;
            for &entity in &targets {
                count += if *grant {
                    crate::advancements::grant(state, entity, &ids, criterion.as_deref())
                } else {
                    crate::advancements::revoke(state, entity, &ids, criterion.as_deref())
                };
            }
            let operation = if *grant { "grant" } else { "revoke" };
            let result = if count == 0 { "failure" } else { "success" };
            let destination = if targets.len() == 1 { "one" } else { "many" };
            let label = targets_label(state, &targets);
            let (subject, arguments) = if let Some(criterion) = criterion {
                (
                    "criterion",
                    vec![
                        text(criterion),
                        text(advancement.as_deref().unwrap_or("")),
                        label,
                    ],
                )
            } else if ids.len() == 1 {
                ("one", vec![text(&ids[0]), label])
            } else {
                ("many", vec![text(ids.len()), label])
            };
            feedback(
                state,
                connection,
                &format!("commands.advancement.{operation}.{subject}.to.{destination}.{result}"),
                arguments,
            );
        }
        Command::EffectGive { .. } | Command::EffectClear { .. } => {
            crate::effects::execute_command(state, connection, executor, command)
        }
        Command::GameMode { target, mode } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return;
            };
            game_mode(
                state, connection, executor, &targets, *mode, command, changes,
            );
        }
        Command::Give {
            target,
            item,
            count,
        } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return;
            };
            give(state, connection, &targets, item, *count, command, changes);
        }
        Command::Clear { target, item, max } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return;
            };
            clear(
                state,
                connection,
                &targets,
                item.as_ref(),
                *max,
                command,
                changes,
            );
        }
        Command::Experience {
            target,
            amount,
            levels,
        } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return;
            };
            for &target in &targets {
                apply_player_command(state, target, command, changes);
            }
            experience_success(state, connection, "add", *amount, *levels, &targets);
        }
        Command::ExperienceSet {
            target,
            amount,
            levels,
        } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return;
            };
            // Points beyond the current level's bar are invalid; vanilla
            // fails only when no target accepts them.
            let accepts = |state: &TickState, id: &EntityId| {
                *levels
                    || *amount
                        < crate::survival::Experience::points_for_level(
                            state.players[id].vitals.experience.level,
                        )
            };
            if !targets.iter().any(|id| accepts(state, id)) {
                feedback(
                    state,
                    connection,
                    "commands.experience.set.points.invalid",
                    Vec::new(),
                );
                return;
            }
            for &target in &targets {
                if accepts(state, &target) {
                    apply_player_command(state, target, command, changes);
                }
            }
            experience_success(state, connection, "set", *amount, *levels, &targets);
        }
        Command::ExperienceQuery { target, levels } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return;
            };
            let [target] = targets[..] else {
                feedback(state, connection, "argument.player.toomany", Vec::new());
                return;
            };
            let experience = state.players[&target].vitals.experience;
            let (key, amount) = if *levels {
                ("commands.experience.query.levels", experience.level)
            } else {
                (
                    "commands.experience.query.points",
                    (experience.progress
                        * crate::survival::Experience::points_for_level(experience.level) as f32)
                        .floor() as i32,
                )
            };
            feedback(
                state,
                connection,
                key,
                vec![entity_name(state, target), text(amount)],
            );
        }
        Command::Enchant(enchant_command) => {
            let targets = resolve_target(state, executor, &enchant_command.target);
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.entity",
                    Vec::new(),
                );
                return;
            }
            let single = targets.len() == 1;
            let mut successful = Vec::new();
            for target in targets {
                let Some(player) = state.players.get(&target) else {
                    if single {
                        feedback(
                            state,
                            connection,
                            "commands.enchant.failed.itemless",
                            vec![entity_name(state, target)],
                        );
                        return;
                    }
                    continue;
                };
                if let Err(message) = crate::command_enchant::validate(
                    &state.registry.item_rules(),
                    &player.inventory,
                    enchant_command,
                ) {
                    if single {
                        error(state, connection, message);
                    }
                    continue;
                }
                let effects = state
                    .players
                    .get_mut(&target)
                    .expect("checked player exists")
                    .apply_command(&state.registry.item_rules(), command);
                if let Some(target_connection) = state.connections.entity_connection(target) {
                    crate::tick::apply_player_effects(
                        state,
                        target_connection,
                        target,
                        effects,
                        changes,
                    );
                }
                successful.push(target);
            }
            if !successful.is_empty() {
                let enchantment = translated(
                    format!(
                        "enchantment.{}",
                        enchant_command.enchantment.as_str().replace(':', ".")
                    ),
                    Vec::new(),
                );
                feedback(
                    state,
                    connection,
                    if successful.len() == 1 {
                        "commands.enchant.success.single"
                    } else {
                        "commands.enchant.success.multiple"
                    },
                    vec![enchantment, targets_label(state, &successful)],
                );
            } else if !single {
                feedback(state, connection, "commands.enchant.failed", Vec::new());
            }
        }
        Command::DebugGive {
            item,
            count,
            hotbar_slot,
        } => {
            let mut replacement = match item.stack(&state.registry.items) {
                Ok(stack) => stack,
                Err(reason) => {
                    error(state, connection, reason);
                    return;
                }
            };
            replacement = if *count <= 0 {
                mc_data::ItemStack::EMPTY
            } else {
                replacement.count = (*count).min(255);
                replacement
            };
            if crate::plugin_inventories::below_player_replacement_floors(
                state,
                executor,
                &[(usize::from((*hotbar_slot).min(8)), &replacement)],
            ) {
                error(
                    state,
                    connection,
                    "Inventory is reserved by an active native operation",
                );
                return;
            }
            let effects = state
                .players
                .get_mut(&executor)
                .map(|player| player.apply_command(&state.registry.item_rules(), command))
                .unwrap_or_default();
            crate::tick::apply_player_effects(state, connection, executor, effects, changes);
        }
        Command::Teleport {
            target,
            destination,
            rotation: facing,
        } => {
            let targets = resolve_target(state, executor, target);
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.entity",
                    Vec::new(),
                );
                return;
            }
            let origin = state
                .entities
                .slot_of(executor)
                .map(|slot| state.entities.pos(slot))
                .unwrap_or_else(|| state.world.world_spawn_position());
            let rotation = state
                .entities
                .slot_of(executor)
                .map(|slot| {
                    let [yaw, pitch, _] = state.entities.rot(slot);
                    [yaw, pitch]
                })
                .unwrap_or([0.0, 0.0]);
            let (to, destination_name, destination_rotation) = match destination {
                Destination::Position(coords) => (
                    crate::commands::resolve_coords(*coords, origin, rotation),
                    None,
                    None,
                ),
                Destination::Entity(target) => {
                    let ids = resolve_target(state, executor, target);
                    if ids.len() != 1 {
                        feedback(
                            state,
                            connection,
                            if ids.is_empty() {
                                "argument.entity.notfound.entity"
                            } else {
                                "argument.entity.toomany"
                            },
                            Vec::new(),
                        );
                        return;
                    }
                    (
                        state.entities.pos(
                            state
                                .entities
                                .slot_of(ids[0])
                                .expect("resolved entity exists"),
                        ),
                        Some(entity_name(state, ids[0])),
                        Some(
                            state.entities.rot(
                                state
                                    .entities
                                    .slot_of(ids[0])
                                    .expect("resolved entity exists"),
                            ),
                        ),
                    )
                }
            };
            if !to.iter().all(|value| value.is_finite())
                || to[0].abs() >= 30_000_000.0
                || to[2].abs() >= 30_000_000.0
                || to[1].abs() > 20_000_000.0
            {
                feedback(
                    state,
                    connection,
                    "commands.teleport.invalidPosition",
                    Vec::new(),
                );
                return;
            }
            for &target in &targets {
                let slot = state
                    .entities
                    .slot_of(target)
                    .expect("resolved entity exists");
                let current_rotation =
                    destination_rotation.unwrap_or_else(|| state.entities.rot(slot));
                let [yaw, pitch] = facing
                    .map(|coords| {
                        [
                            coords[0].resolve(f64::from(rotation[0])) as f32,
                            coords[1].resolve(f64::from(rotation[1])) as f32,
                        ]
                    })
                    .unwrap_or([current_rotation[0], current_rotation[1]]);
                let wrap = |angle: f32| (angle + 180.0).rem_euclid(360.0) - 180.0;
                state
                    .entities
                    .set_rot(slot, wrap(yaw), wrap(pitch).clamp(-90.0, 90.0));
                crate::mounts::dismount(state, target);
                state.entities.set_pos(slot, to);
                state.entities.set_ground(slot, false);
                if let Some(player) = state.players.get_mut(&target) {
                    player.fall_distance = 0.0;
                }
                if let Some(target_connection) = state.connections.entity_connection(target) {
                    let [yaw, pitch, _] = state.entities.rot(slot);
                    state.correct_position(target_connection, to, yaw, pitch);
                }
            }
            let suffix = if targets.len() == 1 {
                "single"
            } else {
                "multiple"
            };
            let mut args = vec![targets_label(state, &targets)];
            let kind = if let Some(name) = destination_name {
                args.push(name);
                "entity"
            } else {
                args.extend(to.map(text));
                "location"
            };
            feedback(
                state,
                connection,
                &format!("commands.teleport.success.{kind}.{suffix}"),
                args,
            );
        }
        Command::Time { ticks, add } => {
            state.day_time = if *add {
                state.day_time.saturating_add(*ticks).max(0)
            } else {
                *ticks
            };
            feedback(
                state,
                connection,
                "commands.time.set.absolute",
                vec![text("minecraft:overworld"), text(state.day_time)],
            );
        }
        Command::TimeMarker { name, ticks } => {
            let delta = *ticks - state.day_time.rem_euclid(24000);
            state.day_time =
                state
                    .day_time
                    .saturating_add(if delta > 0 { delta } else { delta + 24000 });
            feedback(
                state,
                connection,
                "commands.time.set.time_marker",
                vec![text("minecraft:overworld"), text(name)],
            );
        }
        Command::TimeQuery(query) => feedback(
            state,
            connection,
            "commands.time.query",
            vec![text(query.value(
                state.tick_control.game_ticks(state.tick),
                state.day_time,
            ))],
        ),
        Command::Weather {
            weather,
            duration_ticks,
        } => {
            if state.dimension_globals.active && !state.dimension.has_weather() {
                state.dimension_globals.broadcasts.push(
                    crate::dimensions::globals::Broadcast::Weather(*weather, *duration_ticks),
                );
            } else {
                crate::weather::set_command(state, *weather, *duration_ticks);
            }
            feedback(
                state,
                connection,
                match weather {
                    Weather::Clear => "commands.weather.set.clear",
                    Weather::Rain => "commands.weather.set.rain",
                    Weather::Thunder => "commands.weather.set.thunder",
                },
                Vec::new(),
            );
        }
        Command::Difficulty(difficulty) => {
            if state.difficulty == *difficulty {
                feedback(
                    state,
                    connection,
                    "commands.difficulty.failure",
                    vec![difficulty_name(*difficulty)],
                );
                return;
            }
            state.difficulty = *difficulty;
            if state.dimension_globals.active {
                state.dimension_globals.broadcasts.push(
                    crate::dimensions::globals::Broadcast::Difficulty(difficulty.id()),
                );
            }
            let connections: Vec<_> = state.connections.all_connections().collect();
            for connection in connections {
                state.push_player(
                    connection,
                    PlayerChange::DifficultyChanged {
                        difficulty: difficulty.id(),
                    },
                );
            }
            feedback(
                state,
                connection,
                "commands.difficulty.success",
                vec![difficulty_name(*difficulty)],
            );
        }
        Command::DifficultyQuery => feedback(
            state,
            connection,
            "commands.difficulty.query",
            vec![difficulty_name(state.difficulty)],
        ),
        Command::GameRule { name, value } => {
            if let Some(value) = value
                && state.gamerules.get(name) != Some(value)
            {
                state.gamerules.insert(name.clone(), value.clone());
                state.gamerules_dirty = true;
            }
            let value = state
                .gamerules
                .get(name)
                .map(String::as_str)
                .or_else(|| crate::command_rules::default_value(name))
                .unwrap_or("")
                .to_owned();
            feedback(
                state,
                connection,
                if matches!(command, Command::GameRule { value: Some(_), .. }) {
                    "commands.gamerule.set"
                } else {
                    "commands.gamerule.query"
                },
                vec![text(name), text(value)],
            );
        }
        Command::WorldBorder(command) => {
            let origin = state
                .entities
                .slot_of(executor)
                .map(|slot| state.entities.pos(slot))
                .unwrap_or_else(|| state.world.world_spawn_position());
            let message = crate::command_border::execute(&mut state.border, *command, origin);
            state.push_player(connection, PlayerChange::Chat { message });
        }
        Command::Summon {
            entity,
            position,
            nbt,
        } => {
            let Some(facts) = state.registry.entity_types.facts_of(entity) else {
                error(
                    state,
                    connection,
                    format!("Unknown entity type '{}'", entity.as_str()),
                );
                return;
            };
            if facts.flags.is_some_and(|flags| !flags.is_summonable()) {
                feedback(state, connection, "commands.summon.failed", Vec::new());
                return;
            }
            if state.difficulty == crate::survival::Difficulty::Peaceful
                && facts.category.is_hostile()
            {
                feedback(
                    state,
                    connection,
                    "commands.summon.failed.peaceful",
                    Vec::new(),
                );
                return;
            }
            let type_id = facts.protocol_id;
            let origin = state
                .entities
                .slot_of(executor)
                .map(|slot| state.entities.pos(slot))
                .unwrap_or_else(|| state.world.world_spawn_position());
            let rotation = state
                .entities
                .slot_of(executor)
                .map(|slot| {
                    let [yaw, pitch, _] = state.entities.rot(slot);
                    [yaw, pitch]
                })
                .unwrap_or([0.0, 0.0]);
            let at = position
                .as_ref()
                .map(|coords| crate::commands::resolve_coords(*coords, origin, rotation))
                .unwrap_or(origin);
            if at[0].abs() >= 30_000_000.0
                || at[2].abs() >= 30_000_000.0
                || at[1].abs() > 20_000_000.0
            {
                feedback(
                    state,
                    connection,
                    "commands.summon.invalidPosition",
                    Vec::new(),
                );
                return;
            }
            if entity.as_str() == "minecraft:item" {
                let item = match crate::command_summon::prepare_item(state, nbt) {
                    Ok(item) => item,
                    Err(failure) => {
                        error(state, connection, failure.message());
                        return;
                    }
                };
                let id = crate::tick::spawn_item_entity(state, at, item.stack);
                if let Some(slot) = state.entities.slot_of(id) {
                    if let Some(delay) = item.pickup_delay {
                        state.entities.body_mut(slot).pickup_delay = delay;
                    }
                    if let Some(age) = item.age {
                        state.entities.row_mut(slot).age = age;
                    }
                    if let Some(motion) = item.motion {
                        state.entities.set_vel(slot, motion.map(|v| v * 20.0));
                    }
                }
                changes.entities_spawned.push(id);
                feedback(
                    state,
                    connection,
                    "commands.summon.success",
                    vec![entity_name(state, id)],
                );
                return;
            }
            let image =
                match crate::command_summon::prepare(state, type_id, entity.as_str(), at, nbt) {
                    Ok(image) => image,
                    Err(failure) => {
                        error(state, connection, failure.message());
                        return;
                    }
                };
            let health = state
                .registry
                .blueprint(type_id)
                .map_or(20.0, |blueprint| blueprint.params.max_health);
            let id = state.spawn_mob(type_id, at, health);
            crate::command_summon::apply(state, id, image);
            changes.entities_spawned.push(id);
            feedback(
                state,
                connection,
                "commands.summon.success",
                vec![entity_name(state, id)],
            );
        }
        Command::Kill { target } => {
            let targets = resolve_target(state, executor, target);
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.entity",
                    Vec::new(),
                );
                return;
            }
            for &target in &targets {
                if let Some(slot) = state.entities.slot_of(target) {
                    state.merge_damage.push((
                        slot,
                        DamagePiece {
                            physical: None,
                            custom_armor: false,
                            amount: f32::MAX,
                            kind: crate::survival::DamageKind::Void,
                            source: None,
                        },
                    ));
                }
            }
            feedback(
                state,
                connection,
                if targets.len() == 1 {
                    "commands.kill.success.single"
                } else {
                    "commands.kill.success.multiple"
                },
                vec![targets_label(state, &targets)],
            );
        }
        Command::World(command) => {
            crate::command_world::execute(state, connection, executor, command, changes)
        }
        Command::BlockLog(command) => {
            crate::block_log::execute(state, connection, executor, command);
        }
        Command::BossBar(command) => {
            crate::bossbar::execute(state, connection, executor, command);
        }
        Command::ForceLoad(command) => {
            let reply = crate::forceload::execute(state, executor, command);
            let args = reply.args.into_iter().map(text).collect();
            if reply.failure {
                let Tag::Compound(mut fields) = translated(reply.key, args) else {
                    unreachable!("translated builds a compound");
                };
                fields.push(("color".into(), Tag::String("red".into())));
                state.push_player(
                    connection,
                    PlayerChange::StyledChat {
                        content: Tag::Compound(fields),
                    },
                );
            } else {
                feedback(state, connection, reply.key, args);
            }
        }
        Command::Scoreboard(command) => {
            let message = crate::scoreboard::execute(state, executor, command);
            error(state, connection, message);
        }
        Command::Say(message) | Command::Me { message, .. } => {
            let name = entity_name(state, executor);
            let key = if matches!(command, Command::Say(_)) {
                "chat.type.announcement"
            } else {
                "chat.type.emote"
            };
            let message = if matches!(
                command,
                Command::Me {
                    selectors: false,
                    ..
                }
            ) {
                message.clone()
            } else {
                expand_message(state, executor, message)
            };
            let content = translated(key, vec![name, text(message)]);
            if connection == crate::command_console::CONNECTION {
                state.push_player(
                    connection,
                    PlayerChange::StyledChat {
                        content: content.clone(),
                    },
                );
            }
            let connections: Vec<_> = state.connections.all_connections().collect();
            for connection in connections {
                state.push_player(
                    connection,
                    PlayerChange::StyledChat {
                        content: content.clone(),
                    },
                );
            }
            if state.dimension_globals.active {
                state
                    .dimension_globals
                    .broadcasts
                    .push(crate::dimensions::globals::Broadcast::Chat(content));
            }
        }
        Command::Message {
            target,
            message,
            selectors,
        } => {
            let targets = player_targets(state, executor, target);
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.player",
                    Vec::new(),
                );
                return;
            }
            let message = if *selectors {
                expand_message(state, executor, message)
            } else {
                message.clone()
            };
            for target in targets {
                if let Some(recipient) = state.connections.entity_connection(target) {
                    feedback(
                        state,
                        recipient,
                        "commands.message.display.incoming",
                        vec![entity_name(state, executor), text(&message)],
                    );
                }
                feedback(
                    state,
                    connection,
                    "commands.message.display.outgoing",
                    vec![entity_name(state, target), text(&message)],
                );
            }
        }
        Command::TellRaw { target, message } => {
            crate::command_text::execute_tellraw(state, connection, executor, target, message);
        }
        Command::Title { target, action } => {
            crate::command_text::execute_title(state, connection, executor, target, action);
        }
        Command::Settlement(command) => {
            settlement(state, connection, executor, command);
        }
        Command::Seed => feedback(
            state,
            connection,
            "commands.seed.success",
            vec![text(state.world.seed())],
        ),
        Command::Tick(tick) => crate::command_tick::execute(state, connection, tick),
        Command::Locate(locate) => {
            crate::command_locate::execute(state, connection, executor, locate)
        }
    }
}

/// Native `/settlement` execution. Create is open to any authenticated actor;
/// every record mutation needs the owner or an operator. Reads need nothing.
#[allow(clippy::too_many_lines)]
fn settlement(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &crate::settlements::commands::SettlementCommand,
) {
    use crate::settlements::commands::{SettlementCommand, lookup};
    let operator = state.plugin_players.is_operator(connection)
        || connection == crate::command_console::CONNECTION;
    // Reads resolve below without an actor; every mutation arm re-checks.
    let actor = actor_identity(state, connection);
    match command {
        SettlementCommand::Create { name, radius } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let anchor = state
                .entities
                .slot_of(executor)
                .map(|slot| {
                    let pos = state.entities.pos(slot);
                    pos.map(|coordinate| coordinate.floor() as i32)
                })
                .or_else(|| {
                    (connection == crate::command_console::CONNECTION).then(|| {
                        let pos = state.world.world_spawn_position();
                        pos.map(|coordinate| coordinate.floor() as i32)
                    })
                });
            let Some(anchor) = anchor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            match state.settlements.create(
                &mut state.settlement_sequence,
                name.clone(),
                actor,
                anchor,
                *radius,
            ) {
                Ok(id) => {
                    crate::settlements::refresh_owner_names(state);
                    let record = state.settlements.get(id).expect("created record");
                    let revision = record.revision;
                    error(
                        state,
                        connection,
                        format!("Founded {name} (revision {revision})."),
                    );
                }
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Claim => {
            match crate::settlements::commands::claim(state, connection, None) {
                Ok(message) => error(state, connection, message),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Menu { page } => {
            crate::settlements::ui::open_menu(state, connection, page.as_deref());
        }
        SettlementCommand::List => {
            let mut names: Vec<&str> = state
                .settlements
                .records()
                .map(|record| record.name.as_str())
                .collect();
            names.sort_unstable();
            let message = if names.is_empty() {
                "No settlements.".to_owned()
            } else {
                format!("Settlements: {}.", names.join(", "))
            };
            error(state, connection, message);
        }
        SettlementCommand::Info { name } => {
            let record = match name.as_deref() {
                Some(name) => lookup(state, name),
                None => state.settlements.records().next(),
            };
            match record {
                None => error(state, connection, "Settlement not found."),
                Some(record) => {
                    let members = crate::settlements::member_count(state, record.id);
                    let message = format!(
                        "{}\n{} | owner={} anchor={:?} radius={} members={} revision={}",
                        crate::settlements::presentation::summary(state, record),
                        record.name,
                        record.owner.as_deref().unwrap_or("autonomous"),
                        record.anchor,
                        record.radius,
                        members,
                        record.revision,
                    );
                    error(state, connection, message);
                }
            }
        }
        SettlementCommand::Rename { name, new_name } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let Some(record) = lookup(state, name) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !record.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner or an operator renames a settlement.",
                );
                return;
            }
            let id = record.id;
            match state.settlements.rename(id, new_name.clone()) {
                Ok(()) => error(state, connection, format!("Renamed {name} to {new_name}.")),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Remove { name } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let Some(record) = lookup(state, name) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !record.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner or an operator removes a settlement.",
                );
                return;
            }
            let id = record.id;
            if crate::settlements::remove(state, id) {
                error(state, connection, format!("Removed {name}."));
            } else {
                error(
                    state,
                    connection,
                    "Settlement removal refused: construction could not be cancelled.",
                );
            }
        }
        SettlementCommand::Relocate {
            settlement,
            household,
            destination,
        } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let (Some(source), Some(target)) =
                (lookup(state, settlement), lookup(state, destination))
            else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !source.can_manage(&actor, operator) || !target.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner of both settlements or an operator relocates a household.",
                );
                return;
            }
            let (source, target) = (source.id, target.id);
            match crate::settlements::households::relocate(state, source, household, target) {
                Ok(()) => error(
                    state,
                    connection,
                    format!(
                        "Relocated {household} to {destination}; native housing admission will reacquire reachable claims."
                    ),
                ),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Bind {
            settlement,
            villager,
        } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let Some(record) = lookup(state, settlement) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !record.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner or an operator binds residents.",
                );
                return;
            }
            let id = record.id;
            let (entity, uuid) = match settlement_resident(state, executor, villager.as_deref()) {
                Ok(resident) => resident,
                Err(message) => {
                    error(state, connection, message);
                    return;
                }
            };
            match crate::settlements::bind(state, id, entity) {
                Ok(()) => error(state, connection, format!("Bound {uuid} to {settlement}.")),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Unbind { villager } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let (entity, uuid) = match settlement_resident(state, executor, villager.as_deref()) {
                Ok(resident) => resident,
                Err(message) => {
                    error(state, connection, message);
                    return;
                }
            };
            let slot = state.entities.slot_of(entity).expect("resolved resident");
            let Some(villager) = state.entities.row(slot).villager.as_ref() else {
                error(state, connection, "Villager not found.");
                return;
            };
            let id = villager.settlement;
            let Some(id) = id else {
                error(state, connection, "Villager belongs to no settlement.");
                return;
            };
            let Some(record) = state.settlements.get(id) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !record.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner or an operator unbinds residents.",
                );
                return;
            }
            match crate::settlements::unbind(state, entity) {
                Ok(()) => error(state, connection, format!("Unbound {uuid}.")),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Economy {
            domain,
            settlement,
            arguments,
        } => {
            let Some(record) = lookup(state, settlement) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            let status = arguments.len() == 1 && arguments[0] == "status";
            if !status
                && actor
                    .as_ref()
                    .is_none_or(|actor| !record.can_manage(actor, operator))
            {
                error(
                    state,
                    connection,
                    "Only the owner or an operator changes settlement economics.",
                );
                return;
            }
            let id = record.id;
            match crate::settlements::economy_commands::execute(state, id, *domain, arguments) {
                Ok(message) => error(state, connection, message),
                Err(failure) => error(
                    state,
                    connection,
                    format!("Native settlement action refused: {failure:?}."),
                ),
            }
        }
        SettlementCommand::BuildingCertify { kind, min, max } => {
            match crate::settlements::building_commands::certify(
                state,
                executor,
                actor.as_deref(),
                operator,
                kind,
                *min,
                *max,
            ) {
                Ok(message) => error(state, connection, message),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::BuildingUpgrade { id } => {
            match crate::settlements::building_commands::upgrade(
                state,
                actor.as_deref(),
                operator,
                *id,
            ) {
                Ok(message) => error(state, connection, message),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::BuildingList => {
            match crate::settlements::building_commands::list(state, executor) {
                Ok(message) => error(state, connection, message),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Build { blueprint } => {
            build_command(
                state,
                connection,
                executor,
                actor.as_deref(),
                operator,
                blueprint,
            );
        }
        SettlementCommand::BuildUpgrade { id } => {
            build_upgrade_command(state, connection, executor, actor.as_deref(), operator, *id);
        }
        SettlementCommand::Army { command } => {
            army_command(state, connection, executor, command);
        }
    }
}

/// Native `/settlement build ...` execution. The target is always the
/// executor's enclosing settlement (closest anchor within its radius, like
/// building/army commands). Both variants need an authenticated actor and
/// `can_manage`, which also refuses ownerless settlements; operators of an
/// owned settlement pass per existing semantics. Orders start immediately via
/// `construction::request_build/request_upgrade` (no stock debit or placement
/// here); the reply reports the started order or the real refusal (`Busy.`,
/// missing materials, no free site).
fn build_command(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    actor: Option<&str>,
    operator: bool,
    blueprint: &str,
) {
    use crate::settlements::building_commands;
    let Some(id) = building_commands::containing(state, executor) else {
        return error(state, connection, building_commands::NO_SETTLEMENT);
    };
    let Some(actor) = actor else {
        return error(state, connection, building_commands::ADMISSION_UNAVAILABLE);
    };
    if state
        .settlements
        .get(id)
        .is_none_or(|record| !record.can_manage(actor, operator))
    {
        return error(
            state,
            connection,
            "Only the owner or an operator orders construction.",
        );
    }
    match crate::settlements::construction::request_build(state, id, blueprint) {
        Ok(()) => error(
            state,
            connection,
            format!("Started {blueprint} construction."),
        ),
        Err(message) => error(state, connection, message),
    }
}

/// Native `/settlement build upgrade <buildingId>` execution. The building id
/// is global (`settlement << 32 | sequence`, like `/settlement building
/// upgrade`), but authorization runs against the executor's enclosing
/// settlement: the target must belong to it. Starts the funded upgrade
/// immediately (no stock debit or placement here); refusals surface directly.
/// Kept separate from the geometric `/settlement building upgrade` path.
fn build_upgrade_command(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    actor: Option<&str>,
    operator: bool,
    id: u64,
) {
    use crate::settlements::building_commands;
    use crate::settlements::buildings::BuildingId;
    let Some(enclosing) = building_commands::containing(state, executor) else {
        return error(state, connection, building_commands::NO_SETTLEMENT);
    };
    let Some(actor) = actor else {
        return error(state, connection, building_commands::ADMISSION_UNAVAILABLE);
    };
    if state
        .settlements
        .get(enclosing)
        .is_none_or(|record| !record.can_manage(actor, operator))
    {
        return error(
            state,
            connection,
            "Only the owner or an operator orders construction.",
        );
    }
    let building = BuildingId(id);
    let home = crate::settlements::SettlementId(id >> 32);
    if home != enclosing {
        return error(state, connection, "Building not found.");
    }
    match crate::settlements::construction::request_upgrade(state, enclosing, building) {
        Ok(()) => error(
            state,
            connection,
            format!("Started upgrade for building #{id}."),
        ),
        Err(message) => error(state, connection, message),
    }
}
/// Native `/settlement army ...` execution. The target is always the
/// executor's enclosing settlement (closest anchor within its radius, like
/// building commands). Reads are open; every mutation re-checks
/// `can_manage`, which also refuses ownerless settlements. Follow uses the
/// executor's entity UUID; guard uses the executor's current block position.
fn army_command(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &crate::settlements::commands::ArmyCommand,
) {
    use crate::settlements::commands::{ArmyCommand, ArmyOrderKind};
    use crate::settlements::{army, building_commands};
    let Some(id) = building_commands::containing(state, executor) else {
        return error(state, connection, building_commands::NO_SETTLEMENT);
    };
    if !matches!(command, ArmyCommand::List) {
        let operator = state.plugin_players.is_operator(connection)
            || connection == crate::command_console::CONNECTION;
        let Some(actor) = actor_identity(state, connection) else {
            return error(state, connection, building_commands::ADMISSION_UNAVAILABLE);
        };
        if state
            .settlements
            .get(id)
            .is_none_or(|record| !record.can_manage(&actor, operator))
        {
            return error(
                state,
                connection,
                "Only the owner or an operator commands soldiers.",
            );
        }
    }
    match command {
        ArmyCommand::List => {
            let rows = army::list(state, id);
            let message = if rows.is_empty() {
                "No soldiers.".to_owned()
            } else {
                rows.join("\n")
            };
            error(state, connection, message);
        }
        ArmyCommand::Recruit => {
            let result = army::recruit(state, id).map(|uuid| format!("Recruited {uuid}."));
            error(
                state,
                connection,
                result.unwrap_or_else(std::convert::identity),
            );
        }
        ArmyCommand::Dismiss { soldier } => {
            let Ok(uuid) = soldier.parse::<uuid::Uuid>() else {
                return error(state, connection, "Soldier must be a native entity UUID.");
            };
            let result = army::dismiss(state, id, uuid).map(|()| format!("Dismissed {uuid}."));
            error(
                state,
                connection,
                result.unwrap_or_else(std::convert::identity),
            );
        }
        ArmyCommand::Order { target, order } => {
            let uuid = if target == "all" {
                None
            } else {
                let Ok(uuid) = target.parse::<uuid::Uuid>() else {
                    return error(
                        state,
                        connection,
                        "Soldier must be a native entity UUID or all.",
                    );
                };
                Some(uuid)
            };
            let order = match order {
                ArmyOrderKind::Follow | ArmyOrderKind::Guard => {
                    let Some(slot) = state.entities.slot_of(executor) else {
                        return error(state, connection, "Your position is unavailable.");
                    };
                    if *order == ArmyOrderKind::Follow {
                        army::Order::Follow(state.entities.row(slot).uuid)
                    } else {
                        army::Order::Guard(
                            state.entities.pos(slot).map(|value| value.floor() as i32),
                        )
                    }
                }
                ArmyOrderKind::Patrol => army::Order::Patrol,
                ArmyOrderKind::Return => army::Order::Return,
            };
            let result = army::order(state, id, uuid, order)
                .map(|count| format!("Ordered {count} soldier(s)."));
            error(
                state,
                connection,
                result.unwrap_or_else(std::convert::identity),
            );
        }
    }
}

/// Explicit UUIDs retain the native target path. Without one, pick the nearest
/// living resident in this dimension, within eight blocks of the player.
fn settlement_resident(
    state: &TickState,
    executor: EntityId,
    target: Option<&str>,
) -> Result<(EntityId, uuid::Uuid), &'static str> {
    if let Some(target) = target {
        let uuid = target
            .parse::<uuid::Uuid>()
            .map_err(|_| "Villager must be a native entity UUID.")?;
        let slot = state
            .entities
            .slot_of_uuid(uuid)
            .filter(|&slot| state.entities.row(slot).uuid == uuid)
            .ok_or("Villager not found.")?;
        return Ok((state.entities.body(slot).id, uuid));
    }
    let origin = state
        .entities
        .slot_of(executor)
        .map(|slot| state.entities.pos(slot))
        .ok_or("Stand within 8 blocks of a villager, or supply its native UUID.")?;
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            let villager = row.villager.as_ref()?;
            if !body.alive || villager.wandering || villager.conversion_time.is_some() {
                return None;
            }
            let distance: f64 = state
                .entities
                .pos(slot)
                .iter()
                .zip(origin)
                .map(|(at, origin)| (at - origin).powi(2))
                .sum();
            (distance <= 64.0).then_some((distance, body.id, row.uuid))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|(_, entity, uuid)| (entity, uuid))
        .ok_or("No living villager within 8 blocks. Stand next to one, or supply its native UUID.")
}

/// Command actor identity: the connection UUID string, never the display
/// name; the console acts as `"Server"`. Absent means no authenticated actor:
/// mutations and creates refuse rather than inventing a shared owner.
fn actor_identity(state: &TickState, connection: ConnectionId) -> Option<String> {
    if connection == crate::command_console::CONNECTION {
        return Some("Server".to_owned());
    }
    state
        .connections
        .uuid_of(connection)
        .map(|uuid| uuid.to_string())
}

/// Online players a player command addresses; reports "no player found"
/// and yields `None` when there are none.
fn command_players(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    target: &Target,
) -> Option<Vec<EntityId>> {
    let targets = player_targets(state, executor, target);
    if targets.is_empty() {
        feedback(
            state,
            connection,
            "argument.entity.notfound.player",
            Vec::new(),
        );
        return None;
    }
    Some(targets)
}

/// Run a player command's state change on one target and sync its client.
fn apply_player_command(
    state: &mut TickState,
    target: EntityId,
    command: &Command,
    changes: &mut ChangeSet,
) {
    let effects = state
        .players
        .get_mut(&target)
        .expect("resolved player exists")
        .apply_command(&state.registry.item_rules(), command);
    if let Some(target_connection) = state.connections.entity_connection(target) {
        crate::tick::apply_player_effects(state, target_connection, target, effects, changes);
    }
}

fn success_suffix(targets: &[EntityId]) -> &'static str {
    if targets.len() == 1 {
        "single"
    } else {
        "multiple"
    }
}

fn experience_success(
    state: &mut TickState,
    connection: ConnectionId,
    verb: &str,
    amount: i32,
    levels: bool,
    targets: &[EntityId],
) {
    let unit = if levels { "levels" } else { "points" };
    let key = format!(
        "commands.experience.{verb}.{unit}.success.{}",
        success_suffix(targets)
    );
    let label = targets_label(state, targets);
    feedback(state, connection, &key, vec![text(amount), label]);
}

fn game_mode(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    targets: &[EntityId],
    mode: crate::commands::GameMode,
    command: &Command,
    changes: &mut ChangeSet,
) {
    for &target in targets {
        if state.players[&target].game_mode == mode {
            continue;
        }
        apply_player_command(state, target, command, changes);
        if target == executor {
            feedback(
                state,
                connection,
                "commands.gamemode.success.self",
                vec![mode_name(mode)],
            );
            continue;
        }
        let name = entity_name(state, target);
        feedback(
            state,
            connection,
            "commands.gamemode.success.other",
            vec![name, mode_name(mode)],
        );
        if crate::command_rules::boolean(state, "send_command_feedback")
            && let Some(target_connection) = state.connections.entity_connection(target)
        {
            feedback(
                state,
                target_connection,
                "gameMode.changed",
                vec![mode_name(mode)],
            );
        }
    }
}

fn give(
    state: &mut TickState,
    connection: ConnectionId,
    targets: &[EntityId],
    item: &crate::command_items::ItemArgument,
    count: i32,
    command: &Command,
    changes: &mut ChangeSet,
) {
    let stack = match item.stack(&state.registry.items) {
        Ok(stack) => stack,
        Err(reason) => {
            error(state, connection, reason);
            return;
        }
    };
    let maximum = state
        .registry
        .item_rules()
        .max_stack(&stack)
        .saturating_mul(100);
    if count > maximum {
        let name = item_name(state, &item.item);
        feedback(
            state,
            connection,
            "commands.give.failed.toomanyitems",
            vec![text(maximum), name],
        );
        return;
    }
    for &target in targets {
        apply_player_command(state, target, command, changes);
    }
    let key = format!("commands.give.success.{}", success_suffix(targets));
    let args = vec![
        text(count),
        item_name(state, &item.item),
        targets_label(state, targets),
    ];
    feedback(state, connection, &key, args);
}

fn clear(
    state: &mut TickState,
    connection: ConnectionId,
    targets: &[EntityId],
    item: Option<&crate::command_items::ItemPredicate>,
    max: Option<i32>,
    command: &Command,
    changes: &mut ChangeSet,
) {
    let rules = state.registry.item_rules();
    if let Some(item) = item
        && let Err(reason) = item.validate(&rules)
    {
        error(state, connection, reason);
        return;
    }
    // `max == Some(0)` only counts (`/clear ... 0`); a real clear must not
    // debit an inventory an active native operation still reserves.
    if max != Some(0) {
        for &target in targets {
            if !crate::plugin_inventories::has_reservations_for_player(state, target) {
                continue;
            }
            // The native clear kernel walks slots in order. Only its
            // ordinary 0..35 source debits affect addressable holds.
            let mut debits = [(0_usize, 0_u32); 36];
            let mut len = 0;
            let mut budget = max.unwrap_or(i32::MAX);
            for (slot, stack) in state.players[&target]
                .inventory
                .slots
                .iter()
                .take(36)
                .enumerate()
            {
                if stack.is_empty() || item.is_some_and(|item| !item.matches(&rules, stack)) {
                    continue;
                }
                let taken = stack.count.min(budget);
                debits[len] = (slot, taken as u32);
                len += 1;
                budget -= taken;
                if budget == 0 {
                    break;
                }
            }
            if crate::plugin_inventories::below_player_debit_floors(
                state,
                target,
                &debits[..len],
                None,
            ) {
                error(
                    state,
                    connection,
                    "Inventory is reserved by an active native operation",
                );
                return;
            }
        }
    }
    let mut cleared = 0_i32;
    for &target in targets {
        let rules = state.registry.item_rules();
        let matching: i32 = state.players[&target]
            .inventory
            .slots
            .iter()
            .filter(|stack| {
                !stack.is_empty() && item.is_none_or(|item| item.matches(&rules, stack))
            })
            .map(|stack| stack.count)
            .sum();
        cleared =
            cleared.saturating_add(matching.min(max.filter(|max| *max > 0).unwrap_or(i32::MAX)));
        apply_player_command(state, target, command, changes);
    }
    let suffix = success_suffix(targets);
    let label = targets_label(state, targets);
    if cleared == 0 {
        feedback(
            state,
            connection,
            &format!("clear.failed.{suffix}"),
            vec![label],
        );
    } else {
        let mode = if max == Some(0) { "test" } else { "success" };
        feedback(
            state,
            connection,
            &format!("commands.clear.{mode}.{suffix}"),
            vec![text(cleared), label],
        );
    }
}

fn expand_message(state: &TickState, executor: EntityId, message: &str) -> String {
    let mut expanded = String::with_capacity(message.len());
    let mut offset = 0;
    while let Some(relative) = message[offset..].find('@') {
        let start = offset + relative;
        expanded.push_str(&message[offset..start]);
        let rest = &message[start..];
        if !matches!(
            rest.as_bytes().get(1),
            Some(b'p' | b'a' | b'r' | b's' | b'e')
        ) {
            expanded.push('@');
            offset = start + 1;
            continue;
        }
        let end = if rest.as_bytes().get(2) == Some(&b'[') {
            rest.find(']').map(|end| end + 1).unwrap_or(2)
        } else {
            2.min(rest.len())
        };
        let candidate = &rest[..end];
        if let Ok(target) = crate::commands::target_of(candidate) {
            let names = resolve_target(state, executor, &target)
                .into_iter()
                .filter_map(|id| state.connections.name_of(id))
                .collect::<Vec<_>>()
                .join(", ");
            expanded.push_str(&names);
        } else {
            expanded.push_str(candidate);
        }
        offset = start + end;
    }
    expanded.push_str(&message[offset..]);
    expanded
}

#[cfg(test)]
#[path = "command_execution_tests.rs"]
mod tests;
