use super::*;
use crate::boundary::{ChunkPos, ConnectionId, EntityId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use std::sync::{Arc, LazyLock};

struct Empty(u32, &'static str);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.0),
            Identifier::parse(self.1).unwrap(),
        )
    }
}
pub(crate) fn fixture(biome: &'static str) -> TickState {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let mut world = ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air, biome)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 80, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(
        &(-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| (ChunkPos { x, z }, 0)))
            .collect(),
    );
    let stone = registry
        .blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    for x in -16..32 {
        for z in -16..32 {
            world.set_block(x, 79, z, stone);
        }
    }
    world.fold_overlay();
    let mut state = TickState::new(registry, world, 1, 512);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    // Strict radius split: fixture intent carries a ticket; activation flows
    // through the same refresh run_tick uses (tickets simulate, Main binding).
    state
        .tickets
        .insert("lanes-test".into(), (ChunkPos { x: 0, z: 0 }, 1));
    state.refresh_active_chunks(state.tick);
    state
}
fn spawn(state: &mut TickState, name: &str, pos: [f64; 3]) -> EntityId {
    let kind = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(name).unwrap())
        .unwrap();
    state.spawn_mob(
        kind,
        pos,
        state.registry.blueprint(kind).unwrap().params.max_health,
    )
}
pub(crate) fn join(state: &mut TickState) -> EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "observer".into(),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    state
        .entities
        .set_pos(state.entities.slot_of(player).unwrap(), [8.5, 80.0, 8.5]);
    player
}
pub(crate) fn spawn_visit(state: &mut TickState) -> Vec<EntityId> {
    for attempt in 0..300 {
        state.tick = 24_000 * (attempt + 1);
        state.trader_spawner = TraderSpawner {
            delay: 1,
            chance: 75,
        };
        state.trader_tick_delay = 1;
        let mut changes = ChangeSet::default();
        traders(state, &mut changes);
        if !changes.entities_spawned.is_empty() {
            return changes.entities_spawned;
        }
    }
    panic!("deterministic visit clock did not admit a visit");
}

#[test]
fn snow_golem_targets_enemy_and_snowball_damages_blaze_three() {
    let mut state = fixture("minecraft:snowy_plains");
    let golem = spawn(&mut state, "minecraft:snow_golem", [8.5, 80.0, 8.5]);
    let blaze = spawn(&mut state, "minecraft:blaze", [14.5, 80.0, 8.5]);
    let cow = spawn(&mut state, "minecraft:cow", [8.5, 80.0, 10.5]);
    state
        .entities
        .row_mut(state.entities.slot_of(blaze).unwrap())
        .controls
        .no_ai = true;
    state
        .entities
        .row_mut(state.entities.slot_of(cow).unwrap())
        .controls
        .no_ai = true;
    state
        .entities
        .row_mut(state.entities.slot_of(golem).unwrap())
        .age = 20;
    snowballs(&mut state);
    let shot = state
        .entities
        .slots()
        .find(|&slot| state.entities.row(slot).projectile.is_some())
        .unwrap();
    assert!(state.entities.vel(shot)[0] > 0.0);
    assert_eq!(
        state.entities.row(shot).projectile.as_ref().unwrap().kind,
        crate::projectiles::ProjectileKind::Snowball
    );
    state
        .entities
        .row_mut(state.entities.slot_of(golem).unwrap())
        .controls
        .no_ai = true;
    for _ in 0..12 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(blaze).unwrap())
            .health,
        17.0
    );
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(cow).unwrap())
            .health,
        10.0
    );
}

#[test]
fn snow_golem_throw_cadence_range_no_ai_and_occlusion() {
    let mut state = fixture("minecraft:snowy_plains");
    let golem = spawn(&mut state, "minecraft:snow_golem", [8.5, 80.0, 8.5]);
    let zombie = spawn(&mut state, "minecraft:zombie", [19.5, 80.0, 8.5]);
    let slot = state.entities.slot_of(golem).unwrap();
    state.entities.row_mut(slot).age = 20;
    snowballs(&mut state);
    assert!(state.projectiles.spawned.is_empty());
    state
        .entities
        .set_pos(state.entities.slot_of(zombie).unwrap(), [14.5, 80.0, 8.5]);
    state.entities.row_mut(slot).age = 19;
    snowballs(&mut state);
    assert!(state.projectiles.spawned.is_empty());
    state.entities.row_mut(slot).age = 20;
    let stone = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(11, 81, 8, stone);
    snowballs(&mut state);
    assert!(state.projectiles.spawned.is_empty());
    state.world.set_block(11, 81, 8, state.registry.states.air);
    state.entities.row_mut(slot).controls.no_ai = true;
    snowballs(&mut state);
    assert!(state.projectiles.spawned.is_empty());
    state.entities.row_mut(slot).controls.no_ai = false;
    snowballs(&mut state);
    assert_eq!(state.projectiles.spawned.len(), 1);
    state.entities.row_mut(slot).age = 21;
    snowballs(&mut state);
    assert_eq!(state.projectiles.spawned.len(), 1);
    state.entities.row_mut(slot).age = 40;
    snowballs(&mut state);
    assert_eq!(state.projectiles.spawned.len(), 2);
}

#[test]
fn snowball_knocks_back_other_enemy_without_damage() {
    let mut state = fixture("minecraft:snowy_plains");
    let golem = spawn(&mut state, "minecraft:snow_golem", [8.5, 80.0, 8.5]);
    let zombie = spawn(&mut state, "minecraft:zombie", [14.5, 80.0, 8.5]);
    state
        .entities
        .row_mut(state.entities.slot_of(zombie).unwrap())
        .controls
        .no_ai = true;
    state
        .entities
        .row_mut(state.entities.slot_of(golem).unwrap())
        .age = 20;
    snowballs(&mut state);
    state
        .entities
        .row_mut(state.entities.slot_of(golem).unwrap())
        .controls
        .no_ai = true;
    let mut pushed = false;
    for _ in 0..12 {
        run_tick(&mut state, &[]);
        pushed |= state.entities.vel(state.entities.slot_of(zombie).unwrap())[0] > 0.0;
    }
    assert!(pushed);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(zombie).unwrap())
            .health,
        20.0
    );
}

#[test]
fn snow_trail_cold_only_and_melting_rain_roof() {
    let mut state = fixture("minecraft:snowy_plains");
    spawn(&mut state, "minecraft:snow_golem", [8.5, 80.0, 8.5]);
    let mut writes = Vec::new();
    snow_trails(&mut state, &mut writes);
    assert_eq!(writes.len(), 1);
    assert_eq!(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(writes[0].1))
            .unwrap()
            .block
            .id
            .path(),
        "snow"
    );
    state
        .gamerules
        .insert("mob_griefing".into(), "false".into());
    let golem = state
        .entities
        .slots()
        .find(|&slot| crate::mounts::name(&state, slot) == "minecraft:snow_golem")
        .unwrap();
    state.entities.set_pos(golem, [12.5, 80.0, 8.5]);
    writes.clear();
    snow_trails(&mut state, &mut writes);
    assert!(writes.is_empty());
    for biome in ["minecraft:desert", "minecraft:plains"] {
        let mut state = fixture(biome);
        let golem = spawn(&mut state, "minecraft:snow_golem", [8.5, 80.0, 8.5]);
        if biome == "minecraft:plains" {
            state.weather = (crate::commands::Weather::Rain, 100);
        }
        snow_trails(&mut state, &mut Vec::new());
        state.commit_merge_effects();
        assert_eq!(
            state
                .entities
                .row(state.entities.slot_of(golem).unwrap())
                .health,
            3.0
        );
        if biome == "minecraft:plains" {
            let stone = state
                .registry
                .blocks
                .block(&Identifier::parse("minecraft:stone").unwrap())
                .unwrap()
                .default
                .0;
            state.world.set_block(8, 83, 8, stone);
            snow_trails(&mut state, &mut Vec::new());
            assert!(state.merge_damage.is_empty());
        }
    }
}

#[test]
fn wandering_trader_cadence_chance_steps_cap_and_rule_pause() {
    let mut state = fixture("minecraft:plains");
    for _ in 0..23_999 {
        traders(&mut state, &mut ChangeSet::default());
    }
    assert_eq!(
        state.trader_spawner,
        TraderSpawner {
            delay: 1200,
            chance: 25
        }
    );
    assert_eq!(state.trader_tick_delay, 1);
    state
        .gamerules
        .insert("spawn_wandering_traders".into(), "false".into());
    for _ in 0..100 {
        traders(&mut state, &mut ChangeSet::default());
    }
    assert_eq!(state.trader_spawner.delay, 1200);
    assert_eq!(state.trader_tick_delay, 1);
    state
        .gamerules
        .insert("spawn_wandering_traders".into(), "true".into());
    traders(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.trader_spawner,
        TraderSpawner {
            delay: 24_000,
            chance: 50
        }
    );
    for _ in 0..2 {
        state.trader_spawner.delay = 1;
        state.trader_tick_delay = 1;
        traders(&mut state, &mut ChangeSet::default());
        assert_eq!(state.trader_spawner.chance, 75);
    }
}

#[test]
fn wandering_trader_journals_only_at_enabled_minute_boundaries() {
    let mut state = fixture("minecraft:plains");
    let mut entries = Vec::new();
    journal(&mut state, &mut entries);
    assert_eq!(entries.len(), 1, "initial clock image");
    entries.clear();
    for tick in 1..=2400 {
        traders(&mut state, &mut ChangeSet::default());
        journal(&mut state, &mut entries);
        if tick % 1200 == 0 {
            assert_eq!(entries.len(), 1);
            assert!(
                matches!(&entries[0], crate::journal::JournalEntry::WanderingTrader { value }
                if value.delay == 24_000 - tick && value.chance == 25)
            );
            entries.clear();
        } else {
            assert!(entries.is_empty());
        }
    }
    state
        .gamerules
        .insert("spawn_wandering_traders".into(), "false".into());
    for _ in 0..1200 {
        traders(&mut state, &mut ChangeSet::default());
        journal(&mut state, &mut entries);
    }
    assert!(entries.is_empty());
    assert_eq!(state.trader_tick_delay, 1200);
    assert_eq!(state.trader_spawner.delay, 21_600);
}

#[test]
fn wandering_trader_signed_saved_clock_uses_vanilla_integer_steps() {
    let mut state = fixture("minecraft:plains");
    state.trader_spawner = TraderSpawner {
        delay: 0,
        chance: 0,
    };
    state.trader_tick_delay = 1;
    traders(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.trader_spawner,
        TraderSpawner {
            delay: 24_000,
            chance: 25
        }
    );
    state.trader_spawner = TraderSpawner {
        delay: i32::MIN,
        chance: -1,
    };
    state.trader_tick_delay = 1;
    traders(&mut state, &mut ChangeSet::default());
    assert_eq!(state.trader_spawner.delay, i32::MIN.wrapping_sub(1200));
    assert_eq!(
        state.trader_spawner.chance, -1,
        "no chance roll while delay remains positive"
    );
    state.trader_spawner = TraderSpawner {
        delay: -1,
        chance: i32::MAX,
    };
    state.trader_tick_delay = 1;
    traders(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.trader_spawner,
        TraderSpawner {
            delay: 24_000,
            chance: 25
        }
    );
}

#[test]
fn wandering_trader_visit_two_leashed_llamas_and_timed_despawn_rule_off() {
    let mut state = fixture("minecraft:plains");
    join(&mut state);
    let spawned = spawn_visit(&mut state);
    assert_eq!(spawned.len(), 3);
    assert_eq!(state.trader_spawner.chance, 25);
    let trader = state.entities.slot_of(spawned[0]).unwrap();
    let owner = state.entities.row(trader).uuid;
    for id in &spawned[1..] {
        let row = state.entities.row(state.entities.slot_of(*id).unwrap());
        assert_eq!(
            row.entity_life.leash,
            Some(crate::leash::Holder::Entity(owner))
        );
    }
    let deadline = state
        .entities
        .row(trader)
        .villager
        .as_ref()
        .unwrap()
        .despawn_at
        .unwrap();
    state
        .gamerules
        .insert("spawn_wandering_traders".into(), "false".into());
    state.tick = deadline - 1;
    traders(&mut state, &mut ChangeSet::default());
    assert!(
        spawned
            .iter()
            .all(|&id| state.entities.slot_of(id).is_some())
    );
    state.tick = deadline;
    let mut changes = ChangeSet::default();
    traders(&mut state, &mut changes);
    assert_eq!(changes.entities_removed.len(), 3);
    assert!(
        spawned
            .iter()
            .all(|&id| state.entities.slot_of(id).is_none())
    );
}

#[test]
fn due_trader_candidates_match_the_dense_scan_in_a_large_population() {
    let mut state = fixture("minecraft:plains");
    let player = join(&mut state);
    let around = state.entities.pos(state.entities.slot_of(player).unwrap());
    let spawned = spawn_visit(&mut state);
    let trader = state.entities.slot_of(spawned[0]).unwrap();
    let village = state.entities.row(trader).villager.clone().unwrap();
    let deadline = village.despawn_at.unwrap();
    state
        .gamerules
        .insert("spawn_wandering_traders".into(), "false".into());
    // Enough bodies that the due-trader filter splits across the pool, with
    // due-active, due-inactive and not-yet-due carriers scattered among them.
    let cow = state.registry.cow_type_id;
    let far = [around[0] + 4_000.0, around[1], around[2]];
    let mut carriers = Vec::new();
    for index in 0..9_000 {
        let at = if index == 7_000 { far } else { around };
        let id = state.spawn_mob(cow, at, 10.0);
        let despawn_at = match index {
            100 | 5_000 | 7_000 => Some(deadline),
            8_000 => Some(deadline + 10),
            _ => None,
        };
        if despawn_at.is_some() {
            let row = state.entities.row_mut(state.entities.slot_of(id).unwrap());
            let mut carried = village.clone();
            carried.despawn_at = despawn_at;
            row.villager = Some(carried);
            carriers.push(id);
        }
    }
    state.tick = deadline;
    for min_parallel in [0, usize::MAX] {
        let (owners, slots) = super::due_traders(&state, min_parallel);
        let (expected_owners, expected_slots) = super::due_traders_by_scan(&state);
        assert_eq!((owners, slots), (expected_owners, expected_slots.clone()));
        // The visit trader plus the two active due carriers; the inactive and
        // the not-yet-due carrier are not candidates.
        assert_eq!(expected_slots.len(), 3);
    }
    let mut changes = ChangeSet::default();
    traders(&mut state, &mut changes);
    // Trader and its two llamas, plus the two active due carriers.
    assert_eq!(changes.entities_removed.len(), 5);
    assert!(
        state.entities.slot_of(carriers[2]).is_some(),
        "inactive stays"
    );
    assert!(
        state.entities.slot_of(carriers[3]).is_some(),
        "not due stays"
    );
}
