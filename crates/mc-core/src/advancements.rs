//! Vanilla advancement progress; the tick owns awards and the player WAL owns durability.
#[path = "advancements_location.rs"]
mod location;
use mc_physics::mth;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mc_data::advancements::{
    AdvancementDefinition, AdvancementTree, EventContext, evaluate_criterion,
};
use serde_json::{Map, Value, json};

use crate::boundary::{ChangeSet, EntityId, PlayerChange};
use crate::entities::EntityKind;
use crate::tick::TickState;

#[derive(Debug)]
pub(crate) struct Event {
    pub(crate) entity: EntityId,
    pub(crate) trigger: String,
    pub(crate) evidence: Value,
}

/// Queue only an observed, accepted gameplay event. Capturing here preserves
/// the predicates' source position even if the actor moves later in this tick.
pub fn trigger(state: &mut TickState, entity: EntityId, trigger: &str, mut evidence: Value) {
    if state.registry.advancements.definitions.is_empty() || !state.players.contains_key(&entity) {
        return;
    }
    if !state.registry.advancements.trigger_index.is_empty()
        && !state
            .registry
            .advancements
            .trigger_index
            .contains_key(trigger)
    {
        return;
    }
    if state
        .registry
        .advancements
        .trigger_index
        .get(trigger)
        .is_some_and(|criteria| {
            criteria.iter().all(|(id, criterion)| {
                state.players[&entity]
                    .advancement_progress
                    .get(id)
                    .and_then(|entry| entry.get("criteria"))
                    .and_then(Value::as_object)
                    .is_some_and(|awarded| awarded.contains_key(criterion))
            })
        })
    {
        return;
    }
    let Some(fields) = evidence.as_object_mut() else {
        return;
    };
    fields
        .entry("player")
        .or_insert_with(|| entity_snapshot(state, entity));
    fields
        .entry("location")
        .or_insert_with(|| location_snapshot(state, entity));
    if let Some(slot) = state.entities.slot_of_any(entity) {
        add_relative_distances(state.entities.pos(slot), &mut evidence);
    }
    state.advancement_events.push(Event {
        entity,
        trigger: trigger.to_owned(),
        evidence,
    });
}

fn add_relative_distances(origin: [f64; 3], evidence: &mut Value) {
    match evidence {
        Value::Object(fields) => {
            if fields.contains_key("id") {
                return; // Item snapshots have no entity-relative predicates.
            }
            if fields.get("type").is_some_and(Value::is_string)
                && let Some(position) = fields
                    .get("location")
                    .and_then(|location| location.get("position"))
                && let [Some(x), Some(y), Some(z)] =
                    ["x", "y", "z"].map(|axis| position.get(axis).and_then(Value::as_f64))
            {
                let delta = [x - origin[0], y - origin[1], z - origin[2]];
                fields.insert(
                    "distance".into(),
                    json!({
                        "x":delta[0].abs(),"y":delta[1].abs(),"z":delta[2].abs(),
                        "horizontal":mth::length2(delta[0], delta[2]),
                        "absolute":mth::length3(delta[0], delta[1], delta[2])
                    }),
                );
            }
            for child in fields.values_mut() {
                add_relative_distances(origin, child);
            }
        }
        Value::Array(entries) => {
            for child in entries {
                add_relative_distances(origin, child);
            }
        }
        _ => {}
    }
}

pub fn location_snapshot(state: &TickState, entity: EntityId) -> Value {
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return Value::Null;
    };
    let position = state.entities.pos(slot);
    let mut location = json!({"dimension": state.dimension.name(), "position": {
        "x":position[0], "y":position[1], "z":position[2]
    }});
    if let Some(biome) = state.world.biome_at(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    ) {
        location["biomes"] = json!([biome.as_str()]);
    }
    if let Some(block) = state
        .world
        .block_at(
            position[0].floor() as i32,
            position[1].floor() as i32,
            position[2].floor() as i32,
        )
        .and_then(|id| block_metadata(state, id))
    {
        location["block"] = block.clone();
    }
    location
}

fn block_metadata(state: &TickState, id: u32) -> Option<&Value> {
    let block = state.registry.blocks.by_id(mc_world::BlockStateId(id))?;
    let cache = state.registry.advancement_blocks.get(id as usize)?;
    Some(cache.get_or_init(|| {
        let properties: Map<_, _> = block
            .properties
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect();
        let tags: Vec<_> = state
            .registry
            .tags
            .registries
            .iter()
            .filter(|(registry, _)| registry.as_str() == "minecraft:block")
            .flat_map(|(_, tags)| tags.iter())
            .filter(|(_, entries)| entries.contains(&block.block.raw_id))
            .map(|(tag, _)| tag.as_str())
            .collect();
        json!({"id":block.block.id.as_str(),"tags":tags,"state":properties})
    }))
}

pub fn item_snapshot(state: &TickState, stack: &mc_data::ItemStack) -> Value {
    if stack.is_empty() {
        return json!({"id":"minecraft:air","count":0,"tags":[],"components":{}});
    }
    let Some(id) = state.registry.items.name_of(stack.item_id) else {
        return Value::Null;
    };
    let tags: Vec<_> = state
        .registry
        .tags
        .registries
        .iter()
        .filter(|(registry, _)| registry.as_str() == "minecraft:item")
        .flat_map(|(_, tags)| tags.iter())
        .filter(|(_, entries)| entries.contains(&(stack.item_id as i32)))
        .map(|(tag, _)| tag.as_str())
        .collect();
    let mut components = Map::new();
    if let Some(damage) = stack.damage {
        components.insert("minecraft:damage".into(), json!(damage));
    }
    let enchantments: Map<_, _> = stack
        .enchantments
        .iter()
        .map(|enchantment| (enchantment.id.as_str().to_owned(), json!(enchantment.level)))
        .collect();
    components.insert("minecraft:enchantments".into(), Value::Object(enchantments));
    if let Some(potion) = &stack.extended().potion {
        components.insert(
            "minecraft:potion_contents".into(),
            json!({"potion":potion.as_str()}),
        );
    }
    if let Some((song, _, _)) = crate::jukebox::song(&state.registry.items, stack) {
        components.insert(
            "minecraft:jukebox_playable".into(),
            json!({"song":format!("minecraft:{song}")}),
        );
    }
    json!({"id":id.as_str(),"count":stack.count,"tags":tags,"components":components})
}

fn entity_type_tags(state: &TickState, type_id: u32) -> Vec<&str> {
    state
        .registry
        .tags
        .registries
        .iter()
        .filter(|(registry, _)| registry.as_str() == "minecraft:entity_type")
        .flat_map(|(_, tags)| tags.iter())
        .filter(|(_, entries)| entries.contains(&(type_id as i32)))
        .map(|(tag, _)| tag.as_str())
        .collect()
}

pub fn entity_snapshot(state: &TickState, entity: EntityId) -> Value {
    entity_snapshot_inner(state, entity, true)
}

fn entity_snapshot_inner(state: &TickState, entity: EntityId, relations: bool) -> Value {
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return Value::Null;
    };
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let entity_type = match body.kind {
        EntityKind::Player { .. } => "minecraft:player",
        EntityKind::Item { .. } => "minecraft:item",
        EntityKind::ExperienceOrb { .. } => "minecraft:experience_orb",
        EntityKind::Mob { type_id } => state
            .registry
            .blueprints
            .iter()
            .find(|(id, _)| *id == type_id)
            .map_or("", |(_, blueprint)| blueprint.name.as_str()),
    };
    let tags = mc_data::entity_types::entity_type_contract_26_1_2_by_name(entity_type)
        .map_or_else(Vec::new, |contract| {
            entity_type_tags(state, contract.protocol_id)
        });
    let mut equipment = Map::new();
    for (name, index) in [
        ("mainhand", 0),
        ("offhand", 1),
        ("feet", 2),
        ("legs", 3),
        ("chest", 4),
        ("head", 5),
    ] {
        equipment.insert(
            name.into(),
            item_snapshot(
                state,
                row.appearance.equipment[index]
                    .as_ref()
                    .unwrap_or(&mc_data::ItemStack::EMPTY),
            ),
        );
    }
    if let Some(player) = state.players.get(&entity) {
        for (name, index) in [
            ("mainhand", usize::from(player.inventory.selected.min(8))),
            ("offhand", 40),
            ("feet", 36),
            ("legs", 37),
            ("chest", 38),
            ("head", 39),
        ] {
            equipment.insert(
                name.into(),
                item_snapshot(state, &player.inventory.slots[index]),
            );
        }
    }
    let effects = state
        .players
        .get(&entity)
        .map_or(&row.active_effects, |player| &player.active_effects);
    let effect_values: Map<_, _> = effects
        .entries
        .iter()
        .map(|effect| {
            (
                format!("minecraft:{}", effect.id.name()),
                json!({"amplifier":effect.amplifier,"duration":effect.duration}),
            )
        })
        .collect();
    // Entity.getOnPos uses the block just below the feet, including at
    // integral heights where the entity's own location is in the air above.
    let position = state.entities.pos(slot);
    let stepping_on = block_location(
        state,
        crate::boundary::BlockPos {
            x: position[0].floor() as i32,
            y: (position[1] - 1.0e-5).floor() as i32,
            z: position[2].floor() as i32,
        },
        None,
    );
    let mut snapshot = json!({"type":entity_type,"tags":tags,"location":location_snapshot(state,entity),
        "flags":{"is_baby":row.appearance.baby,"is_on_fire":body.burn_ticks>0,"is_on_ground":state.entities.ground(slot),"is_sneaking":row.appearance.pose==5,"is_sprinting":state.players.get(&entity).is_some_and(|player| player.sprinting)},
        "equipment":equipment,"effects":effect_values,"stepping_on":stepping_on});
    if relations {
        let passengers = |slot| {
            state
                .entities
                .row(slot)
                .appearance
                .passengers
                .iter()
                .map(|passenger| entity_snapshot_inner(state, *passenger, false))
                .collect::<Vec<_>>()
        };
        snapshot["passenger"] = json!(passengers(slot));
        if let Some(vehicle) = crate::mounts::vehicle(state, entity) {
            let mut evidence = entity_snapshot_inner(state, state.entities.body(vehicle).id, false);
            evidence["passenger"] = json!(passengers(vehicle));
            snapshot["vehicle"] = evidence;
        }
    }
    snapshot
}

pub fn sync_join(state: &mut TickState, entity: EntityId) {
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    // Vanilla saves the recipe book separately. Imported progress without a
    // book can still recover recipe rewards without granting XP a second time.
    for (id, definition) in &state.registry.advancements.definitions {
        if complete(&player.advancement_progress, id, definition) {
            for recipe in &definition.rewards.recipes {
                if !player.unlocked_recipes.contains(recipe) {
                    Arc::make_mut(&mut player.unlocked_recipes).insert(recipe.clone());
                }
            }
        }
    }
    let connection = player.connection;
    let progress = player.advancement_progress.as_ref().clone();
    let recipes = player.unlocked_recipes.iter().cloned().collect();
    state.push_player(
        connection,
        PlayerChange::Advancements {
            reset: true,
            definitions: state.registry.advancements.clone(),
            progress,
        },
    );
    state.push_player(
        connection,
        PlayerChange::RecipesUnlocked {
            recipes,
            replace: true,
        },
    );
}

pub fn seen(state: &mut TickState, entity: EntityId, tab: Option<&str>) {
    let Some(tab) = tab else { return };
    let Some(definition) = state.registry.advancements.definitions.get(tab) else {
        return;
    };
    if definition.parent.is_some() || definition.display.is_none() {
        return;
    }
    let Some(player) = state.players.get(&entity) else {
        return;
    };
    let connection = player.connection;
    state.push_player(
        connection,
        PlayerChange::AdvancementsTab {
            tab: Some(tab.to_owned()),
        },
    );
}

pub(crate) fn run(state: &mut TickState, changes: &mut ChangeSet) {
    if state.registry.advancements.definitions.is_empty() {
        return;
    }
    let players: Vec<_> = state.players.keys().copied().collect();
    let mut selected = Vec::new();
    for entity in players {
        let inventory_changed = state.players.get_mut(&entity).is_some_and(|player| {
            if player.advancement_inventory == player.inventory.slots {
                return false;
            }
            player
                .advancement_inventory
                .clone_from(&player.inventory.slots);
            true
        });
        if inventory_changed {
            let inventory: Vec<_> = state.players[&entity]
                .inventory
                .slots
                .iter()
                .filter(|stack| !stack.is_empty())
                .map(|stack| item_snapshot(state, stack))
                .collect();
            let occupied = inventory.len();
            let full = state.players[&entity]
                .inventory
                .slots
                .iter()
                .filter(|stack| {
                    !stack.is_empty() && stack.count >= state.registry.item_rules().max_stack(stack)
                })
                .count();
            trigger(
                state,
                entity,
                "minecraft:inventory_changed",
                json!({"inventory":inventory,"slots":{"occupied":occupied,"full":full,"empty":41-occupied}}),
            );
        }
        trigger(state, entity, "minecraft:tick", json!({}));
        if state.tick.is_multiple_of(20) {
            location::periodic(state, entity, &mut selected);
        }
        enter_block(state, entity);
    }
    let mut events: VecDeque<_> = std::mem::take(&mut state.advancement_events).into();
    while let Some(event) = events.pop_front() {
        let tree = state.registry.advancements.clone();
        let inventory = event
            .evidence
            .get("inventory")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let context = EventContext {
            trigger: &event.trigger,
            event: &event.evidence,
            inventory,
        };
        let candidates;
        let indexed = if tree.trigger_index.is_empty() {
            candidates = tree
                .definitions
                .iter()
                .flat_map(|(id, definition)| {
                    definition
                        .criteria
                        .iter()
                        .filter(|(_, criterion)| criterion.trigger == event.trigger)
                        .map(|(criterion, _)| (id.clone(), criterion.clone()))
                })
                .collect::<Vec<_>>();
            candidates.as_slice()
        } else {
            tree.trigger_index
                .get(&event.trigger)
                .map_or(&[][..], Vec::as_slice)
        };
        if tree.trigger_index.is_empty() {
            selected.clear();
            selected.extend(0..indexed.len());
        } else {
            tree.candidate_indices(&context, &mut selected);
        }
        let matched: Vec<_> = selected
            .iter()
            .map(|&index| &indexed[index])
            .filter(|(id, name)| {
                !state.players.get(&event.entity).is_some_and(|player| {
                    player
                        .advancement_progress
                        .get(id)
                        .and_then(|progress| progress.get("criteria"))
                        .and_then(|criteria| criteria.get(name))
                        .is_some()
                })
            })
            .filter(|(id, name)| {
                evaluate_criterion(&tree.definitions[id].criteria[name], &context).matches
            })
            .cloned()
            .collect();
        for (id, criterion) in matched {
            grant(state, event.entity, &[id], Some(&criterion));
        }
        events.extend(std::mem::take(&mut state.advancement_events));
    }
    changes
        .entities_spawned
        .append(&mut state.advancement_reward_spawns);
}

fn enter_block(state: &mut TickState, entity: EntityId) {
    let tree = &state.registry.advancements;
    let indexed = tree.trigger_index.get("minecraft:enter_block");
    if indexed.is_none() && !tree.trigger_index.is_empty() {
        return;
    }
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return;
    };
    let [x, y, z] = state.entities.pos(slot).map(|axis| axis.floor() as i32);
    let Some(block) = state
        .world
        .block_at(x, y, z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
    else {
        return;
    };
    let progress = &state.players[&entity].advancement_progress;
    let pending = |id: &str, name: &str, criterion: &mc_data::advancements::Criterion| {
        criterion.trigger == "minecraft:enter_block"
            && progress
                .get(id)
                .and_then(|entry| entry.get("criteria"))
                .and_then(|criteria| criteria.get(name))
                .is_none()
            && (criterion.conditions.is_null()
                || criterion.conditions.as_object().is_some_and(|conditions| {
                    conditions
                        .get("block")
                        .is_none_or(|id| id.as_str() == Some(block.block.id.as_str()))
                }))
    };
    let could_match = if let Some(indexed) = indexed {
        indexed
            .iter()
            .any(|(id, name)| pending(id, name, &tree.definitions[id].criteria[name]))
    } else {
        tree.definitions.iter().any(|(id, definition)| {
            definition
                .criteria
                .iter()
                .any(|(name, criterion)| pending(id, name, criterion))
        })
    };
    if !could_match {
        return;
    }
    // Only now snapshot the player/location. Reevaluate every matching tick:
    // equipment and other player predicates can change without leaving a block.
    let event = json!({"block":block.block.id.as_str(),"state":block.properties});
    trigger(state, entity, "minecraft:enter_block", event);
}

fn complete(progress: &Value, id: &str, definition: &AdvancementDefinition) -> bool {
    let Some(criteria) = progress
        .get(id)
        .and_then(|entry| entry.get("criteria"))
        .and_then(Value::as_object)
    else {
        return false;
    };
    !definition.requirements.is_empty()
        && definition.requirements.iter().all(|group| {
            group
                .iter()
                .any(|criterion| criteria.contains_key(criterion))
        })
}

pub fn grant(
    state: &mut TickState,
    entity: EntityId,
    ids: &[String],
    criterion: Option<&str>,
) -> usize {
    mutate(state, entity, ids, criterion, true)
}

pub fn revoke(
    state: &mut TickState,
    entity: EntityId,
    ids: &[String],
    criterion: Option<&str>,
) -> usize {
    mutate(state, entity, ids, criterion, false)
}

fn mutate(
    state: &mut TickState,
    entity: EntityId,
    ids: &[String],
    criterion: Option<&str>,
    grant: bool,
) -> usize {
    let tree = state.registry.advancements.clone();
    let mut changed = Map::new();
    let mut completions = Vec::new();
    let Some(player) = state.players.get_mut(&entity) else {
        return 0;
    };
    let connection = player.connection;
    let Some(progress) = player.advancement_progress.as_object() else {
        return 0;
    };
    let would_change = |id: &String| {
        let Some(definition) = tree.definitions.get(id) else {
            return false;
        };
        let criteria = match progress.get(id) {
            Some(entry) => {
                let Some(fields) = entry.as_object() else {
                    return false;
                };
                match fields.get("criteria") {
                    Some(criteria) => {
                        let Some(criteria) = criteria.as_object() else {
                            return false;
                        };
                        Some(criteria)
                    }
                    None => None,
                }
            }
            None => None,
        };
        definition.criteria.keys().any(|name| {
            criterion.is_none_or(|only| only == name.as_str())
                && (criteria.is_some_and(|criteria| criteria.contains_key(name)) != grant)
        })
    };
    if !ids.iter().any(would_change) {
        return 0;
    }
    let timestamp = timestamp_from_unix((state.wall_clock)());
    let progress = Arc::make_mut(&mut player.advancement_progress)
        .as_object_mut()
        .expect("preflight admitted an advancement object");
    let mut count = 0;
    for id in ids {
        let Some(definition) = tree.definitions.get(id) else {
            continue;
        };
        // Do not create empty records for unchanged ids alongside a real mutation.
        if progress.get(id).is_some_and(|entry| {
            !entry.is_object()
                || entry
                    .get("criteria")
                    .is_some_and(|criteria| !criteria.is_object())
        }) {
            continue;
        }
        let criteria = progress.get(id).and_then(|entry| entry.get("criteria"));
        if !definition.criteria.keys().any(|name| {
            criterion.is_none_or(|only| only == name.as_str())
                && (criteria.is_some_and(|criteria| criteria.get(name).is_some()) != grant)
        }) {
            continue;
        }
        let entry = progress
            .entry(id.clone())
            .or_insert_with(|| json!({"criteria":{},"done":false}));
        let Some(fields) = entry.as_object_mut() else {
            continue;
        };
        let was_done = fields.get("done").and_then(Value::as_bool).unwrap_or(false);
        let criteria = fields.entry("criteria").or_insert_with(|| json!({}));
        let Some(criteria) = criteria.as_object_mut() else {
            continue;
        };
        let mut modified = false;
        for name in definition
            .criteria
            .keys()
            .filter(|name| criterion.is_none_or(|only| only == name.as_str()))
        {
            if grant && !criteria.contains_key(name) {
                criteria.insert(name.clone(), Value::String(timestamp.clone()));
                modified = true;
            } else if !grant && criteria.remove(name).is_some() {
                modified = true;
            }
        }
        if !modified {
            continue;
        }
        let done = !definition.requirements.is_empty()
            && definition
                .requirements
                .iter()
                .all(|group| group.iter().any(|name| criteria.contains_key(name)));
        fields.insert("done".into(), json!(done));
        if done && !was_done {
            completions.push(id.clone());
        }
        changed.insert(id.clone(), entry.clone());
        count += 1;
    }
    if count > 0 {
        progress
            .entry("DataVersion")
            .or_insert_with(|| json!(mc_world::anvil::TARGET_DATA_VERSION));
        state.push_player(
            connection,
            PlayerChange::Advancements {
                reset: false,
                definitions: Arc::new(AdvancementTree::default()),
                progress: Value::Object(changed),
            },
        );
    }
    for id in completions {
        reward(state, entity, &id, &tree.definitions[&id]);
    }
    count
}

fn reward(state: &mut TickState, entity: EntityId, id: &str, definition: &AdvancementDefinition) {
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    let connection = player.connection;
    let recipes: Vec<_> = definition
        .rewards
        .recipes
        .iter()
        .filter(|recipe| !player.unlocked_recipes.contains(*recipe))
        .cloned()
        .collect();
    let mut recipes = recipes;
    if !recipes.is_empty() {
        let book = Arc::make_mut(&mut player.unlocked_recipes);
        recipes.retain(|recipe| book.insert(recipe.clone()));
    }
    if definition.rewards.experience != 0 {
        player
            .vitals
            .experience
            .add_points(definition.rewards.experience);
        let change = player.experience_change();
        state.push_player(connection, change);
    }
    if !recipes.is_empty() {
        state.push_player(
            connection,
            PlayerChange::RecipesUnlocked {
                recipes: recipes.clone(),
                replace: false,
            },
        );
        for recipe in recipes {
            trigger(
                state,
                entity,
                "minecraft:recipe_unlocked",
                json!({"recipe":recipe}),
            );
        }
    }
    if !definition.rewards.loot.is_empty() {
        let registry = state.registry.clone();
        let mut rng = mc_data::loot::chest_26_1_2::ChestRng::new(
            id.bytes().fold(state.tick ^ entity.0 as u64, |seed, byte| {
                seed.wrapping_mul(31).wrapping_add(u64::from(byte))
            }),
        );
        for table in &definition.rewards.loot {
            let Some(table) = registry.advancement_reward_loot.get(table) else {
                continue;
            };
            let stacks = table
                .roll(&mut rng, &registry.items, 0.0)
                .expect("startup-admitted reward table");
            let mut leftovers = Vec::new();
            if let Some(player) = state.players.get_mut(&entity) {
                for mut stack in stacks {
                    player.inventory.add(&registry.item_rules(), &mut stack);
                    if !stack.is_empty() {
                        leftovers.push(stack);
                    }
                }
                let contents = player.inventory_contents();
                state.push_player(connection, contents);
            }
            if let Some(slot) = state.entities.slot_of(entity) {
                let position = state.entities.pos(slot);
                for stack in leftovers {
                    let spawned = crate::tick::spawn_item_entity(state, position, stack);
                    state.advancement_reward_spawns.push(spawned);
                }
            }
        }
    }
    if definition.display.as_ref().is_some_and(|display| {
        display
            .get("announce_to_chat")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }) && crate::command_rules::boolean(state, "show_advancement_messages")
    {
        let title = definition
            .display
            .as_ref()
            .and_then(|display| display.get("title"))
            .cloned()
            .unwrap_or_else(|| json!(id));
        let frame = definition
            .display
            .as_ref()
            .and_then(|display| display.get("frame"))
            .and_then(Value::as_str)
            .unwrap_or("task");
        let name = state
            .connections
            .name_of(entity)
            .unwrap_or("Player")
            .to_owned();
        let text = json!({"translate":format!("chat.type.advancement.{frame}"),"with":[{"text":name},{"text":"[","extra":[title,{"text":"]"}]}]});
        let content = crate::scoreboard::component(&text.to_string());
        let recipients: Vec<_> = state.connections.all_connections().collect();
        for recipient in recipients {
            state.push_player(
                recipient,
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
}

/// Vanilla's criterion timestamps use UTC date strings in the JSON save and
/// signed epoch milliseconds in AdvancementProgress's stream codec.
pub fn timestamp_to_millis(timestamp: &str) -> Option<i64> {
    if timestamp.len() != 25 {
        return None;
    }
    let b = timestamp.as_bytes();
    if b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b' '
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b' '
    {
        return None;
    }
    let year: i64 = timestamp.get(0..4)?.parse().ok()?;
    let month: i64 = timestamp.get(5..7)?.parse().ok()?;
    let day: i64 = timestamp.get(8..10)?.parse().ok()?;
    let hour: i64 = timestamp.get(11..13)?.parse().ok()?;
    let minute: i64 = timestamp.get(14..16)?.parse().ok()?;
    let second: i64 = timestamp.get(17..19)?.parse().ok()?;
    let offset_hour: i64 = timestamp.get(21..23)?.parse().ok()?;
    let offset_minute: i64 = timestamp.get(23..25)?.parse().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&month)
        || day < 1
        || day > days[(month - 1) as usize]
        || hour > 23
        || minute > 59
        || second > 59
        || offset_hour > 23
        || offset_minute > 59
    {
        return None;
    }
    let offset_sign = match b[20] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let adjusted = year - i64::from(month <= 2);
    let era = adjusted.div_euclid(400);
    let yoe = adjusted - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Some(
        (days * 86400 + hour * 3600 + minute * 60 + second
            - offset_sign * (offset_hour * 3600 + offset_minute * 60))
            * 1000,
    )
}

/// Real UTC seconds: the production wall clock behind `TickState::wall_clock`.
/// Advancement obtained-stamps are real calendar data, so the clock is an
/// injectable external input; deterministic runs install a fixed clock.
pub(crate) fn system_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Vanilla-format criterion stamp for unix `seconds`.
fn timestamp_from_unix(seconds: i64) -> String {
    let days = seconds.div_euclid(86400) + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    let time = seconds.rem_euclid(86400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} +0000",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vanilla_timestamp_codec_preserves_timezone_and_rejects_invalid_dates() {
        assert_eq!(timestamp_to_millis("1970-01-01 00:00:00 +0000"), Some(0));
        assert_eq!(timestamp_to_millis("1970-01-01 01:00:00 +0100"), Some(0));
        assert_eq!(timestamp_to_millis("1969-12-31 23:00:00 -0100"), Some(0));
        assert_eq!(timestamp_to_millis("2026-02-29 00:00:00 +0000"), None);
        assert!(timestamp_to_millis(&timestamp_from_unix(system_unix_seconds())).is_some());
        assert_eq!(timestamp_from_unix(0), "1970-01-01 00:00:00 +0000");
        assert_eq!(
            timestamp_from_unix(1_767_225_600 + 86_461),
            "2026-01-02 00:01:01 +0000"
        );
    }
}

#[cfg(test)]
#[path = "advancements_tests.rs"]
mod gameplay_tests;

pub(crate) fn load_reward_loot(
    root: &std::path::Path,
    tree: &AdvancementTree,
    items: &mc_data::items::ItemRegistry,
) -> Result<
    std::collections::BTreeMap<String, mc_data::loot::chest_26_1_2::ChestLootTable>,
    crate::CoreError,
> {
    use std::io::Read;
    let mut tables = std::collections::BTreeMap::new();
    for name in tree
        .definitions
        .values()
        .flat_map(|definition| &definition.rewards.loot)
    {
        if tables.contains_key(name) {
            continue;
        }
        let load = || -> Result<_, String> {
            let id = mc_data::Identifier::parse(name).map_err(|error| error.to_string())?;
            let mut path = mc_data::ResourcePath::new();
            path.push_static("data")
                .map_err(|error| error.to_string())?;
            path.push_namespace(&id)
                .map_err(|error| error.to_string())?;
            path.push_static("loot_table")
                .map_err(|error| error.to_string())?;
            path.push_identifier_path(&id)
                .map_err(|error| error.to_string())?;
            path.set_extension("json")
                .map_err(|error| error.to_string())?;
            let opened = path
                .open_existing_under(root)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("missing reward loot table {name}"))?;
            let (_, file) = opened.into_parts();
            let mut raw = String::new();
            file.take(mc_data::loot::chest_26_1_2::MAX_CHEST_TABLE_BYTES as u64 + 1)
                .read_to_string(&mut raw)
                .map_err(|error| error.to_string())?;
            let table =
                mc_data::loot::chest_26_1_2::ChestLootTable::compile_advancement_reward(id, &raw)
                    .map_err(|error| error.to_string())?;
            for entry in table.pools().iter().flat_map(|pool| &pool.entries) {
                if let mc_data::loot::chest_26_1_2::ChestLootEntry::Item { item, .. } = entry
                    && items.id_of(item).is_none()
                {
                    return Err(format!("reward loot {name}: unknown item {item}"));
                }
            }
            Ok(table)
        };
        tables.insert(name.clone(), load().map_err(crate::CoreError::Registry)?);
    }
    Ok(tables)
}

pub(crate) fn block_location(
    state: &TickState,
    pos: crate::boundary::BlockPos,
    block_id: Option<u32>,
) -> Value {
    let mut location = json!({"dimension":state.dimension.name(),"position":{"x":pos.x,"y":pos.y,"z":pos.z},"smokey":crate::bees::smoke(state,pos)});
    if let Some(biome) = state.world.biome_at(pos.x, pos.y, pos.z) {
        location["biomes"] = json!([biome.as_str()]);
    }
    if let Some(block) = block_id
        .or_else(|| state.world.block_at(pos.x, pos.y, pos.z))
        .and_then(|id| block_metadata(state, id))
    {
        location["block"] = block.clone();
    }
    location
}

pub(crate) fn accepted_block(
    state: &mut TickState,
    entity: EntityId,
    trigger_name: &str,
    pos: crate::boundary::BlockPos,
    block_id: Option<u32>,
    item: Option<mc_data::ItemStack>,
) {
    if state.registry.advancements.definitions.is_empty() {
        return;
    }
    let location = block_location(state, pos, block_id);
    let mut event = json!({"location":location});
    if let Some(item) = item {
        event["item"] = item_snapshot(state, &item);
    }
    trigger(state, entity, trigger_name, event);
}

pub(crate) fn damage(
    state: &mut TickState,
    victim: EntityId,
    source: Option<crate::intent::DamageSource>,
    kind: crate::survival::DamageKind,
    amounts: (f32, f32),
    killed: bool,
    blocked: bool,
) {
    use crate::intent::DamageSource;
    if state.registry.advancements.definitions.is_empty() {
        return;
    }
    let attacker = match source {
        Some(
            DamageSource::Melee { attacker, .. }
            | DamageSource::Mace { attacker, .. }
            | DamageSource::Sweep { attacker, .. },
        ) => Some(attacker),
        Some(DamageSource::Projectile { owner, .. }) => Some(owner),
        Some(DamageSource::Explosion { kill_credit, .. }) => kill_credit.map(|(id, _)| id),
        Some(DamageSource::Command { cause, .. }) => cause.map(|(id, _)| id),
        None => None,
    };
    let mut source_evidence = json!({});
    if let Some(attacker) = attacker {
        source_evidence["source_entity"] = entity_snapshot(state, attacker);
    }
    let damage_id = kind.type_name(
        source,
        attacker.is_some_and(|id| state.players.contains_key(&id)),
    );
    if let Ok(id) = mc_data::Identifier::parse(damage_id)
        && let Some(raw) = mc_data::required_registry_entry_id("damage_type", &id)
    {
        let tags: Vec<_> = state
            .registry
            .tags
            .registries
            .iter()
            .filter(|(registry, _)| registry.as_str() == "minecraft:damage_type")
            .flat_map(|(_, tags)| tags.iter())
            .filter(|(_, entries)| entries.contains(&(raw as i32)))
            .map(|(tag, _)| tag.as_str())
            .collect();
        source_evidence["tags"] = json!(tags);
    }
    if let Some(DamageSource::Projectile {
        projectile,
        direct_type_id,
        ..
    }) = source
    {
        let mut direct = entity_snapshot(state, projectile);
        if direct.is_null() {
            direct =
                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(direct_type_id)
                    .map_or(Value::Null, |contract| json!({"type":contract.name,"tags":entity_type_tags(state,contract.protocol_id)}));
        }
        source_evidence["direct_entity"] = direct;
    } else if let Some(DamageSource::Command { direct, .. }) = source {
        if let Some((entity, type_id)) = direct {
            let mut evidence = entity_snapshot(state, entity);
            if evidence.is_null() {
                evidence = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(
                    type_id,
                )
                .map_or(
                    Value::Null,
                    |contract| json!({"type":contract.name,"tags":entity_type_tags(state,type_id)}),
                );
            }
            source_evidence["direct_entity"] = evidence;
        }
    } else if let Some(attacker) = attacker {
        source_evidence["direct_entity"] = entity_snapshot(state, attacker);
    }
    let victim_evidence = entity_snapshot(state, victim);
    let (dealt, taken) = amounts;
    if let Some(attacker) = attacker {
        trigger(
            state,
            attacker,
            "minecraft:player_hurt_entity",
            json!({"entity":victim_evidence,"damage":{"dealt":dealt,"taken":taken,"blocked":blocked,"type":source_evidence}}),
        );
        if killed {
            trigger(
                state,
                attacker,
                "minecraft:player_killed_entity",
                json!({"entity":victim_evidence,"killing_blow":source_evidence}),
            );
        }
    }
    if state.players.contains_key(&victim) {
        trigger(
            state,
            victim,
            "minecraft:entity_hurt_player",
            json!({"damage":{"dealt":dealt,"taken":taken,"blocked":blocked,"type":source_evidence}}),
        );
        if killed {
            trigger(
                state,
                victim,
                "minecraft:entity_killed_player",
                json!({"entity":attacker.map(|attacker|entity_snapshot(state,attacker)),"killing_blow":source_evidence}),
            );
        }
    }
    if killed && let Some(DamageSource::Projectile { projectile, .. }) = source {
        crate::projectiles::payloads::record_crossbow_kill(state, projectile, victim_evidence);
    }
}
