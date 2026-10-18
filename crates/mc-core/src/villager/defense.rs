//! Village defense targets; friendly villagers and creepers are never attacked.
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::{boundary::EntityId, entities::EntityKind, tick::TickState};

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

pub(crate) fn targets(state: &TickState) -> Vec<(EntityId, [f64; 3])> {
    let mut out = Vec::new();
    for slot in state.entities.slots() {
        let body = state.entities.body(slot);
        if !body.alive {
            continue;
        }
        let EntityKind::Mob { type_id } = body.kind else {
            continue;
        };
        if state.registry.blueprint(type_id).is_some_and(|b| {
            b.category == crate::blueprint::MobCategory::Monster && b.name != "minecraft:creeper"
        }) {
            out.push((body.id, state.entities.pos(slot)));
        }
    }
    // Players a village turns on: a villager within 32 blocks whose gossip
    // reputation toward them is hostile, or a golem they hit in the last
    // 100 ticks. One pass over entities (never one pass per player); the
    // result is set membership, so player order below stays `state.players`.
    let mut eligible: HashMap<uuid::Uuid, (EntityId, [f64; 3])> = HashMap::default();
    let mut by_id: HashSet<EntityId> = HashSet::default();
    for (&player_id, player) in &state.players {
        if player.creative() || player.spectator() {
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
    let mut hostile: HashSet<EntityId> = HashSet::default();
    if !eligible.is_empty() {
        let mut reputation: HashMap<uuid::Uuid, i32> = HashMap::default();
        for slot in state.entities.slots() {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            if !body.alive {
                continue;
            }
            if let Some(villager) = &row.villager
                && villager.conversion_time.is_none()
                && villager
                    .gossips
                    .iter()
                    .any(|gossip| eligible.contains_key(&gossip.target))
            {
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
            if matches!(body.kind, EntityKind::Mob { type_id } if state.registry.blueprint(type_id).is_some_and(|b| b.name == "minecraft:iron_golem"))
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
