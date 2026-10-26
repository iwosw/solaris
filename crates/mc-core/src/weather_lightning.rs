//! Lightning's bounded block effects; entity damage remains projectile intents.
use crate::boundary::BlockPos;
use crate::tick::TickState;

pub(crate) fn rod(state: &TickState, pos: BlockPos) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| block.block.id.path().ends_with("lightning_rod"))
}

pub(crate) fn strike_blocks(state: &mut TickState, position: [f64; 3]) {
    let [x, y, z] = position.map(|v| v.floor() as i32);
    let origin = BlockPos { x, y, z };
    let below = BlockPos { y: y - 1, ..origin };
    let conductor = [origin, below].into_iter().find(|pos| rod(state, *pos));
    if let Some(pos) = conductor {
        if crate::plugin_player::protects_cell(state, pos) {
            return;
        }
        let current = state.world.block_at(pos.x, pos.y, pos.z).unwrap();
        let block = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(current))
            .unwrap();
        let next = block.block.states.iter().find_map(|id| {
            let candidate = state.registry.blocks.by_id(*id)?;
            block
                .properties
                .iter()
                .all(|(key, value)| {
                    candidate.properties.iter().any(|(other_key, other_value)| {
                        key == other_key
                            && other_value == if key == "powered" { "true" } else { value }
                    })
                })
                .then_some(id.0)
        });
        if let Some(next) = next {
            state.write_block(pos, next);
            crate::redstone::schedule_cell(state, pos, 8, 4);
        }
        return;
    }
    if !matches!(
        state.difficulty,
        crate::survival::Difficulty::Normal | crate::survival::Difficulty::Hard
    ) {
        return;
    }
    let mut rng = crate::natural_spawn::Rng(
        state.world.seed() as u64
            ^ state.tick
            ^ (x as u64).wrapping_mul(0x9E37_79B9)
            ^ (z as u64).wrapping_mul(0x85EB_CA6B),
    );
    for attempt in 0..5 {
        let pos = if attempt == 0 {
            origin
        } else {
            BlockPos {
                x: x + rng.below(3) as i32 - 1,
                y: y + rng.below(3) as i32 - 1,
                z: z + rng.below(3) as i32 - 1,
            }
        };
        if crate::fire::spread_into_enabled(state, pos)
            && crate::fire::ignite_block(state, pos)
            && let Some(next) = state.world.block_at(pos.x, pos.y, pos.z)
        {
            state.merge_block_writes.push((pos, next));
        }
    }
}

pub(crate) fn channel_rod(
    state: &mut TickState,
    record: &crate::projectiles::ProjectileState,
    pos: BlockPos,
) {
    if record.kind != crate::projectiles::ProjectileKind::Trident
        || !rod(state, pos)
        || !record
            .stack
            .enchantments
            .iter()
            .any(|entry| entry.id.as_str() == "minecraft:channeling" && entry.level > 0)
        || !state.dimension.has_weather()
        || !crate::conduit::sky_exposed(
            state,
            [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)],
        )
        || state.weather.0 != crate::commands::Weather::Thunder
    {
        return;
    }
    let bolt = crate::projectiles::ProjectileState::new(
        crate::projectiles::ProjectileKind::Lightning,
        record.owner,
        mc_data::ItemStack::EMPTY,
    );
    crate::projectiles::launch(
        state,
        bolt,
        [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y + 1),
            f64::from(pos.z) + 0.5,
        ],
        [0.0; 3],
    );
}

/// Accepted hits mutate retained native entity state. Transformations replace
/// the species without a death/drop event and consume the ordinary bolt damage.
pub(crate) fn strike_entity(state: &mut TickState, id: crate::boundary::EntityId) -> bool {
    let Some(slot) = state.entities.slot_of(id) else {
        return false;
    };
    let crate::entities::EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return false;
    };
    let Some(name) = state
        .registry
        .blueprint(type_id)
        .map(|blueprint| blueprint.name.as_str())
    else {
        return false;
    };
    let replacement = match name {
        "minecraft:creeper" => {
            let (body, row) = state.entities.parts_mut(slot);
            row.entity_life.charged_lightning = true;
            crate::entity_life::restore_appearance(body, row);
            state.appearance_changed.push(id);
            return false;
        }
        "minecraft:mooshroom" => {
            let (body, row) = state.entities.parts_mut(slot);
            row.entity_life.brown_mooshroom = !row.entity_life.brown_mooshroom;
            crate::entity_life::restore_appearance(body, row);
            state.appearance_changed.push(id);
            return true;
        }
        "minecraft:pig" => "minecraft:zombified_piglin",
        "minecraft:villager" if state.difficulty != crate::survival::Difficulty::Peaceful => {
            "minecraft:witch"
        }
        _ => return false,
    };
    let Some(next) = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(replacement).expect("static entity identifier"))
    else {
        return false;
    };
    let pig = replacement == "minecraft:zombified_piglin";
    let sword = pig
        .then(|| {
            state.registry.items.id_of(
                &mc_data::Identifier::parse("minecraft:golden_sword")
                    .expect("static item identifier"),
            )
        })
        .flatten();
    crate::mounts::death(state, slot);
    crate::mounts::dismount(state, id);
    crate::leash::detach(state, slot);
    let position = state.entities.pos(slot);
    let row = state.entities.row_mut(slot);
    if let Some(villager) = row.villager.as_mut() {
        crate::settlements::allocator::clear_pois(villager);
    }
    row.villager = None;
    let child_ticks = if pig { row.entity_life.child_ticks } else { 0 };
    row.entity_life = crate::entity_life::EntityLifeState {
        child_ticks,
        ..Default::default()
    };
    let appearance = std::sync::Arc::make_mut(&mut row.appearance);
    appearance.extra.clear();
    appearance.baby = pig && (appearance.baby || child_ticks > 0);
    if let Some(saddle) = appearance.saddle.take() {
        state.merge_life_drops.push((position, saddle));
    }
    if let Some(sword) = sword {
        appearance.equipment[0] = Some(mc_data::ItemStack::new(sword, 1));
    }
    crate::villager::conversion::replace_type(state, slot, next);
    true
}

/// Rare-strike POI search: at most 17×17 resident chunks, palette-gated. Never
/// loads terrain, and checks the authoritative overlay before accepting a rod.
pub(crate) fn attract(state: &TickState, origin: BlockPos) -> BlockPos {
    let is_rod = |id: u32| {
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(id))
            .is_some_and(|block| block.block.id.path().ends_with("lightning_rod"))
    };
    let mut best = None;
    let mut consider = |pos: BlockPos| {
        let distance = i64::from(pos.x - origin.x).pow(2)
            + i64::from(pos.y - origin.y).pow(2)
            + i64::from(pos.z - origin.z).pow(2);
        let key = (distance, pos.x, pos.y, pos.z);
        if distance <= 128 * 128
            && best.is_none_or(|known| key < known)
            && rod(state, pos)
            && crate::conduit::sky_exposed(
                state,
                [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)],
            )
        {
            best = Some(key);
        }
    };
    for cz in (origin.z - 128).div_euclid(16)..=(origin.z + 128).div_euclid(16) {
        for cx in (origin.x - 128).div_euclid(16)..=(origin.x + 128).div_euclid(16) {
            let Some(chunk) = state.world.chunk(mc_world::ChunkPos { x: cx, z: cz }) else {
                continue;
            };
            for (index, section) in chunk.sections.iter().enumerate() {
                let min_y = chunk.geometry().min_y() + index as i32 * 16;
                if min_y > origin.y + 128 || min_y + 15 < origin.y - 128 {
                    continue;
                }
                let matches = match section.palette() {
                    Some(palette) => palette.iter().any(|id| is_rod(id.0)),
                    None => is_rod(section.get(0, 0, 0).0),
                };
                if !matches {
                    continue;
                }
                for y in 0..16u8 {
                    for z in 0..16u8 {
                        for x in 0..16u8 {
                            if is_rod(section.get(x, y, z).0) {
                                consider(BlockPos {
                                    x: cx * 16 + i32::from(x),
                                    y: min_y + i32::from(y),
                                    z: cz * 16 + i32::from(z),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    for ((x, y, z), id) in state.world.overlay_entries() {
        if is_rod(id) && state.world.has_chunk(x.div_euclid(16), z.div_euclid(16)) {
            consider(BlockPos { x, y, z });
        }
    }
    best.map_or(origin, |(_, x, y, z)| BlockPos { x, y: y + 1, z })
}
