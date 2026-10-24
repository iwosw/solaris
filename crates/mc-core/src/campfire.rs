//! Four-slot campfire cooking with vanilla item NBT and recipe times.

use std::collections::BTreeMap;

use mc_data::item_nbt::{field, stack_from_fields, stack_to_fields};
use mc_data::recipes::{Recipe, RecipeKind, ingredient_accepts_stack};
use mc_data::{ItemStack, items::ItemRegistry};
use mc_nbt::{ListTag, Tag};

use crate::boundary::{BlockPos, EntityId, Hand, PlayerChange};
use crate::inventory::ItemRules;
use crate::tick::{TickState, spawn_item_entity};

pub(crate) type Campfires = BTreeMap<(i32, i32, i32), CampfireState>;

fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CampfireState {
    pub(crate) slots: [ItemStack; 4],
    pub(crate) progress: [i32; 4],
    total: [i32; 4],
}

fn recipe<'a>(rules: &ItemRules<'a>, stack: &ItemStack) -> Option<&'a Recipe> {
    rules.recipes.iter().find(|recipe| {
        let RecipeKind::CampfireCooking(cooking) = &recipe.kind else {
            return false;
        };
        ingredient_accepts_stack(
            rules.items,
            rules.facts,
            rules.tags,
            stack,
            &cooking.ingredient,
        )
    })
}

fn lit(state: &TickState, pos: BlockPos) -> Option<bool> {
    let block = state.registry.blocks.by_id(mc_world::BlockStateId(
        state.world.block_at(pos.x, pos.y, pos.z)?,
    ))?;
    matches!(
        block.block.id.as_str(),
        "minecraft:campfire" | "minecraft:soul_campfire"
    )
    .then(|| {
        block
            .properties
            .iter()
            .any(|(key, value)| key == "lit" && value == "true")
    })
}

/// Hydrate only vacant entries: live progress is authoritative, just like furnaces.
pub(crate) fn hydrate_chunk(
    campfires: &mut Campfires,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) {
    for (pos, bytes) in &chunk.block_entities {
        if campfires.contains_key(&key(*pos)) {
            continue;
        }
        let is_campfire = chunk
            .get_block(
                pos.x.rem_euclid(16) as u8,
                pos.y,
                pos.z.rem_euclid(16) as u8,
            )
            .and_then(|id| registry.blocks.by_id(id))
            .is_some_and(|block| {
                matches!(
                    block.block.id.as_str(),
                    "minecraft:campfire" | "minecraft:soul_campfire"
                )
            });
        if is_campfire && let Some(campfire) = CampfireState::decode(bytes, &registry.items) {
            campfires.insert(key(*pos), campfire);
        }
    }
}

/// Non-food clicks deliberately retain ordinary adjacent block placement.
pub(crate) fn intercepts_placement(state: &TickState, pos: BlockPos, held: &ItemStack) -> bool {
    !held.is_empty()
        && lit(state, pos).is_some()
        && recipe(&state.registry.item_rules(), held).is_some()
}

/// A food use owns the interaction even when the fire is unlit or full.
pub(crate) fn use_food(
    state: &mut TickState,
    entity: EntityId,
    hand: Hand,
    pos: BlockPos,
    held: &ItemStack,
) -> bool {
    let Some(is_lit) = lit(state, pos) else {
        return false;
    };
    if held.is_empty() {
        return false;
    }
    let Some(player) = state.players.get(&entity) else {
        return true;
    };
    // Non-food uses still fall through to ordinary adjacent block placement.
    let rules = state.registry.item_rules();
    let Some(recipe) = recipe(&rules, held) else {
        return false;
    };
    // Vanilla preloads raw food onto an unlit campfire; cooking only
    // resumes once relit (`run` pauses progress while unlit).
    let _ = is_lit;
    if player.game_mode == crate::commands::GameMode::Spectator {
        return true;
    }
    let RecipeKind::CampfireCooking(cooking) = &recipe.kind else {
        unreachable!()
    };
    let total = i32::try_from(cooking.cooking_time)
        .unwrap_or(i32::MAX)
        .max(1);
    if crate::tick::held_debit_below_floor(state, entity, hand, 1) {
        return true;
    }
    let campfire = state.campfires.entry(key(pos)).or_insert_with(|| {
        let chunk = crate::boundary::ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        state
            .world
            .chunk(chunk)
            .and_then(|chunk| chunk.block_entities.get(&pos))
            .and_then(|bytes| CampfireState::decode(bytes, &state.registry.items))
            .unwrap_or_default()
    });
    let Some(index) = campfire.slots.iter().position(ItemStack::is_empty) else {
        return true;
    };
    campfire.slots[index] = held.clone();
    campfire.slots[index].count = 1;
    campfire.progress[index] = 0;
    campfire.total[index] = total;
    let connection = player.connection;
    let effects = state
        .players
        .get_mut(&entity)
        .expect("player exists")
        .consume_held(hand, 1);
    for change in effects.changes {
        state.push_player(connection, change);
    }
    let campfires = std::mem::take(&mut state.campfires);
    persist(state, pos, &campfires[&key(pos)], true);
    state.campfires = campfires;
    true
}

/// Cooking/cooldown is paused outside resident chunks. Outputs use ordinary
/// item-entity spawning, publication, pickup and entity journaling.
pub(crate) fn run(state: &mut TickState, drops: &mut Vec<EntityId>) {
    let mut campfires = std::mem::take(&mut state.campfires);
    campfires.retain(|pos, campfire| {
        let pos = &BlockPos {
            x: pos.0,
            y: pos.1,
            z: pos.2,
        };
        if !state
            .world
            .is_block_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
        {
            return true;
        }
        let Some(is_lit) = lit(state, *pos) else {
            for stack in &mut campfire.slots {
                if !stack.is_empty() {
                    drops.push(spawn_item_entity(
                        state,
                        drop_pos(*pos),
                        std::mem::take(stack),
                    ));
                }
            }
            state.world.set_opaque_block_entity(*pos, None);
            state.opaque_changes.push((*pos, None));
            return false;
        };
        let mut changed = false;
        let mut items_changed = false;
        for index in 0..4 {
            if !is_lit {
                let next = (campfire.progress[index] - 2).max(0);
                changed |= next != campfire.progress[index];
                campfire.progress[index] = next;
                continue;
            }
            if campfire.slots[index].is_empty() {
                continue;
            }
            changed = true;
            campfire.progress[index] = campfire.progress[index].saturating_add(1);
            if campfire.progress[index] >= campfire.total[index] {
                let rules = state.registry.item_rules();
                let result = recipe(&rules, &campfire.slots[index])
                    .and_then(|recipe| recipe.result.to_stack(rules.items, rules.facts))
                    .unwrap_or_else(|| campfire.slots[index].clone());
                drops.push(spawn_item_entity(state, drop_pos(*pos), result));
                campfire.slots[index] = ItemStack::EMPTY;
                items_changed = true;
            }
        }
        if changed {
            persist(state, *pos, campfire, items_changed);
        }
        true
    });
    state.campfires = campfires;
}

fn drop_pos(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

fn persist(state: &mut TickState, pos: BlockPos, campfire: &CampfireState, sync: bool) {
    let (bytes, update) = campfire.encode(pos, &state.registry.items, sync);
    let chunk_pos = crate::boundary::ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    };
    // Timers go to the WAL each tick, but don't copy a shared whole chunk or
    // invalidate its wire cache until Items change (or a lit edit cleared NBT).
    if sync
        || state
            .world
            .chunk(chunk_pos)
            .is_none_or(|chunk| !chunk.block_entities.contains_key(&pos))
    {
        state
            .world
            .set_opaque_block_entity(pos, Some(bytes.clone()));
    }
    state.opaque_changes.push((pos, Some(bytes)));
    if let Some(nbt) = update {
        state.world.mark_block_entity_visible(pos);
        let viewers: Vec<_> = state
            .players
            .values()
            .map(|player| player.connection)
            .collect();
        for connection in viewers {
            state.push_player(
                connection,
                PlayerChange::BlockEntityData {
                    position: pos,
                    block_entity_type: mc_data::block_entity_kinds::BlockEntityKind::Campfire
                        .protocol_id(),
                    nbt: nbt.clone(),
                },
            );
        }
    }
}

/// Freeze current timers into the region base before its checkpoint supersedes
/// their WAL post-images. As with furnace injection, this runs only at capture.
pub(crate) fn checkpoint(state: &mut TickState) {
    let campfires = std::mem::take(&mut state.campfires);
    for ((x, y, z), campfire) in &campfires {
        let pos = BlockPos {
            x: *x,
            y: *y,
            z: *z,
        };
        let (bytes, _) = campfire.encode(pos, &state.registry.items, false);
        state.world.set_opaque_block_entity(pos, Some(bytes));
    }
    state.campfires = campfires;
}

impl CampfireState {
    fn encode(&self, pos: BlockPos, items: &ItemRegistry, sync: bool) -> (Vec<u8>, Option<Tag>) {
        let mut fields = self.items_nbt(items);
        let update = sync.then(|| Tag::Compound(fields.clone()));
        fields.extend([
            ("id".into(), Tag::String("minecraft:campfire".into())),
            ("x".into(), Tag::Int(pos.x)),
            ("y".into(), Tag::Int(pos.y)),
            ("z".into(), Tag::Int(pos.z)),
            ("CookingTimes".into(), Tag::IntArray(self.progress.to_vec())),
            (
                "CookingTotalTimes".into(),
                Tag::IntArray(self.total.to_vec()),
            ),
        ]);
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("campfire NBT encodes");
        (bytes, update)
    }

    fn items_nbt(&self, items: &ItemRegistry) -> Vec<(String, Tag)> {
        let elements = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, stack)| !stack.is_empty())
            .map(|(index, stack)| {
                let mut fields = stack_to_fields(items, stack).expect("registered campfire item");
                fields.push(("Slot".into(), Tag::Byte(index as i8)));
                Tag::Compound(fields)
            })
            .collect();
        vec![(
            "Items".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements,
            }),
        )]
    }

    fn decode(mut bytes: &[u8], items: &ItemRegistry) -> Option<Self> {
        let Tag::Compound(fields) = mc_nbt::read_network(&mut bytes).ok()? else {
            return None;
        };
        let mut campfire = Self::default();
        if let Some(Tag::List(list)) = field(&fields, "Items") {
            for item in &list.elements {
                let Tag::Compound(fields) = item else {
                    continue;
                };
                let index = match field(fields, "Slot") {
                    Some(Tag::Byte(index)) => i32::from(*index),
                    Some(Tag::Int(index)) => *index,
                    _ => continue,
                };
                if (0..4).contains(&index) {
                    campfire.slots[index as usize] = stack_from_fields(items, fields).ok()??;
                }
            }
        }
        for (name, target) in [
            ("CookingTimes", &mut campfire.progress),
            ("CookingTotalTimes", &mut campfire.total),
        ] {
            if let Some(Tag::IntArray(values)) = field(&fields, name) {
                for (slot, value) in target.iter_mut().zip(values) {
                    *slot = (*value).max(0);
                }
            }
        }
        Some(campfire)
    }
}

#[cfg(test)]
#[path = "campfire_tests.rs"]
mod tests;
