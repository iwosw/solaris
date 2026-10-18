//! Beacon structure, selection and area effects owned by the native tick.
//! Opaque BE post-images follow conduit.rs so selections reach the WAL/checkpoint.
use crate::boundary::{BlockPos, ChangeSet, ChunkPos, ConnectionId, EntityId, PlayerChange};
use crate::effects::{ActiveEffect, EffectId};
use crate::inventory::MenuKind;
use crate::tick::TickState;
use mc_nbt::Tag;

fn named(state: &TickState, pos: BlockPos, name: &str) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| block.block.id.path() == name)
}

pub(crate) fn levels(state: &TickState, pos: BlockPos) -> i32 {
    let Some(chunk) = state.world.chunk(ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    }) else {
        return 0;
    };
    // Vanilla blocks the beam only at full light dampening (bedrock is exempt).
    if !(pos.y + 1..chunk.geometry().max_y()).all(|y| {
        state.world.block_at(pos.x, y, pos.z).is_some_and(|id| {
            state
                .registry
                .light
                .opacity(id)
                .is_some_and(|opacity| opacity < 15)
                || named(
                    state,
                    BlockPos {
                        x: pos.x,
                        y,
                        z: pos.z,
                    },
                    "bedrock",
                )
        })
    }) {
        return 0;
    }
    let mut levels = 0;
    for layer in 1..=4 {
        for x in -layer..=layer {
            for z in -layer..=layer {
                let valid = state
                    .world
                    .block_at(pos.x + x, pos.y - layer, pos.z + z)
                    .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                    .is_some_and(|block| {
                        matches!(
                            block.block.id.path(),
                            "iron_block"
                                | "gold_block"
                                | "emerald_block"
                                | "diamond_block"
                                | "netherite_block"
                        )
                    });
                if !valid {
                    return levels;
                }
            }
        }
        levels = layer;
    }
    levels
}

fn image(state: &TickState, pos: BlockPos) -> Vec<(String, Tag)> {
    state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos))
        .map(|bytes| {
            let mut reader = bytes.as_slice();
            let tag = mc_nbt::read_network(&mut reader).expect("loaded beacon NBT");
            let Tag::Compound(fields) = tag else {
                panic!("beacon BE must be a compound")
            };
            fields
        })
        .unwrap_or_default()
}
fn selection(fields: &[(String, Tag)], key: &str) -> Option<EffectId> {
    fields.iter().find_map(|(name, tag)| match tag {
        Tag::String(value) if name == key => EffectId::from_name(value),
        _ => None,
    })
}
fn allowed(primary: EffectId, level: i32) -> bool {
    match primary {
        EffectId::Speed | EffectId::Haste => level >= 1,
        EffectId::Resistance | EffectId::JumpBoost => level >= 2,
        EffectId::Strength => level >= 3,
        _ => false,
    }
}
fn store(state: &mut TickState, pos: BlockPos, mut fields: Vec<(String, Tag)>) {
    fields.retain(|(key, _)| !matches!(key.as_str(), "id" | "x" | "y" | "z"));
    fields.extend([
        ("id".into(), Tag::String("minecraft:beacon".into())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ]);
    let mut bytes = Vec::new();
    let update = Tag::Compound(fields);
    mc_nbt::write_network(&mut bytes, &update).expect("beacon NBT encodes");
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
    state.world.mark_block_entity_visible(pos);
    let viewers: Vec<_> = state.players.values().map(|p| p.connection).collect();
    for connection in viewers {
        state.push_player(
            connection,
            PlayerChange::BlockEntityData {
                position: pos,
                block_entity_type: mc_data::block_entity_kinds::BlockEntityKind::Beacon
                    .protocol_id(),
                nbt: update.clone(),
            },
        );
    }
}
fn properties(
    state: &mut TickState,
    entity: EntityId,
    level: i32,
    primary: Option<EffectId>,
    secondary: Option<EffectId>,
) {
    let values = [
        level,
        primary.map_or(0, |id| id as i32 + 1),
        secondary.map_or(0, |id| id as i32 + 1),
    ];
    let Some(player) = state.players.get(&entity) else {
        return;
    };
    let Some(menu) = player
        .open
        .as_ref()
        .filter(|menu| menu.kind == MenuKind::Beacon)
    else {
        return;
    };
    let (window, connection) = (menu.window, player.connection);
    for (property, value) in values.into_iter().enumerate() {
        state.push_player(
            connection,
            PlayerChange::MenuProperty {
                window,
                property: property as i16,
                value: value as i16,
            },
        );
    }
}
pub(crate) fn open(
    state: &mut TickState,
    entity: EntityId,
    connection: ConnectionId,
    pos: BlockPos,
    changes: &mut ChangeSet,
) {
    let registry = std::sync::Arc::clone(&state.registry);
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    let effects = player.open_menu(
        &registry.item_rules(),
        MenuKind::Beacon,
        "Beacon".into(),
        &[],
    );
    player
        .statistics
        .increment_custom(mc_data::statistics::CustomStat::InteractWithBeacon, 1);
    crate::tick::apply_player_effects(state, connection, entity, effects, changes);
    state.open_containers.insert(entity, (pos.x, pos.y, pos.z));
    let fields = image(state, pos);
    properties(
        state,
        entity,
        levels(state, pos),
        selection(&fields, "primary_effect"),
        selection(&fields, "secondary_effect"),
    );
}
pub(crate) fn configure(
    state: &mut TickState,
    connection: ConnectionId,
    primary: Option<u32>,
    secondary: Option<u32>,
) {
    let Some(entity) = state.connections.entity_of(connection) else {
        return;
    };
    let Some(&(x, y, z)) = state.open_containers.get(&entity) else {
        return;
    };
    let pos = BlockPos { x, y, z };
    let Some(slot) = state.entities.slot_of(entity) else {
        return;
    };
    if !state.entities.body(slot).alive
        || !named(state, pos, "beacon")
        || state
            .entities
            .pos(slot)
            .into_iter()
            .zip([x, y, z])
            .map(|(a, b)| (a - f64::from(b) - 0.5).powi(2))
            .sum::<f64>()
            > 64.0
    {
        return;
    }
    let Some(primary) = primary.and_then(|id| EffectId::ALL.get(id as usize).copied()) else {
        return;
    };
    let secondary = match secondary {
        Some(id) => match EffectId::ALL.get(id as usize) {
            Some(effect) => Some(*effect),
            None => return,
        },
        None => None,
    };
    let level = levels(state, pos);
    if !allowed(primary, level)
        || secondary.is_some_and(|id| level < 4 || (id != EffectId::Regeneration && id != primary))
    {
        return;
    }
    let rules = state.registry.item_rules();
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    let Some(menu) = player
        .open
        .as_mut()
        .filter(|menu| menu.kind == MenuKind::Beacon)
    else {
        return;
    };
    let payment = &mut menu.grid[0];
    if payment.is_empty() || !rules.is_beacon_payment(payment) {
        return;
    }
    mc_data::inventory_semantics_26_1_2::take_from_stack(payment, 1);
    menu.state_id = (menu.state_id + 1) & 0x7fff;
    let updates = player.sync_container_view(&[]);
    for update in updates {
        state.push_player(connection, update);
    }
    let mut fields = image(state, pos);
    fields.retain(|(key, _)| {
        !matches!(
            key.as_str(),
            "primary_effect" | "secondary_effect" | "Levels"
        )
    });
    fields.push(("Levels".into(), Tag::Int(level)));
    fields.push((
        "primary_effect".into(),
        Tag::String(format!("minecraft:{}", primary.name())),
    ));
    if let Some(secondary) = secondary {
        fields.push((
            "secondary_effect".into(),
            Tag::String(format!("minecraft:{}", secondary.name())),
        ));
    }
    store(state, pos, fields);
    properties(state, entity, level, Some(primary), secondary);
}
pub(crate) fn run(state: &mut TickState) {
    if !state.tick.is_multiple_of(80) {
        return;
    }
    let mut positions: Vec<_> = state
        .world
        .active_positions()
        .filter_map(|pos| state.world.chunk(pos))
        .flat_map(|chunk| chunk.block_entities.keys().copied())
        .filter(|pos| named(state, *pos, "beacon"))
        .collect();
    if positions.is_empty() {
        return;
    }
    // Effects target players only; do not walk the (potentially huge) mob table.
    let players: Vec<_> = state
        .players
        .keys()
        .filter_map(|entity| state.entities.slot_of(*entity))
        .collect();
    positions.sort_unstable_by_key(|pos| (pos.x, pos.y, pos.z));
    for pos in positions {
        let level = levels(state, pos);
        let mut fields = image(state, pos);
        let primary = selection(&fields, "primary_effect");
        let secondary = selection(&fields, "secondary_effect");
        if !fields
            .iter()
            .any(|(key, tag)| key == "Levels" && *tag == Tag::Int(level))
        {
            fields.retain(|(key, _)| key != "Levels");
            fields.push(("Levels".into(), Tag::Int(level)));
            store(state, pos, fields);
        }
        if let Some(primary) = primary.filter(|id| allowed(*id, level)) {
            let radius = f64::from(10 + 10 * level);
            let duration = (9 + 2 * level) * 20;
            let amplifier = u8::from(level == 4 && secondary == Some(primary));
            for &slot in &players {
                let row = state.entities.body(slot);
                if !row.alive {
                    continue;
                }
                let entity = row.id;
                let [x, y, z] = state.entities.pos(slot);
                if x < f64::from(pos.x) - radius
                    || x >= f64::from(pos.x + 1) + radius
                    || z < f64::from(pos.z) - radius
                    || z >= f64::from(pos.z + 1) + radius
                    || y < f64::from(pos.y) - radius
                {
                    continue;
                }
                if let Some(player) = state.players.get_mut(&entity) {
                    let connection = player.connection;
                    let mut effect = ActiveEffect::new(primary, duration, amplifier);
                    effect.ambient = true;
                    let mut updates = player.give_effect(effect);
                    if level == 4 && secondary == Some(EffectId::Regeneration) {
                        let mut regen = ActiveEffect::new(EffectId::Regeneration, duration, 0);
                        regen.ambient = true;
                        updates.extend(player.give_effect(regen));
                    }
                    for update in updates {
                        state.push_player(connection, update);
                    }
                    crate::effects::sync_appearance(state, entity);
                }
            }
        }
        let viewers: Vec<_> = state
            .open_containers
            .iter()
            .filter(|(_, at)| **at == (pos.x, pos.y, pos.z))
            .map(|(entity, _)| *entity)
            .collect();
        for entity in viewers {
            properties(state, entity, level, primary, secondary);
        }
    }
}
