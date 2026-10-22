//! `defense::targets` must equal the per-player linear scan it replaced.

use std::sync::Arc;

use crate::boundary::{ConnectionId, EntityId, PlayerInput};
use crate::commands::GameMode;
use crate::entities::EntityKind;
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};

/// The replaced implementation: one scan over every entity per player.
fn linear_targets(state: &TickState) -> Vec<(EntityId, [f64; 3])> {
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
    for (&player_id, player) in &state.players {
        if player.creative() || player.spectator() {
            continue;
        }
        let Some(player_slot) = state.entities.slot_of(player_id) else {
            continue;
        };
        let uuid = state.entities.row(player_slot).uuid;
        let hostile = state.entities.slots().any(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            if !body.alive {
                return false;
            }
            let near = state
                .entities
                .pos(slot)
                .iter()
                .zip(state.entities.pos(player_slot))
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                <= 32.0 * 32.0;
            if let Some(villager) = &row.villager
                && villager.conversion_time.is_none()
                && near
            {
                let reputation: i32 = villager
                    .gossips
                    .iter()
                    .filter(|g| g.target == uuid)
                    .map(|g| {
                        g.value.saturating_mul(match g.kind.as_str() {
                            "major_negative" => -5,
                            "minor_negative" => -1,
                            "major_positive" => 5,
                            "minor_positive" | "trading" => 1,
                            _ => 0,
                        })
                    })
                    .sum();
                if reputation <= -100 {
                    return true;
                }
            }
            matches!(body.kind, EntityKind::Mob { type_id } if state.registry.blueprint(type_id).is_some_and(|b| b.name == "minecraft:iron_golem"))
                && row
                    .last_player_hit
                    .is_some_and(|(attacker, at)| attacker == player_id && state.tick.saturating_sub(at) < 100)
        });
        if hostile {
            out.push((player_id, state.entities.pos(player_slot)));
        }
    }
    out
}

/// splitmix64: seeded, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn type_id(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).expect("id parses"))
        .expect("entity type")
}

#[test]
fn village_targets_match_linear_scan_on_random_layouts() {
    let mut rng = Rng(0x5EED_0003);
    let registry = Arc::new(CoreRegistry::load(None).expect("registry loads"));
    let mut state = TickState::new(
        Arc::clone(&registry),
        crate::horde_bench::flat_world(&registry),
        2,
        512,
    );
    let joins: Vec<_> = (1..=12)
        .map(|connection| PlayerInput::Join {
            connection: ConnectionId(connection),
            uuid: uuid::Uuid::new_v4(),
            name: format!("p{connection}"),
            permission_level: 0,
        })
        .collect();
    run_tick(&mut state, &joins);
    let players: Vec<EntityId> = (1..=12)
        .map(|connection| {
            state
                .connections
                .entity_of(ConnectionId(connection))
                .expect("joined")
        })
        .collect();
    let uuids: Vec<uuid::Uuid> = players
        .iter()
        .map(|id| {
            state
                .entities
                .row(state.entities.slot_of(*id).unwrap())
                .uuid
        })
        .collect();
    let villager = type_id(&state, "minecraft:villager");
    let golem = type_id(&state, "minecraft:iron_golem");
    let zombie = type_id(&state, "minecraft:zombie");
    let creeper = type_id(&state, "minecraft:creeper");
    let cow = type_id(&state, "minecraft:cow");
    let kinds = [
        "major_negative",
        "minor_negative",
        "major_positive",
        "minor_positive",
        "trading",
        "unknown",
    ];
    // Defense targeting must ignore a trading hall's unrelated livestock
    // population while preserving its exact target result.
    for index in 0..1_024 {
        state.spawn_mob(cow, [f64::from(index), 64.0, 256.0], 10.0);
    }
    for round in 0..8 {
        for (index, id) in players.iter().enumerate() {
            let slot = state.entities.slot_of(*id).unwrap();
            state
                .entities
                .set_pos(slot, [rng.below(120) as f64, 64.0, rng.below(120) as f64]);
            state.players.get_mut(id).unwrap().game_mode = match (index + round) % 6 {
                0 => GameMode::Creative,
                1 => GameMode::Spectator,
                _ => GameMode::Survival,
            };
        }
        state.tick = 1_000 + round as u64;
        for _ in 0..120 {
            let at = [rng.below(120) as f64, 64.0, rng.below(120) as f64];
            let id = state.spawn_mob(villager, at, 20.0);
            let slot = state.entities.slot_of(id).unwrap();
            let (body, row) = state.entities.parts_mut(slot);
            body.alive = rng.below(10) != 0;
            let record = row.villager.as_mut().expect("villager state");
            if rng.below(8) == 0 {
                record.conversion_time = Some(100);
            }
            for _ in 0..rng.below(12) {
                let target = if rng.below(6) == 0 {
                    uuid::Uuid::new_v4()
                } else {
                    uuids[rng.below(uuids.len() as u64) as usize]
                };
                record.gossips.push(crate::villager::Gossip {
                    target,
                    kind: kinds[rng.below(kinds.len() as u64) as usize].to_owned(),
                    value: rng.below(40) as i32,
                });
            }
        }
        for _ in 0..10 {
            let at = [rng.below(120) as f64, 64.0, rng.below(120) as f64];
            let id = state.spawn_mob(golem, at, 100.0);
            let slot = state.entities.slot_of(id).unwrap();
            let attacker = players[rng.below(players.len() as u64) as usize];
            state.entities.row_mut(slot).last_player_hit =
                Some((attacker, state.tick - rng.below(160)));
        }
        for _ in 0..30 {
            let at = [rng.below(120) as f64, 64.0, rng.below(120) as f64];
            state.spawn_mob(if rng.below(2) == 0 { zombie } else { creeper }, at, 20.0);
        }
        let expected = linear_targets(&state);
        assert_eq!(
            crate::villager::defense::targets(&mut state),
            expected,
            "round {round}"
        );
        if round == 7 {
            let capacity = state.villager_pois.defense_slots.capacity();
            assert_eq!(
                crate::villager::defense::targets(&mut state),
                expected,
                "reused type-lane scratch changed targets"
            );
            assert_eq!(state.villager_pois.defense_slots.capacity(), capacity);
        }
        assert!(
            expected.iter().any(|(id, _)| players.contains(id)),
            "round {round}: the layout must make at least one player hostile"
        );
    }
}
