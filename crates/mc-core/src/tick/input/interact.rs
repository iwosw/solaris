use crate::boundary::{
    BlockPos, ChangeSet, ConnectionId, DigAction, EntityId, Hand, PlayerChange, PlayerInput,
};
use crate::intent::{Footprint, Intent, IntentKind};
use crate::tick::{
    TickState, apply_player_effects, bucket_intent, container_cells, container_menu_for,
    container_slots, held_slot_index, inventory_transition_refused, merchant, plan_break,
    plan_placement, stations, within_block_reach, within_entity_reach,
};

pub(in crate::tick) fn build_preview_fence(fence: &crate::build_precommit::PreviewFence) {
    fence.apply();
}

pub(in crate::tick) fn dig(
    state: &mut TickState,
    (connection, position, action, sequence): (&ConnectionId, &BlockPos, &DigAction, &i32),
    intents: &mut Vec<Intent>,
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    state.push_player(
        *connection,
        PlayerChange::BlockActionsAcked {
            sequence: *sequence,
        },
    );
    let Some(current) = state.world.block_at(position.x, position.y, position.z) else {
        return;
    };
    if !crate::plugin_player::allows_block(
        state,
        entity,
        mc_world::BlockPos {
            x: position.x,
            y: position.y,
            z: position.z,
        },
    ) || crate::plugin_inventories::has_reservations_for_block(state, *position)
    {
        changes.blocks.push(crate::boundary::BlockChange {
            position: *position,
            state: current,
        });
        return;
    }
    if !within_block_reach(state, entity, *position) {
        changes.blocks.push(crate::boundary::BlockChange {
            position: *position,
            state: current,
        });
        return;
    }
    if current == state.registry.states.air {
        return;
    }
    // Vanilla `handleBlockBreakAction`: a finished dig breaks; a
    // started one breaks at once in creative and on blocks with
    // no hardness (grass, flowers, torches), which the client
    // destroys without ever sending the finish.
    let creative = state
        .players
        .get(&entity)
        .is_some_and(|player| player.creative());
    let instant = creative || crate::effects::mining_progress(state, entity, current) >= 1.0;
    let breaks = match action {
        DigAction::Finish => true,
        DigAction::Start => instant,
        _ => false,
    };
    if !breaks {
        return;
    }
    let mut intent = plan_break(state, entity, *position, current);
    intent.seq = state.next_seq(entity);
    intents.push(intent);
}

pub(in crate::tick) fn use_item_on(
    state: &mut TickState,
    (connection, position, hand, face, cursor, sequence): (
        &ConnectionId,
        &BlockPos,
        &Hand,
        &u8,
        &[f32; 3],
        &i32,
    ),
    input: &PlayerInput,
    intents: &mut Vec<Intent>,
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if crate::tick::client_build_refused(state, entity, input) {
        crate::build_precommit::resync_input(state, input);
        return;
    }
    state.push_player(
        *connection,
        PlayerChange::BlockActionsAcked {
            sequence: *sequence,
        },
    );
    if crate::leash::use_fence(state, entity, *position) {
        return;
    }
    // A client may use its predicted block before the placement
    // in this input batch reaches merge. Replay next tick against
    // the accepted world, never against an uncommitted intent.
    if intents
        .iter()
        .any(|intent| crate::build_precommit::is_placement(intent, *position))
    {
        state.deferred_uses.push(input.clone());
        return;
    }
    // Empty-hand collection must precede the ordinary held-item
    // placement path; composting owns its single inventory debit.
    if crate::copper::use_on(state, entity, *hand, *position)
        || crate::spawn_egg::use_on(state, entity, *hand, *position, *face)
        || crate::armor_stand::use_on(state, entity, *hand, *position, *face)
    {
        return;
    }
    if crate::composter::use_on(state, entity, *hand, *position, *sequence, changes) {
        return;
    }
    if crate::vehicles::use_on(state, entity, *hand, *position, *face, changes) {
        return;
    }
    if crate::shelf::use_on(state, entity, *hand, *position, *face, *cursor) {
        return;
    }
    // Containers open on use: chest/barrel/crafting/furnace family.
    if let Some((kind, title, size)) = container_menu_for(state, position.x, position.y, position.z)
    {
        if kind == crate::inventory::MenuKind::Beacon {
            crate::beacon::open(state, entity, *connection, *position, changes);
            return;
        }
        if kind.is_station() {
            stations::open(state, entity, *connection, *position, kind, title, changes);
            return;
        }
        // One storage per cell; the menu shows them back to back
        // (a double chest's two halves share its 54 slots).
        let cells = container_cells(state, (position.x, position.y, position.z));
        let each = size / cells.len();
        for cell in &cells {
            let storage = state.block_entities.entry(*cell).or_insert_with(|| {
                crate::tick::BlockEntityState {
                    slots: vec![mc_data::ItemStack::EMPTY; each],
                    furnace: None,
                }
            });
            // Resize legacy/smaller storages to the menu size.
            if storage.slots.len() != each {
                storage.slots.resize(each, mc_data::ItemStack::EMPTY);
            }
        }
        let slots = container_slots(state, &cells);
        let rules = state.registry.item_rules();
        let effects = state
            .players
            .get_mut(&entity)
            .map(|player| player.open_menu(&rules, kind, title, &slots))
            .unwrap_or_default();
        apply_player_effects(state, *connection, entity, effects, changes);
        crate::statistics::opened(state, entity, *position);
        state
            .open_containers
            .insert(entity, (position.x, position.y, position.z));
        if matches!(kind, crate::inventory::MenuKind::Furnace(_)) {
            let properties = state
                .block_entities
                .get_mut(&(position.x, position.y, position.z))
                .expect("opened container storage")
                .furnace
                .get_or_insert_with(Default::default)
                .properties();
            let window = state.players[&entity]
                .open
                .as_ref()
                .expect("opened menu")
                .window;
            for (property, value) in properties.into_iter().enumerate() {
                state.push_player(
                    *connection,
                    PlayerChange::MenuProperty {
                        window,
                        property: property as i16,
                        value: value as i16,
                    },
                );
            }
        }
        if kind == crate::inventory::MenuKind::BrewingStand {
            crate::brewing::opened(state, entity, *connection, *position);
        }
        if kind == crate::inventory::MenuKind::Crafter {
            crate::crafter::opened(state, entity, *position);
        }
        return;
    }
    if crate::end_portals::use_eye(state, entity, *hand, *position) {
        return;
    }
    if crate::end_fight::use_crystal(state, entity, *position, *hand, changes) {
        return;
    }
    if crate::dimensions::rules::use_block(state, entity, *position, *hand, intents) {
        return;
    }
    // Beds put the player to sleep.
    if crate::sleep::use_bed(state, entity, *connection, *position) {
        return;
    }
    if use_bell(state, *connection, *position, *face) {
        return;
    }
    // Interactive blocks react to the hand (doors, gates, levers).
    let rotation = state
        .entities
        .slot_of(entity)
        .map(|slot| state.entities.rot(slot));
    if let Some([yaw, _, _]) = rotation
        && let Some(cells) =
            crate::placement::use_block(&state.registry.blocks, *position, yaw, &|pos: BlockPos| {
                state.world.block_at(pos.x, pos.y, pos.z)
            })
    {
        let Some(block_expect) = cells
            .iter()
            .map(|(pos, _)| {
                state
                    .world
                    .block_at(pos.x, pos.y, pos.z)
                    .map(|current| (*pos, current))
            })
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let seq = state.next_seq(entity);
        intents.push(Intent {
            actor: entity,
            seq,
            kind: IntentKind::UseBlock {
                pos: *position,
                hand: Some(*hand),
            },
            footprint: Footprint {
                block_expect,
                block_write: cells,
                ..Default::default()
            },
        });
        return;
    }
    // Otherwise place the held block item (vanilla placement
    // state, second cell of doors and beds).
    let held = state
        .players
        .get(&entity)
        .map(|player| {
            player
                .inventory
                .slots
                .get(held_slot_index(player, *hand))
                .cloned()
                .unwrap_or(mc_data::ItemStack::EMPTY)
        })
        .unwrap_or(mc_data::ItemStack::EMPTY);
    if crate::jukebox::use_on(state, entity, *hand, *position, changes) {
        return;
    }
    if held.is_empty() {
        crate::signs::use_on(state, entity, *hand, *position);
        return;
    }
    if crate::vault::use_key(state, entity, *hand, *position, &held, changes) {
        return;
    }
    if crate::fire::use_on(
        state,
        entity,
        *hand,
        *position,
        crate::placement::Face::from_code(*face),
        changes,
    ) {
        return;
    }
    // Hoes till soil and bone meal grows crops.
    if let Some(intent) = crate::farming::use_intent(
        state,
        entity,
        *hand,
        *position,
        crate::placement::Face::from_code(*face),
        &held,
    ) {
        intents.push(intent);
        return;
    }
    if crate::bees::use_on(state, entity, *hand, *position, &held, changes) {
        return;
    }
    if crate::bottles::use_on(state, entity, *hand, *position, changes) {
        return;
    }
    if crate::campfire::use_food(state, entity, *hand, *position, &held) {
        return;
    }
    if let Some(intent) = crate::fluids::use_on(
        state,
        entity,
        *hand,
        *position,
        crate::placement::Face::from_code(*face),
        &held,
    ) {
        intents.push(intent);
        return;
    }
    // Buckets own waterlogging before the sign's ordinary editor
    // fallback; decorators and other held uses then reach the sign.
    if crate::signs::use_on(state, entity, *hand, *position) {
        return;
    }
    if let Some(mut intent) = plan_placement(state, entity, *hand, *position, *face, *cursor, &held)
    {
        intent.seq = state.next_seq(entity);
        intents.push(intent);
    }
}

/// Handles the vanilla bell block action and returns whether `position` was a bell.
fn use_bell(
    state: &mut TickState,
    connection: ConnectionId,
    position: BlockPos,
    parameter: u8,
) -> bool {
    let Some(state_id) = state.world.block_at(position.x, position.y, position.z) else {
        return false;
    };
    let Some(block_state) = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(state_id))
    else {
        return false;
    };
    if block_state.block.id.path() != "bell" {
        return false;
    }
    let Ok(block) = u32::try_from(block_state.block.raw_id) else {
        return false;
    };
    let settlement = state
        .settlements
        .records()
        .find(|record| record.anchor == [position.x, position.y, position.z])
        .map(|record| record.id);
    crate::redstone::ring_bell(state, position, block, state_id, parameter);
    if let Some(settlement) = settlement {
        crate::settlements::ui::open_for_settlement(state, connection, settlement);
    }
    true
}

pub(in crate::tick) fn interact_entity(
    state: &mut TickState,
    (connection, target, hand, location): (&ConnectionId, &EntityId, &Hand, &[f64; 3]),
    input: &PlayerInput,
    intents: &mut Vec<Intent>,
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    let (Some(_), Some(target_slot)) = (
        state.entities.slot_of(entity),
        state.entities.slot_of(*target),
    ) else {
        return;
    };
    if !within_entity_reach(state, entity, *target) {
        return;
    }
    let secondary = matches!(input, PlayerInput::InteractEntitySecondary { .. });
    if crate::armor_stand::interact(state, entity, *target, *hand, *location) {
        crate::plugin::entity_interaction(state, entity, *target, *hand, secondary, None);
        return;
    }
    if crate::piglin::interact(state, entity, *target, *hand) {
        crate::plugin::entity_interaction(state, entity, *target, *hand, secondary, None);
        return;
    }
    if crate::golden_dandelion::interact(state, entity, *target, *hand) {
        crate::plugin::entity_interaction(state, entity, *target, *hand, secondary, None);
        return;
    }
    if crate::allay::interact(state, entity, *target, *hand, changes) {
        crate::plugin::entity_interaction(state, entity, *target, *hand, secondary, None);
        return;
    }
    if crate::copper_golem::interact(state, entity, *target, *hand) {
        crate::plugin::entity_interaction(state, entity, *target, *hand, secondary, None);
        return;
    }
    if crate::vehicles::interact(state, entity, *target, *hand, secondary, changes) {
        crate::plugin::entity_interaction(state, entity, *target, *hand, secondary, None);
        return;
    }
    let first_intent = intents.len();
    if crate::mounts::interact(state, entity, *target, *hand, secondary, intents) {
        crate::plugin::entity_interaction(
            state,
            entity,
            *target,
            *hand,
            secondary,
            intents.get(first_intent),
        );
        return;
    }
    if crate::villager::conversion::interact(state, entity, target_slot, *hand) {
        crate::plugin::entity_interaction(state, entity, *target, *hand, secondary, None);
        return;
    }
    if merchant::open(state, entity, *target, *connection, *hand, changes) {
        crate::plugin::entity_interaction(state, entity, *target, *hand, secondary, None);
        return;
    }
    let first_intent = intents.len();
    if crate::dolphin::interact(state, entity, *target, *hand, intents) {
        crate::plugin::entity_interaction(
            state,
            entity,
            *target,
            *hand,
            secondary,
            intents.get(first_intent),
        );
        return;
    }
    let first_intent = intents.len();
    crate::entity_life::interact(state, entity, *target, *hand, intents);
    if let Some(intent) = intents.get(first_intent) {
        crate::plugin::entity_interaction(state, entity, *target, *hand, secondary, Some(intent));
    }
}

pub(in crate::tick) fn use_item(
    state: &mut TickState,
    (connection, hand, yaw, pitch, sequence): (&ConnectionId, &Hand, &f32, &f32, &i32),
    intents: &mut Vec<Intent>,
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    state.push_player(
        *connection,
        PlayerChange::BlockActionsAcked {
            sequence: *sequence,
        },
    );
    let Some(slot) = state.entities.slot_of(entity) else {
        return;
    };
    // Vanilla use packets carry the look direction used by blocking.
    state.entities.set_rot(slot, *yaw, *pitch);
    if crate::physical_defense::set_item_use(state, slot, *hand, true)
        || crate::physical_attacks::try_use(state, slot, *hand)
            != crate::physical_attacks::AttackAdmission::NotCustom
    {
        return;
    }
    if crate::bottles::use_item(state, entity, *hand, *yaw, *pitch, changes) {
        return;
    }
    if let Some(intent) = bucket_intent(state, entity, slot, *hand, *yaw, *pitch) {
        intents.push(intent);
        return;
    }
    if crate::projectiles::weapons::use_item(state, entity, *hand) {
        return;
    }
    let rules = state.registry.item_rules();
    let player_type = crate::entities::player_type_id() as i32;
    if crate::plugin_inventories::has_reservations_for_player(state, entity) {
        let Some(player) = state.players.get(&entity) else {
            return;
        };
        let mut swapped = false;
        let plan = player.plan_inventory_transition(&[], |candidate, _| {
            let effects = candidate.swap_equippable(&rules, *hand, player_type);
            swapped = effects.is_some();
            effects.unwrap_or_default()
        });
        let effects =
            if crate::plugin_inventories::below_player_floors(state, entity, plan.player_slots()) {
                inventory_transition_refused(state, entity)
            } else {
                let player = state
                    .players
                    .get_mut(&entity)
                    .expect("prepared equipment owner");
                let effects = player.apply_inventory_transition(plan, &mut []);
                if !swapped {
                    player.use_item(&rules, *hand);
                }
                effects
            };
        apply_player_effects(state, *connection, entity, effects, changes);
        return;
    }
    let effects = state.players.get_mut(&entity).and_then(|player| {
        let effects = player.swap_equippable(&rules, *hand, player_type);
        if effects.is_none() {
            player.use_item(&rules, *hand);
        }
        effects
    });
    if let Some(effects) = effects {
        apply_player_effects(state, *connection, entity, effects, changes);
    }
}

pub(in crate::tick) fn release_use_item(state: &mut TickState, connection: &ConnectionId) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    let Some(slot) = state.entities.slot_of(entity) else {
        return;
    };
    if crate::projectiles::weapons::release(state, entity) {
        return;
    }
    if crate::physical_defense::set_item_use(state, slot, crate::boundary::Hand::Main, false)
        || crate::physical_attacks::try_release(state, slot)
            != crate::physical_attacks::AttackAdmission::NotCustom
    {
        return;
    }
    if let Some(player) = state.players.get_mut(&entity) {
        player.release_use_item();
    }
}
