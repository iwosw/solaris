//! Vanilla conduit frame/effect cadence; the client derives activation from blocks.
use crate::boundary::{BlockPos, ChunkPos, PlayerChange};
use crate::effects::{ActiveEffect, EffectId};
use crate::entities::{EntityKind, EntitySlot};
use crate::intent::{Footprint, Intent, IntentKind};
use crate::tick::TickState;
use mc_nbt::Tag;
use uuid::Uuid;

fn named(state: &TickState, pos: BlockPos, name: &str) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| block.block.id.path() == name)
}

pub(crate) fn water(state: &TickState, pos: [f64; 3]) -> bool {
    let [x, y, z] = pos.map(|v| v.floor() as i32);
    state
        .world
        .block_at(x, y, z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| {
            block.block.id.path() == "water"
                || block
                    .properties
                    .iter()
                    .any(|(key, value)| key == "waterlogged" && value == "true")
        })
}

pub(crate) fn wet(state: &TickState, pos: [f64; 3]) -> bool {
    if water(state, pos) {
        return true;
    }
    rain_exposed(state, pos)
}

/// Rain reaches only an exposed column (vanilla MOTION_BLOCKING heightmap:
/// collision or fluid blocks rain; glass/leaves do, torches/flowers don't).
/// Unknown columns fail closed.
pub(crate) fn rain_exposed(state: &TickState, pos: [f64; 3]) -> bool {
    if !state.dimension.has_weather() || state.weather.0 == crate::commands::Weather::Clear {
        return false;
    }
    let [x, y, z] = pos.map(|v| v.floor() as i32);
    crate::weather::rains_at(state, BlockPos { x, y, z }) && sky_exposed(state, pos)
}

pub(crate) fn sky_exposed(state: &TickState, pos: [f64; 3]) -> bool {
    let [x, y, z] = pos.map(|v| v.floor() as i32);
    let Some(chunk) = state.world.chunk(ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
    }) else {
        return false;
    };
    (y + 1..chunk.geometry().max_y()).all(|above| {
        state.world.block_at(x, above, z).is_some_and(|id| {
            state.registry.materials.classify(id) == mc_physics::BlockMaterial::Air
        })
    })
}

pub(crate) fn frame_cell(x: i32, y: i32, z: i32) -> bool {
    (x.abs() > 1 || y.abs() > 1 || z.abs() > 1)
        && ((x == 0 && (y.abs() == 2 || z.abs() == 2))
            || (y == 0 && (x.abs() == 2 || z.abs() == 2))
            || (z == 0 && (x.abs() == 2 || y.abs() == 2)))
}

pub(crate) fn frame_size(state: &TickState, pos: BlockPos) -> usize {
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                if !water(
                    state,
                    [
                        f64::from(pos.x + x),
                        f64::from(pos.y + y),
                        f64::from(pos.z + z),
                    ],
                ) {
                    return 0;
                }
            }
        }
    }
    let mut size = 0;
    for x in -2..=2 {
        for y in -2..=2 {
            for z in -2..=2 {
                if frame_cell(x, y, z)
                    && state
                        .world
                        .block_at(pos.x + x, pos.y + y, pos.z + z)
                        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                        .is_some_and(|block| {
                            matches!(
                                block.block.id.path(),
                                "prismarine"
                                    | "prismarine_bricks"
                                    | "dark_prismarine"
                                    | "sea_lantern"
                            )
                        })
                {
                    size += 1;
                }
            }
        }
    }
    size
}

fn image(state: &TickState, pos: BlockPos) -> Result<Option<Uuid>, String> {
    let chunk = state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .ok_or_else(|| "conduit chunk is unavailable".to_owned())?;
    let Some(bytes) = chunk.block_entities.get(&pos) else {
        return Ok(None);
    };
    let mut reader = bytes.as_slice();
    let Tag::Compound(fields) =
        mc_nbt::read_network(&mut reader).map_err(|err| format!("conduit NBT: {err}"))?
    else {
        return Err("conduit NBT: expected TAG_Compound".into());
    };
    if !reader.is_empty() {
        return Err("conduit NBT: trailing bytes".into());
    }
    for (key, value) in [
        ("id", Tag::String("minecraft:conduit".into())),
        ("x", Tag::Int(pos.x)),
        ("y", Tag::Int(pos.y)),
        ("z", Tag::Int(pos.z)),
    ] {
        let mut values = fields.iter().filter(|(name, _)| name == key);
        if values.next().map(|(_, tag)| tag) != Some(&value) || values.next().is_some() {
            return Err(format!(
                "conduit ({},{},{}): invalid {key}",
                pos.x, pos.y, pos.z
            ));
        }
    }
    mc_data::conduit_nbt::target(&fields)
}

pub(crate) fn validate_loaded(state: &TickState) -> Result<(), String> {
    state
        .world
        .chunks()
        .try_for_each(|chunk| validate_chunk(state, chunk))
}

pub(crate) fn validate_chunk(state: &TickState, chunk: &mc_world::Chunk) -> Result<(), String> {
    for pos in chunk.block_entities.keys() {
        if named(state, *pos, "conduit") {
            image(state, *pos)?;
        }
    }
    Ok(())
}

fn store(state: &mut TickState, pos: BlockPos, target: Option<Uuid>, sync: bool) {
    let update = Tag::Compound(mc_data::conduit_nbt::fields(target));
    let Tag::Compound(mut fields) = update.clone() else {
        unreachable!()
    };
    fields.extend([
        ("id".into(), Tag::String("minecraft:conduit".into())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("conduit NBT encodes");
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
    if sync {
        state.world.mark_block_entity_visible(pos);
        let viewers: Vec<_> = state.players.values().map(|p| p.connection).collect();
        for connection in viewers {
            state.push_player(
                connection,
                PlayerChange::BlockEntityData {
                    position: pos,
                    block_entity_type: mc_data::block_entity_kinds::BlockEntityKind::Conduit
                        .protocol_id(),
                    nbt: update.clone(),
                },
            );
        }
    }
}

/// Placement creates the vanilla BE even before its first activation scan.
pub(crate) fn after_blocks(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    for (pos, _) in writes {
        if named(state, *pos, "conduit") {
            let exists = state
                .world
                .chunk(ChunkPos {
                    x: pos.x.div_euclid(16),
                    z: pos.z.div_euclid(16),
                })
                .is_some_and(|c| c.block_entities.contains_key(pos));
            if !exists {
                store(state, *pos, None, false);
            }
        }
    }
}

fn distance(pos: BlockPos, other: [f64; 3]) -> f64 {
    [pos.x, pos.y, pos.z]
        .into_iter()
        .zip(other)
        .map(|(a, b)| (f64::from(a) - b.floor()).powi(2))
        .sum()
}

pub(crate) fn run(state: &mut TickState, intents: &mut Vec<Intent>) {
    if !state.tick.is_multiple_of(40) {
        return;
    }
    let mut positions: Vec<_> = state
        .world
        .active_positions()
        .filter_map(|pos| state.world.chunk(pos))
        .flat_map(|chunk| chunk.block_entities.keys().copied())
        .filter(|pos| named(state, *pos, "conduit"))
        .collect();
    positions.sort_unstable_by_key(|pos| (pos.x, pos.y, pos.z));
    for pos in positions {
        let size = frame_size(state, pos);
        let old = image(state, pos).unwrap_or_else(|err| panic!("{err}"));
        if size >= 16 {
            let radius = (size / 7 * 16) as f64;
            for index in 0..state.entities.len() {
                let slot = EntitySlot(index as u32);
                let body = state.entities.body(slot);
                let entity = body.id;
                if !body.alive
                    || !matches!(body.kind, EntityKind::Player { .. })
                    || distance(pos, state.entities.pos(slot)) >= radius * radius
                    || !wet(state, state.entities.pos(slot))
                {
                    continue;
                }
                let mut effect = ActiveEffect::new(EffectId::ConduitPower, 260, 0);
                effect.ambient = true;
                if let Some(player) = state.players.get_mut(&entity) {
                    let connection = player.connection;
                    let updates = player.give_effect(effect);
                    for update in updates {
                        state.push_player(connection, update);
                    }
                    crate::effects::sync_appearance(state, entity);
                }
            }
        }
        let valid = |slot: EntitySlot| {
            let body = state.entities.body(slot);
            body.alive && distance(pos, state.entities.pos(slot)) < 64.0
        };
        let target = if size < 42 {
            None
        } else if let Some(uuid) = old {
            state
                .entities
                .slot_of_uuid(uuid)
                .filter(|slot| valid(*slot))
        } else {
            state
                .entities
                .slots()
                .filter(|slot| valid(*slot) && wet(state, state.entities.pos(*slot)))
                .filter(|slot| match state.entities.body(*slot).kind {
                    EntityKind::Mob { type_id } => state
                        .registry
                        .blueprint(type_id)
                        .is_some_and(|b| b.category == crate::blueprint::MobCategory::Monster),
                    _ => false,
                })
                .min_by_key(|slot| state.entities.body(*slot).id)
        };
        let uuid = target.map(|slot| state.entities.row(slot).uuid);
        if uuid != old {
            store(state, pos, uuid, true);
        }
        if let Some(slot) = target {
            let entity = state.entities.body(slot).id;
            let seq = state.next_seq(entity);
            intents.push(Intent {
                actor: entity,
                seq,
                kind: IntentKind::DamageEntity {
                    target: entity,
                    amount: 4.0,
                    kind: crate::survival::DamageKind::Magic,
                    source: None,
                },
                footprint: Footprint {
                    entity_reads: vec![entity],
                    entity_writes: vec![entity],
                    ..Default::default()
                },
            });
        }
    }
}

pub(crate) fn deserialize_nbt<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<u8>>, D::Error> {
    use serde::Deserialize;
    let bytes = Option::<Vec<u8>>::deserialize(deserializer)?;
    if let Some(bytes) = &bytes {
        mc_data::conduit_nbt::validate(bytes).map_err(serde::de::Error::custom)?;
        crate::sculk::validate_record(bytes).map_err(serde::de::Error::custom)?;
        crate::shrieker::validate_record(bytes).map_err(serde::de::Error::custom)?;
        let fields = mc_data::trial_nbt::decode(bytes).map_err(serde::de::Error::custom)?;
        mc_data::trial_nbt::validate(&fields, None).map_err(serde::de::Error::custom)?;
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "conduit_tests.rs"]
mod tests;
