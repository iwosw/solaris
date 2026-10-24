use super::*;
use crate::boundary::BlockPos;
use crate::entities::ColdRow;
use crate::projectiles::ProjectileKind;
use crate::tick::DamagePiece;

/// WitherBoss's custom drop is not in its (empty) vanilla loot table.
/// The caller owns the persisted death-handled guard and mob_drops rule.
pub(crate) fn death_drop(state: &mut TickState, slot: EntitySlot) -> Option<EntityId> {
    if !matches!(
        state.entities.row(slot).special_mob.as_deref(),
        Some(Retained::Wither { .. })
    ) {
        return None;
    }
    let star = item_id(state, "nether_star").expect("vanilla registry contains nether star");
    let at = state.entities.pos(slot);
    Some(crate::tick::spawn_item_entity(
        state,
        at,
        mc_data::ItemStack::new(star, 1),
    ))
}

pub(crate) fn damage_allowed(row: &ColdRow, piece: &DamagePiece) -> bool {
    let Some(&Retained::Wither { invulnerable }) = row.special_mob.as_deref() else {
        return true;
    };
    if piece.kind.bypasses_invulnerability() {
        return true;
    }
    if invulnerable > 0 {
        return false;
    }
    if matches!(
        piece.kind,
        DamageKind::Drowning | DamageKind::Poison | DamageKind::Wither
    ) {
        return false;
    }
    // The armor phase rejects arrows/tridents, not melee, magic or explosions.
    !(row.health <= 150.0 && piece.kind == DamageKind::Projectile)
}
pub(crate) fn run(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    targets: &[(EntityId, [f64; 3])],
    attack: &mut Attack,
) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    let Some(&Retained::Wither { invulnerable }) = state.entities.row(slot).special_mob.as_deref()
    else {
        return;
    };
    let wpos = state.entities.pos(slot);
    if !state.world.is_active(
        (wpos[0].floor() as i32).div_euclid(16),
        (wpos[2].floor() as i32).div_euclid(16),
    ) {
        return;
    }
    let pos = state.entities.pos(slot);
    if invulnerable > 0 {
        let next = invulnerable - 1;
        if let Some(Retained::Wither { invulnerable }) =
            state.entities.row_mut(slot).special_mob.as_deref_mut()
        {
            *invulnerable = next;
        }
        metadata(state, slot, 19, EntityDataValue::VarInt(next as i32));
        state.entities.set_vel(slot, [0.0; 3]);
        if next.is_multiple_of(10) {
            heal(state, slot, 10.0);
        }
        if next == 0 {
            let seq = state.next_seq(id);
            intents.push(Intent {
                actor: id,
                seq,
                kind: IntentKind::Explode {
                    center: [pos[0], pos[1] + 1.5, pos[2]],
                    power: 7.0,
                    mob_grief: Some(crate::command_rules::MobGrief::Wither),
                    kill_credit: crate::statistics::living_type(state, id)
                        .map(|type_id| (id, type_id)),
                },
                footprint: Footprint::default(),
            });
            attack.deadline = state.tick + 40;
        }
        return;
    }
    if state.tick.is_multiple_of(20) {
        heal(state, slot, 1.0);
    }
    let found = target(state, slot, targets, 20.0);
    for head in 0..3u8 {
        metadata(
            state,
            slot,
            16 + head,
            EntityDataValue::VarInt(found.map_or(0, |(target, _)| target.0)),
        );
    }
    if state.tick >= attack.deadline
        && let Some((_, at)) = found
    {
        let yaw = f64::from(state.entities.rot(slot)[0]).to_radians();
        for head in 0..3 {
            let offset = match head {
                1 => -1.3,
                2 => 1.3,
                _ => 0.0,
            };
            let from = [
                pos[0] + f64::from(mth::cos(yaw)) * offset,
                pos[1] + if head == 0 { 3.0 } else { 2.2 },
                pos[2] + f64::from(mth::sin(yaw)) * offset,
            ];
            shooters::shoot(
                state,
                id,
                ProjectileKind::WitherSkull,
                from,
                [at[0], at[1] + 0.81, at[2]],
                0.1,
                None,
            );
        }
        attack.deadline = state.tick + 40;
    }
    // Hurt block-break delay uses the admitted hit clock, not a polling blast;
    // protected cells are excluded even when their resistance is merely finite.
    if state
        .entities
        .body(slot)
        .hurt_at
        .is_some_and(|tick| state.tick.saturating_sub(tick) == 20)
        && crate::command_rules::mob_griefing(state, crate::command_rules::MobGrief::Wither)
    {
        let base = pos.map(|value| value.floor() as i32);
        for y in base[1]..=base[1] + 3 {
            for z in base[2] - 1..=base[2] + 1 {
                for x in base[0] - 1..=base[0] + 1 {
                    let Some(block) = state.world.block_at(x, y, z) else {
                        continue;
                    };
                    if breakable(state, block) {
                        let pos = BlockPos { x, y, z };
                        let seq = state.next_seq(id);
                        let mut footprint = Footprint::default();
                        footprint.block_expect.push((pos, block));
                        footprint.block_write.push((pos, 0));
                        intents.push(Intent {
                            actor: id,
                            seq,
                            kind: IntentKind::BreakBlock { pos },
                            footprint,
                        });
                    }
                }
            }
        }
    }
}
fn heal(state: &mut TickState, slot: EntitySlot, amount: f32) {
    if state.entities.row(slot).health >= 300.0 {
        return;
    }
    let row = state.entities.row_mut(slot);
    row.health = (row.health + amount).min(300.0);
    std::sync::Arc::make_mut(&mut row.appearance).health = row.health;
    state.appearance_changed.push(state.entities.body(slot).id);
}
fn breakable(state: &TickState, block: u32) -> bool {
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(block))
        .is_some_and(|block| {
            !matches!(
                block.block.id.path(),
                "air"
                    | "cave_air"
                    | "void_air"
                    | "bedrock"
                    | "barrier"
                    | "end_portal"
                    | "end_portal_frame"
                    | "end_gateway"
                    | "command_block"
                    | "chain_command_block"
                    | "repeating_command_block"
                    | "structure_block"
                    | "jigsaw"
                    | "moving_piston"
                    | "reinforced_deepslate"
            )
        })
}
/// Placement-triggered ritual. All seven cells are consumed with the birth in
/// the same journal tick; loaded templates and already completed claims cannot
/// duplicate a boss. Both orientations and soul sand/soil are accepted.
pub(crate) fn after_blocks(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>) {
    if state.difficulty == crate::survival::Difficulty::Peaceful {
        return;
    }
    let Some(type_id) = summons::type_id(state, "minecraft:wither") else {
        return;
    };
    let skulls: Vec<BlockPos> = writes
        .iter()
        .filter(|(_, block)| {
            name(state, *block).is_some_and(|name| {
                matches!(name, "wither_skeleton_skull" | "wither_skeleton_wall_skull")
            })
        })
        .map(|(pos, _)| *pos)
        .collect();
    for skull in skulls {
        for axis in [0usize, 2] {
            for shift in -1..=1 {
                let mut center = [skull.x, skull.y, skull.z];
                center[axis] += shift;
                let cell = |offset: i32, dy: i32| {
                    let mut at = center;
                    at[axis] += offset;
                    at[1] += dy;
                    BlockPos {
                        x: at[0],
                        y: at[1],
                        z: at[2],
                    }
                };
                let read = |pos: BlockPos| {
                    state
                        .world
                        .block_at(pos.x, pos.y, pos.z)
                        .and_then(|block| name(state, block))
                };
                if !(-1..=1).all(|i| {
                    read(cell(i, 0)).is_some_and(|name| {
                        matches!(name, "wither_skeleton_skull" | "wither_skeleton_wall_skull")
                    })
                }) || !(-1..=1).all(|i| {
                    read(cell(i, -1)).is_some_and(|name| matches!(name, "soul_sand" | "soul_soil"))
                }) || !read(cell(0, -2))
                    .is_some_and(|name| matches!(name, "soul_sand" | "soul_soil"))
                    || ![-1, 1].iter().all(|i| {
                        read(cell(*i, -2))
                            .is_some_and(|name| matches!(name, "air" | "cave_air" | "void_air"))
                    })
                {
                    continue;
                }
                let cells = [
                    cell(-1, 0),
                    cell(0, 0),
                    cell(1, 0),
                    cell(-1, -1),
                    cell(0, -1),
                    cell(1, -1),
                    cell(0, -2),
                ];
                for pos in cells {
                    state.world.set_block(pos.x, pos.y, pos.z, 0);
                    writes.push((pos, 0));
                    // Skull block entities are not kept under air at checkpoint.
                    state.world.set_opaque_block_entity(pos, None);
                    state.opaque_changes.push((pos, None));
                }
                let id = state.spawn_mob(
                    type_id,
                    [
                        f64::from(center[0]) + 0.5,
                        f64::from(center[1] - 2) + 0.55,
                        f64::from(center[2]) + 0.5,
                    ],
                    100.0,
                );
                let slot = state.entities.slot_of(id).expect("wither");
                state.entities.row_mut(slot).special_mob =
                    Some(Box::new(Retained::Wither { invulnerable: 220 }));
                metadata(state, slot, 19, EntityDataValue::VarInt(220));
                state.hostile.spawned.push(id);
                summoned_nearby(state, id);
            }
        }
    }
}
/// Vanilla construction observes players intersecting the new entity's box
/// inflated by five blocks, not a guessed placer.
pub(crate) fn summoned_nearby(state: &mut TickState, id: EntityId) {
    let slot = state.entities.slot_of(id).expect("summoned entity");
    let position = state.entities.pos(slot);
    let row = state.entities.body(slot);
    let width = row.half_width + 5.0;
    let min_y = position[1] - 5.0;
    let max_y = position[1] + row.height + 5.0;
    let nearby: Vec<_> = state
        .players
        .keys()
        .copied()
        .filter(|player| {
            state.entities.slot_of(*player).is_some_and(|player_slot| {
                let at = state.entities.pos(player_slot);
                let player = state.entities.body(player_slot);
                (at[0] - position[0]).abs() < width + player.half_width
                    && (at[2] - position[2]).abs() < width + player.half_width
                    && at[1] < max_y
                    && at[1] + player.height > min_y
            })
        })
        .collect();
    for player in nearby {
        let entity = crate::advancements::entity_snapshot(state, id);
        crate::advancements::trigger(
            state,
            player,
            "minecraft:summoned_entity",
            serde_json::json!({ "entity": entity }),
        );
    }
}
fn name(state: &TickState, block: u32) -> Option<&str> {
    Some(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(block))?
            .block
            .id
            .path(),
    )
}
