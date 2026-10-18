//! Server-owned bottle filling and water-bottle cauldron exchanges.
use crate::boundary::{BlockPos, ChangeSet, EntityEvent, EntityId, Hand};
use crate::fluids::Fluid;
use crate::tick::{TickState, held_slot_index};
use mc_data::{Identifier, ItemStack};

fn product(state: &TickState, water: bool) -> ItemStack {
    let name = if water {
        "minecraft:potion"
    } else {
        "minecraft:glass_bottle"
    };
    let id = state
        .registry
        .items
        .id_of(&Identifier::parse(name).expect("vanilla item"))
        .expect("vanilla bottle item");
    let mut stack = ItemStack::new(id, 1);
    if water {
        stack.extended_mut().potion =
            Some(Identifier::parse("minecraft:water").expect("vanilla potion"));
    }
    stack
}

fn exchange(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    water: bool,
    cauldron: bool,
    changes: &mut ChangeSet,
) -> bool {
    let product = product(state, water);
    let rules = state.registry.item_rules();
    let player = state.players.get(&actor).expect("bottle player");
    let connection = player.connection;
    // ItemUtils.createFilledResult keeps creative input and only adds a result
    // when no identical item/components already exist; a full inventory drops
    // survival overflow, not creative overflow.
    let exchange = |player: &mut crate::player::PlayerState, _: &mut [ItemStack]| {
        if cauldron {
            player.statistics.increment_custom(
                if water {
                    mc_data::statistics::CustomStat::UseCauldron
                } else {
                    mc_data::statistics::CustomStat::FillCauldron
                },
                1,
            );
        }
        if player.creative() {
            let item = player.inventory.slots[held_slot_index(player, hand)].item_id;
            player
                .statistics
                .increment(mc_data::statistics::StatType::Used, item, 1);
        }
        if player.creative() {
            if player.inventory.slots.iter().any(|stack| {
                !stack.is_empty() && mc_data::inventory_semantics_26_1_2::can_stack(stack, &product)
            }) {
                crate::player::PlayerEffects::default()
            } else {
                let mut effects = player.receive_stack(&rules, product);
                effects.dropped.clear();
                effects
            }
        } else {
            player.harvest_held(&rules, hand, product, true)
        }
    };
    let effects = if crate::plugin_inventories::has_reservations_for_player(state, actor) {
        let plan = player.plan_inventory_transition(&[], exchange);
        if crate::plugin_inventories::below_player_floors(state, actor, plan.player_slots()) {
            return false;
        }
        state
            .players
            .get_mut(&actor)
            .unwrap()
            .apply_inventory_transition(plan, &mut [])
    } else {
        exchange(state.players.get_mut(&actor).unwrap(), &mut [])
    };
    crate::tick::apply_player_effects(state, connection, actor, effects, changes);
    changes.entity_events.push(EntityEvent::Bottle {
        entity: actor,
        empty: !water,
        cauldron,
    });
    true
}

/// BottleItem.use uses a SOURCE_ONLY fluid ray, not a UseItemOn target.
/// Bottling does not remove the source (including contained water).
pub(crate) fn use_item(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    yaw: f32,
    pitch: f32,
    changes: &mut ChangeSet,
) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let Some(held) = player.inventory.slots.get(held_slot_index(player, hand)) else {
        return false;
    };
    if held.is_empty()
        || state
            .registry
            .items
            .name_of(held.item_id)
            .is_none_or(|item| item.as_str() != "minecraft:glass_bottle")
    {
        return false;
    }
    let connection = player.connection;
    if !player.spectator()
        && let Some(slot) = state.entities.slot_of(actor)
    {
        let pos = state.entities.pos(slot);
        let eye = [
            pos[0],
            pos[1] + crate::tick::player_eye_height(state.entities.row(slot).appearance.pose),
            pos[2],
        ];
        if let Some((target, _)) = crate::fluids::bucket_target(state, eye, yaw, pitch, None)
            && let Some(current) = state.world.block_at(target.x, target.y, target.z)
            && crate::fluids::contained(state, current) == Some((Fluid::Water, 0))
        {
            if !exchange(state, actor, hand, true, false, changes) {
                let contents = state.players[&actor].inventory_contents();
                state.push_player(connection, contents);
                return true;
            }
            crate::vibration::emit(
                state,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y) + 0.5,
                    f64::from(target.z) + 0.5,
                ],
                crate::vibration::GameEvent::FluidPickup,
                Some(actor),
            );
            return true;
        }
    }
    // A miss/refusal must retire any client-side held-item prediction too.
    let contents = state.players[&actor].inventory_contents();
    state.push_player(connection, contents);
    true
}

/// Cauldron interactions exchange one bottle and one water level atomically
/// on the same input owner as hive harvesting; all after-images ride this tick.
pub(crate) fn use_on(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    pos: BlockPos,
    changes: &mut ChangeSet,
) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.spectator() {
        return false;
    }
    let Some(held) = player.inventory.slots.get(held_slot_index(player, hand)) else {
        return false;
    };
    let Some(item) = state
        .registry
        .items
        .name_of(held.item_id)
        .filter(|_| !held.is_empty())
    else {
        return false;
    };
    let bottle = item.as_str() == "minecraft:glass_bottle";
    let water = item.as_str() == "minecraft:potion"
        && held
            .extended()
            .potion
            .as_ref()
            .is_some_and(|id| id.as_str() == "minecraft:water")
        && held.extended().potion_custom_effects.is_empty();
    if !bottle && !water {
        return false;
    }
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(current)) else {
        return false;
    };
    let level = match block.block.id.as_str() {
        "minecraft:cauldron" => 0,
        "minecraft:water_cauldron" => {
            let Some(level) = block
                .properties
                .iter()
                .find(|(key, _)| key == "level")
                .and_then(|(_, value)| value.parse::<u8>().ok())
            else {
                return false;
            };
            level
        }
        _ => return false,
    };
    let next = if bottle && level > 0 {
        level - 1
    } else if water && level < 3 {
        level + 1
    } else {
        return false;
    };
    let new = if next == 0 {
        state
            .registry
            .blocks
            .block(&Identifier::parse("minecraft:cauldron").expect("vanilla block"))
            .expect("cauldron")
            .default
            .0
    } else {
        state
            .registry
            .blocks
            .by_name_and_props(
                &Identifier::parse("minecraft:water_cauldron").expect("vanilla block"),
                &[("level".into(), next.to_string())],
            )
            .expect("water cauldron level")
            .0
    };
    if !exchange(state, actor, hand, bottle, true, changes) {
        return true;
    }
    state.world.set_block(pos.x, pos.y, pos.z, new);
    state.merge_block_writes.push((pos, new));
    crate::vibration::emit(
        state,
        [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ],
        if bottle {
            crate::vibration::GameEvent::FluidPickup
        } else {
            crate::vibration::GameEvent::FluidPlace
        },
        Some(actor),
    );
    true
}
