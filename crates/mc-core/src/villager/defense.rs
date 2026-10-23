//! Village defense targets; friendly villagers and creepers are never attacked.
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::{
    boundary::{ChunkPos, EntityId},
    entities::EntityKind,
    tick::TickState,
};

/// Reputation at or below which villagers treat a nearby player as hostile.
const HOSTILE_REPUTATION: i32 = -100;

fn gossip_weight(kind: &str) -> i32 {
    match kind {
        "major_negative" => -5,
        "minor_negative" => -1,
        "major_positive" => 5,
        "minor_positive" | "trading" => 1,
        _ => 0,
    }
}

pub(crate) fn targets(state: &mut TickState) -> Vec<(EntityId, [f64; 3])> {
    // Keep the previous player order at publication, but obtain every candidate
    // from indices that are maintained at spawn, removal and compaction.
    let mut eligible: HashMap<uuid::Uuid, (EntityId, [f64; 3])> = HashMap::default();
    let mut by_id: HashSet<EntityId> = HashSet::default();
    for (&player_id, player) in &state.players {
        if player.creative() || player.spectator() || state.session_locked(player.connection) {
            continue;
        }
        let Some(player_slot) = state.entities.slot_of(player_id) else {
            continue;
        };
        eligible.insert(
            state.entities.row(player_slot).uuid,
            (player_id, state.entities.pos(player_slot)),
        );
        by_id.insert(player_id);
    }

    {
        let chunks = &mut state.villager_pois.defense_chunks;
        chunks.clear();
        for &(_, at) in eligible.values() {
            let x = (at[0].floor() as i32).div_euclid(16);
            let z = (at[2].floor() as i32).div_euclid(16);
            for x in x - 2..=x + 2 {
                for z in z - 2..=z + 2 {
                    chunks.insert(ChunkPos { x, z });
                }
            }
        }
    }
    {
        let registry = &state.registry;
        let slots = &mut state.villager_pois.defense_slots;
        state.entities.mob_slots_matching_into(
            |type_id| {
                registry.blueprint(type_id).is_some_and(|blueprint| {
                    (blueprint.category == crate::blueprint::MobCategory::Monster
                        && !blueprint.capabilities.ignored_by_monster_hunters)
                        || blueprint.capabilities.hunts_monsters
                })
            },
            slots,
        );
    }

    let defense_slots = &state.villager_pois.defense_slots;
    let mut out = Vec::new();
    for &slot in defense_slots {
        let body = state.entities.body(slot);
        if !body.alive {
            continue;
        }
        let EntityKind::Mob { type_id } = body.kind else {
            continue;
        };
        if state.registry.blueprint(type_id).is_some_and(|blueprint| {
            blueprint.category == crate::blueprint::MobCategory::Monster
                && !blueprint.capabilities.ignored_by_monster_hunters
        }) {
            out.push((body.id, state.entities.pos(slot)));
        }
    }

    let mut hostile: HashSet<EntityId> = HashSet::default();
    if !eligible.is_empty() {
        let mut reputation: HashMap<uuid::Uuid, i32> = HashMap::default();
        for chunk in &state.villager_pois.defense_chunks {
            let Some(ids) = state.entities.villager_chunks().get(chunk) else {
                continue;
            };
            for &id in ids {
                let Some(slot) = state.entities.slot_of(id) else {
                    continue;
                };
                let body = state.entities.body(slot);
                let row = state.entities.row(slot);
                if !body.alive {
                    continue;
                }
                let Some(villager) = &row.villager else {
                    continue;
                };
                if villager.conversion_time.is_some()
                    || !villager
                        .gossips
                        .iter()
                        .any(|gossip| eligible.contains_key(&gossip.target))
                {
                    continue;
                }
                reputation.clear();
                for gossip in &villager.gossips {
                    if eligible.contains_key(&gossip.target) {
                        let term = gossip.value.saturating_mul(gossip_weight(&gossip.kind));
                        *reputation.entry(gossip.target).or_insert(0) += term;
                    }
                }
                let at = state.entities.pos(slot);
                for (uuid, &total) in &reputation {
                    let (player_id, player_at) = eligible[uuid];
                    if total <= HOSTILE_REPUTATION
                        && at
                            .iter()
                            .zip(player_at)
                            .map(|(a, b)| (a - b).powi(2))
                            .sum::<f64>()
                            <= 32.0 * 32.0
                    {
                        hostile.insert(player_id);
                    }
                }
            }
        }
        for &slot in defense_slots {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            if body.alive
                && matches!(body.kind, EntityKind::Mob { type_id } if state.registry.blueprint(type_id).is_some_and(|b| b.capabilities.hunts_monsters))
                && let Some((attacker, at)) = row.last_player_hit
                && by_id.contains(&attacker)
                && state.tick.saturating_sub(at) < 100
            {
                hostile.insert(attacker);
            }
        }
    }
    for &player_id in state.players.keys() {
        if hostile.contains(&player_id)
            && let Some(player_slot) = state.entities.slot_of(player_id)
        {
            out.push((player_id, state.entities.pos(player_slot)));
        }
    }
    out
}
