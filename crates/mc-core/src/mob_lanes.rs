//! Snow golem snowballs/trails and the overworld wandering-trader visit clock.
//! The clock uses native journal/checkpoint post-images; merchants and UUID
//! leashes use the existing entity NBT authority and Anvil projection.
#[cfg(test)]
#[path = "mob_lanes_tests.rs"]
pub(crate) mod tests;
#[path = "mob_lanes_trades.rs"]
pub(crate) mod trades;

use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::natural_spawn::Rng;
use crate::tick::TickState;
use mc_data::entity_types::MobCategory;
use mc_data::{Identifier, ItemStack};
use mc_physics::BlockMaterial;
use mc_physics::mth;
use serde::{Deserialize, Serialize};

static SNOW_GOLEM_TYPE: std::sync::LazyLock<Option<u32>> = std::sync::LazyLock::new(|| {
    mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:snow_golem")
        .map(|contract| contract.protocol_id)
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraderSpawner {
    /// Vanilla saved-data Codec.INT values, preserved without clamping at load.
    pub delay: i32,
    pub chance: i32,
}
impl Default for TraderSpawner {
    fn default() -> Self {
        Self {
            delay: 24_000,
            chance: 25,
        }
    }
}

pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot, type_id: u32) {
    if state
        .registry
        .blueprint(type_id)
        .is_none_or(|b| b.name != "minecraft:wandering_trader")
    {
        return;
    }
    let mut rng = Rng(state.world.seed() as u64 ^ state.entities.row(slot).uuid.as_u128() as u64);
    let offers = trades::generate(&state.registry, &mut rng);
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    row.villager = Some(Box::new(crate::villager::VillagerState {
        wandering: true,
        offers,
        ..Default::default()
    }));
}

pub(crate) fn journal(state: &mut TickState, entries: &mut Vec<crate::journal::JournalEntry>) {
    if state.dimension == crate::dimensions::Dimension::Overworld
        && state.journaled_trader_spawner != Some(state.trader_spawner)
    {
        entries.push(crate::journal::JournalEntry::WanderingTrader {
            value: state.trader_spawner,
        });
        state.journaled_trader_spawner = Some(state.trader_spawner);
    }
}

fn position_near(
    state: &TickState,
    center: [f64; 3],
    radius: i32,
    rng: &mut Rng,
) -> Option<[f64; 3]> {
    for _ in 0..10 {
        let x = center[0].floor() as i32 + rng.below((radius * 2) as u64) as i32 - radius;
        let z = center[2].floor() as i32 + rng.below((radius * 2) as u64) as i32 - radius;
        if let Some(y) = crate::natural_spawn::spawn_floor(state, x, z, MobCategory::Misc) {
            if state
                .world
                .biome_at(x, y, z)
                .is_some_and(|b| b.path() == "the_void")
            {
                continue;
            }
            return Some([f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5]);
        }
    }
    None
}

fn visit(state: &mut TickState, rng: &mut Rng, changes: &mut ChangeSet) -> bool {
    let mut players: Vec<_> = state
        .players
        .iter()
        .filter_map(|(&id, player)| {
            let slot = state.entities.slot_of(id)?;
            (!player.spectator() && state.entities.body(slot).alive).then_some(id)
        })
        .collect();
    players.sort_unstable_by_key(|id| id.0);
    if players.is_empty() {
        return false;
    }
    let player = players[rng.below(players.len() as u64) as usize];
    let center = state
        .entities
        .pos(state.entities.slot_of(player).expect("live player"));
    // The vanilla spawner adds a one-in-ten admission roll after the daily chance.
    if rng.below(10) != 0 {
        return false;
    }
    let Some(pos) = position_near(state, center, 48, rng) else {
        return false;
    };
    // Spawn placement respects the active set (e.g. sim0): a trader outside
    // the player sim radius must not materialize in a resident-inactive chunk.
    if !state.world.is_active(
        (pos[0].floor() as i32).div_euclid(16),
        (pos[2].floor() as i32).div_euclid(16),
    ) {
        return false;
    }
    let Some(llama_a) = position_near(state, pos, 4, rng) else {
        return false;
    };
    let Some(llama_b) = position_near(state, pos, 4, rng) else {
        return false;
    };
    let trader_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:wandering_trader").expect("vanilla type"));
    let llama_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:trader_llama").expect("vanilla type"));
    let (Some(trader_type), Some(llama_type)) = (trader_type, llama_type) else {
        return false;
    };
    let trader = state.spawn_mob(trader_type, pos, 20.0);
    let slot = state.entities.slot_of(trader).expect("new trader");
    let owner = state.entities.row(slot).uuid;
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .expect("merchant initialized")
        .despawn_at = Some(state.tick.saturating_add(48_000));
    changes.entities_spawned.push(trader);
    for pos in [llama_a, llama_b] {
        let llama = state.spawn_mob(llama_type, pos, 15.0);
        let slot = state.entities.slot_of(llama).expect("new llama");
        state.entities.row_mut(slot).persistent = true;
        state
            .entities
            .set_leash(slot, Some(crate::leash::Holder::Entity(owner)));
        changes.entities_spawned.push(llama);
    }
    true
}

/// Due, resident-active wandering traders in slot order: owners and slots.
/// Almost no body is a due trader, so a read-only filter (on the pool once
/// `rows >= min_parallel`) drops the rest before the serial activity check.
fn due_traders(
    state: &TickState,
    min_parallel: usize,
) -> (Vec<uuid::Uuid>, Vec<crate::entities::EntitySlot>) {
    let tick = state.tick;
    let entities = &state.entities;
    let due = |slot: &crate::entities::EntitySlot| {
        entities
            .row(*slot)
            .villager
            .as_ref()
            .is_some_and(|v| v.wandering && v.despawn_at.is_some_and(|at| at <= tick))
    };
    let rows = state.entities.len();
    let due_slots: Vec<crate::entities::EntitySlot> = if rows < min_parallel {
        state.entities.slots().filter(|slot| due(slot)).collect()
    } else {
        use rayon::prelude::*;
        (0..rows as u32)
            .into_par_iter()
            .with_min_len(2048)
            .map(crate::entities::EntitySlot)
            .filter(|slot| due(slot))
            .collect()
    };
    let (mut removed_owners, mut gone) = (Vec::new(), Vec::new());
    for slot in due_slots {
        // Expiry is live simulation: skip resident-inactive bodies.
        let tpos = state.entities.pos(slot);
        if !state.world.is_active(
            (tpos[0].floor() as i32).div_euclid(16),
            (tpos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        removed_owners.push(state.entities.row(slot).uuid);
        gone.push(slot);
    }
    (removed_owners, gone)
}

/// The previous activity-first dense scan, the oracle for [`due_traders`].
#[cfg(test)]
pub(crate) fn due_traders_by_scan(
    state: &TickState,
) -> (Vec<uuid::Uuid>, Vec<crate::entities::EntitySlot>) {
    let (mut removed_owners, mut gone) = (Vec::new(), Vec::new());
    for slot in state.entities.slots() {
        let tpos = state.entities.pos(slot);
        if !state.world.is_active(
            (tpos[0].floor() as i32).div_euclid(16),
            (tpos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let row = state.entities.row(slot);
        if !row
            .villager
            .as_ref()
            .is_some_and(|v| v.wandering && v.despawn_at.is_some_and(|at| at <= state.tick))
        {
            continue;
        }
        removed_owners.push(row.uuid);
        gone.push(slot);
    }
    (removed_owners, gone)
}

/// Despawn precedes the entity phase, so removals cannot invalidate intent slots.
pub(crate) fn traders(state: &mut TickState, changes: &mut ChangeSet) {
    let (removed_owners, mut gone) = due_traders(state, crate::contact_hazards::MIN_PARALLEL_ROWS);
    if !removed_owners.is_empty() {
        for slot in state.entities.slots() {
            // Linked-llama removal follows the same simulation freeze.
            let lpos = state.entities.pos(slot);
            if !state.world.is_active(
                (lpos[0].floor() as i32).div_euclid(16),
                (lpos[2].floor() as i32).div_euclid(16),
            ) {
                continue;
            }
            let row = state.entities.row(slot);
            if crate::mounts::name(state, slot) == "minecraft:trader_llama"
                && !row.entity_life.tamed
                && matches!(&row.entity_life.leash, Some(crate::leash::Holder::Entity(owner)) if removed_owners.contains(owner))
            {
                gone.push(slot);
            }
        }
        gone.sort_unstable_by_key(|slot| std::cmp::Reverse(slot.0));
        gone.dedup();
        for slot in gone {
            changes.entities_removed.push(state.entities.body(slot).id);
            state.remove_entity(slot);
        }
    }
    if state.dimension != crate::dimensions::Dimension::Overworld
        || !crate::command_rules::boolean(
            state,
            crate::command_rules::BoolRule::SPAWN_WANDERING_TRADERS,
        )
    {
        return;
    }
    // WanderingTraderSpawner's tickDelay is transient and restarts at 1200;
    // only the saved spawn delay/chance change at this minute boundary.
    state.trader_tick_delay -= 1;
    if state.trader_tick_delay != 0 {
        return;
    }
    state.trader_tick_delay = 1200;
    state.trader_spawner.delay = state.trader_spawner.delay.wrapping_sub(1200);
    if state.trader_spawner.delay > 0 {
        return;
    }
    let chance = state.trader_spawner.chance;
    state.trader_spawner.delay = 24_000;
    state.trader_spawner.chance = chance.wrapping_add(25).clamp(25, 75);
    let mut rng = Rng(state.world.seed() as u64 ^ state.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    if (rng.below(100) as i32) < chance && visit(state, &mut rng, changes) {
        state.trader_spawner.chance = 25;
    }
}

pub(crate) fn snowballs(state: &mut TickState) {
    let golems: Vec<_> = SNOW_GOLEM_TYPE
        .iter()
        .flat_map(|&type_id| state.entities.mob_slots(type_id))
        .collect();
    for slot in golems {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if !body.alive || row.controls.no_ai || !row.age.is_multiple_of(20) {
            continue;
        }
        let gpos = state.entities.pos(slot);
        if !state.world.is_active(
            (gpos[0].floor() as i32).div_euclid(16),
            (gpos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let owner = row.uuid;
        let at = state.entities.pos(slot);
        let muzzle = [at[0], at[1] + 1.5, at[2]];
        let target = state
            .entities
            .slots()
            .filter(|&other| {
                let body = state.entities.body(other);
                let row = state.entities.row(other);
                body.alive
                    && row.health > 0.0
                    && crate::natural_spawn::category_of(state, other) == MobCategory::Monster
            })
            .filter_map(|other| {
                let pos = state.entities.pos(other);
                let distance: f64 = (0..3).map(|axis| (at[axis] - pos[axis]).powi(2)).sum();
                let aim = [
                    pos[0],
                    pos[1] + state.entities.body(other).height * 0.5,
                    pos[2],
                ];
                (distance <= 100.0 && crate::hostile::sight_clear(state, muzzle, aim))
                    .then_some((other, aim, distance))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2).then_with(|| a.0.0.cmp(&b.0.0)));
        let Some((_, aim, _)) = target else { continue };
        let Some(item) = state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:snowball").expect("vanilla item"))
        else {
            continue;
        };
        let delta = [aim[0] - muzzle[0], aim[1] - muzzle[1], aim[2] - muzzle[2]];
        let horizontal = mth::length2(delta[0], delta[2]);
        let aimed = [delta[0], delta[1] + horizontal * 0.2, delta[2]];
        let length = aimed.iter().map(|v| v * v).sum::<f64>().sqrt().max(0.001);
        let projectile = crate::projectiles::ProjectileState::new(
            crate::projectiles::ProjectileKind::Snowball,
            Some(owner),
            ItemStack::new(item, 1),
        );
        crate::projectiles::launch(state, projectile, muzzle, aimed.map(|v| v / length * 1.6));
    }
}

pub(crate) fn snow_trails(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>) {
    let snow = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:snow").expect("vanilla block"))
        .map(|b| b.default.0);
    let golems: Vec<_> = SNOW_GOLEM_TYPE
        .iter()
        .flat_map(|&type_id| state.entities.mob_slots(type_id))
        .collect();
    for slot in golems {
        if !state.entities.body(slot).alive
            || crate::mounts::name(state, slot) != "minecraft:snow_golem"
        {
            continue;
        }
        let tpos = state.entities.pos(slot);
        if !state.world.is_active(
            (tpos[0].floor() as i32).div_euclid(16),
            (tpos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let at = state.entities.pos(slot);
        let [x, y, z] = at.map(|v| v.floor() as i32);
        let biome = state.world.biome_at(x, y, z).map_or("", |b| b.path());
        let hot = matches!(
            biome,
            "desert"
                | "badlands"
                | "eroded_badlands"
                | "wooded_badlands"
                | "savanna"
                | "savanna_plateau"
                | "windswept_savanna"
        ) || state.dimension == crate::dimensions::Dimension::Nether;
        let cold = matches!(
            biome,
            "snowy_plains"
                | "ice_spikes"
                | "snowy_taiga"
                | "snowy_beach"
                | "frozen_river"
                | "frozen_ocean"
                | "deep_frozen_ocean"
                | "grove"
                | "snowy_slopes"
                | "frozen_peaks"
                | "jagged_peaks"
        );
        let rain = state.dimension.has_weather()
            && state.weather.0 != crate::commands::Weather::Clear
            && !cold
            && !hot
            && (y + 2..state.dimension.geometry().max_y()).all(|above| {
                state.world.block_at(x, above, z).is_some_and(|block| {
                    state.registry.materials.classify(block) == BlockMaterial::Air
                })
            });
        if hot || rain {
            state.merge_damage.push((
                slot,
                crate::tick::DamagePiece {
                    physical: None,
                    custom_armor: false,
                    amount: 1.0,
                    kind: crate::survival::DamageKind::Generic,
                    source: None,
                },
            ));
        }
        if !cold
            || state.entities.row(slot).controls.no_ai
            || !crate::command_rules::boolean(state, crate::command_rules::BoolRule::MOB_GRIEFING)
        {
            continue;
        }
        let Some(snow) = snow else { continue };
        for (dx, dz) in [(-0.25, -0.25), (-0.25, 0.25), (0.25, -0.25), (0.25, 0.25)] {
            let pos = BlockPos {
                x: (at[0] + dx).floor() as i32,
                y,
                z: (at[2] + dz).floor() as i32,
            };
            if state.world.block_at(pos.x, y, pos.z) == Some(state.registry.states.air)
                && state
                    .world
                    .block_at(pos.x, y - 1, pos.z)
                    .is_some_and(|block| {
                        state.registry.materials.classify(block) == BlockMaterial::Solid
                    })
            {
                state.world.set_block(pos.x, y, pos.z, snow);
                writes.push((pos, snow));
            }
        }
    }
}
