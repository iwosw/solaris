//! Skeleton, spider, creeper and arrows on a stone platform in open sky.

use std::sync::Arc;

use crate::boundary::{ConnectionId, EntityDataValue, EntityId, PlayerInput};
use crate::hostile::{CREEPER_SWELL_DIR_INDEX, FUSE_TICKS, launch_arrow};
use crate::projectiles::{ARROW_IN_GROUND_INDEX, ARROW_STUCK_TICKS};
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;

/// Platform height: far above the flat fixture floor.
const Y: i32 = 200;

fn test_state(sim_distance: i32) -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry loads"));
    let world = crate::horde_bench::flat_world(&registry);
    TickState::new(registry, world, sim_distance, 512)
}

fn type_id(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).expect("id parses"))
        .expect("entity type")
}

/// A day-time world with one player standing on a 25x25 stone platform;
/// returns the platform centre column (feet height of the platform top) and
/// the player, who starts `offset` blocks west of the centre.
pub(crate) fn setup(offset: f64) -> (TickState, EntityId, [f64; 3]) {
    let mut state = test_state(2);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "target".to_owned(),
            permission_level: 0,
        }],
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state.refresh_active_chunks(state.tick);
    let player = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("joined");
    let slot = state.entities.slot_of(player).expect("slot");
    let at = state.entities.pos(slot);
    let centre = [at[0].floor() + 0.5, f64::from(Y + 1), at[2].floor() + 0.5];
    let stone = state.registry.states.stone;
    for dx in -12..=12 {
        for dz in -12..=12 {
            state
                .world
                .set_block(centre[0] as i32 + dx, Y, centre[2] as i32 + dz, stone);
        }
    }
    state.world.fold_overlay();
    state
        .entities
        .set_pos(slot, [centre[0] - offset, centre[1], centre[2]]);
    // Spawn protection would swallow the first hits.
    state.entities.row_mut(slot).protected_until = 0;
    state.day_time = 6_000;
    (state, player, centre)
}

fn health(state: &TickState, player: EntityId) -> f32 {
    state.players[&player].vitals.health
}

fn move_player(state: &mut TickState, player: EntityId, to: [f64; 3]) {
    let slot = state.entities.slot_of(player).expect("slot");
    state.entities.set_pos(slot, to);
}

fn extra(state: &TickState, id: EntityId, index: u8) -> Option<EntityDataValue> {
    let slot = state.entities.slot_of(id)?;
    state
        .entities
        .row(slot)
        .appearance
        .extra
        .iter()
        .find(|(at, _)| *at == index)
        .map(|(_, value)| value.clone())
}

#[test]
fn statistics_creeper_killed_by_survives_exploding_source_removal() {
    use mc_data::statistics::{CustomStat, StatType};
    let (mut state, player, centre) = setup(2.0);
    state
        .gamerules
        .insert("mob_griefing".into(), "false".into());
    state.players.get_mut(&player).unwrap().vitals.health = 1.0;
    let creeper_type = type_id(&state, "minecraft:creeper");
    let creeper = state.spawn_mob(creeper_type, centre, 20.0);
    for _ in 0..FUSE_TICKS {
        run_tick(&mut state, &[]);
    }
    assert!(state.entities.slot_of_any(creeper).is_none());
    let statistics = &state.players[&player].statistics;
    assert_eq!(statistics.get_custom(CustomStat::Deaths), 1);
    assert_eq!(statistics.get(StatType::KilledBy, creeper_type), 1);
}

/// A skeleton shoots a ballistic arrow entity that damages the player at range.
#[test]
fn skeleton_arrow_damages_a_player_at_range() {
    let (mut state, player, centre) = setup(8.0);
    // Skeletons burn/flee in sunlight (minecraft.wiki/w/Skeleton#Sunlight).
    // Isolate bow combat from that goal and from natural health regeneration.
    state.day_time = 14_000;
    let vitals = &mut state.players.get_mut(&player).expect("player").vitals;
    vitals.food = 17;
    vitals.saturation = 0.0;
    let skeleton = type_id(&state, "minecraft:skeleton");
    let arrow = type_id(&state, "minecraft:arrow");
    state.spawn_mob(skeleton, [centre[0], centre[1], centre[2]], 20.0);
    let mut arrows_seen = 0;
    for _ in 0..400 {
        let (output, _, _) = run_tick(&mut state, &[]);
        arrows_seen = arrows_seen.max(
            output
                .snapshot
                .entities
                .iter()
                .filter(|entity| entity.type_id == arrow)
                .count(),
        );
    }
    assert!(arrows_seen > 0, "the skeleton shoots arrow entities");
    assert!(health(&state, player) < 20.0, "an arrow hit the player");
}

#[test]
fn skeleton_draws_for_twenty_ticks_and_uses_difficulty_bow_intervals() {
    for (difficulty, second_shot) in [
        (crate::survival::Difficulty::Easy, 81),
        (crate::survival::Difficulty::Normal, 81),
        (crate::survival::Difficulty::Hard, 61),
    ] {
        let (mut state, _, centre) = setup(8.0);
        state.difficulty = difficulty;
        // Sun avoidance outranks the bow goal in vanilla AbstractSkeleton.
        state.day_time = 14_000;
        let skeleton = type_id(&state, "minecraft:skeleton");
        let arrow = type_id(&state, "minecraft:arrow");
        state.spawn_mob(skeleton, centre, 20.0);
        let mut seen = std::collections::HashSet::new();
        let mut shots = Vec::new();
        for elapsed in 1..=second_shot {
            let (output, _, _) = run_tick(&mut state, &[]);
            for entity in output.snapshot.entities.iter() {
                if entity.type_id == arrow && seen.insert(entity.id) {
                    shots.push(elapsed);
                }
            }
        }
        assert_eq!(shots, vec![21, second_shot], "{difficulty:?}");
    }
}

#[test]
fn natural_skeleton_arrow_hit_and_tombstone_survive_checkpoint_wal_replay() {
    use crate::boundary::EntityEvent;
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let (mut state, player, centre) = setup(12.0);
    state.day_time = 18_000;
    state.difficulty = crate::survival::Difficulty::Normal;
    state.players.get_mut(&player).unwrap().vitals.food = 17;
    let player_uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid
        .to_string();
    let skeleton = state.spawn_mob(type_id(&state, "minecraft:skeleton"), centre, 20.0);
    let bow = mc_data::ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:bow").unwrap())
            .unwrap(),
        1,
    );
    Arc::make_mut(
        &mut state
            .entities
            .row_mut(state.entities.slot_of(skeleton).unwrap())
            .appearance,
    )
    .equipment[0] = Some(bow);
    let owner = state
        .entities
        .row(state.entities.slot_of(skeleton).unwrap())
        .uuid;
    let directory = tempfile::tempdir().unwrap();
    let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
    let mut checkpoint = None;
    let mut checkpoint_tick = 0;
    let mut hit = None;
    let mut moved = std::collections::HashSet::new();
    let mut initial = std::collections::HashMap::new();
    for elapsed in 1..=400 {
        let (output, _, _) = run_tick(&mut state, &[]);
        for entity in output.snapshot.entities.iter() {
            let Some(slot) = state.entities.slot_of(entity.id) else {
                continue;
            };
            let Some(record) = &state.entities.row(slot).projectile else {
                continue;
            };
            if record.owner != Some(owner) {
                continue;
            }
            assert!(!record.pickup, "skeleton ammunition is never pickupable");
            assert!(
                record.stack.is_empty(),
                "skeleton does not mint player ammunition"
            );
            let from = initial.entry(entity.id).or_insert(entity.position);
            if *from != entity.position {
                moved.insert(entity.id);
            }
        }
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        if checkpoint.is_none() && !initial.is_empty() {
            assert_eq!(elapsed, 21, "twenty tick bow draw before the first launch");
            let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
            let mut base = Replay::default();
            for entity in chunks.values().flat_map(|chunk| &chunk.entities) {
                let payload = mc_nbt::Tag::Compound(
                    mc_data::entity_nbt::entity_to_fields(&state.registry.items, entity).unwrap(),
                );
                crate::dimensions::recovery::hydrate_entity(
                    &state.registry,
                    &mut base,
                    &payload,
                    state.tick,
                )
                .unwrap();
            }
            checkpoint_tick = state.tick;
            checkpoint = Some(base);
        } else if checkpoint.is_some() {
            writer.append(output.tick, output.tick, &entries).unwrap();
        }
        for event in &output.changes.entity_events {
            if let EntityEvent::Damage {
                entity,
                cause: Some(cause),
                direct: Some(direct),
                source_type_id,
                ..
            } = *event
                && entity == player
                && cause == skeleton
            {
                assert_eq!(
                    source_type_id,
                    crate::survival::DamageKind::Projectile.type_id(
                        Some(crate::intent::DamageSource::Projectile {
                            projectile: direct,
                            direct_type_id: type_id(&state, "minecraft:arrow"),
                            owner: skeleton,
                            velocity: [0.0; 2],
                            punch: 0,
                            flame: false,
                        }),
                        false
                    )
                );
                assert!(moved.contains(&direct), "the arrow flew before hitting");
                assert!(output.changes.entities_removed.contains(&direct));
                assert!(health(&state, player) < 20.0);
                hit = Some(direct);
            }
        }
        if hit.is_some() {
            break;
        }
    }
    let hit = hit.expect("natural arrow applies damage and publishes causal hit/removal");
    writer.flush().unwrap();
    let batches =
        read_complete_batches_after(&directory.path().join("journal"), checkpoint_tick).unwrap();
    let recovered = replay_batches(checkpoint.unwrap(), &batches);
    assert!(
        !recovered.entities.contains_key(&hit.0),
        "WAL tombstone overrides in-flight checkpoint"
    );
    assert_eq!(
        recovered.players[&player_uuid].health,
        health(&state, player)
    );
    let reapplied = replay_batches(recovered, &batches);
    assert!(
        !reapplied.entities.contains_key(&hit.0),
        "replaying twice cannot duplicate arrows"
    );
}

#[test]
fn skeleton_arrow_on_ground_cannot_be_picked_up() {
    let (mut state, player, centre) = setup(9.0);
    state.day_time = 18_000;
    let skeleton = state.spawn_mob(type_id(&state, "minecraft:skeleton"), centre, 20.0);
    state
        .entities
        .row_mut(state.entities.slot_of(skeleton).unwrap())
        .controls
        .no_ai = true;
    let arrow = launch_arrow(
        &mut state,
        skeleton,
        [centre[0], centre[1] + 1.0, centre[2]],
        [0.5, -0.3, 0.0],
        2.0,
    )
    .unwrap();
    for _ in 0..20 {
        run_tick(&mut state, &[]);
    }
    let slot = state.entities.slot_of(arrow).unwrap();
    assert!(state.entities.row(slot).projectile.as_ref().unwrap().stuck);
    let pos = state.entities.pos(slot);
    let inventory = state.players[&player].inventory.slots.clone();
    move_player(&mut state, player, [pos[0], centre[1], pos[2]]);
    for _ in 0..10 {
        run_tick(&mut state, &[]);
    }
    assert!(
        state.entities.slot_of(arrow).is_some(),
        "proximity does not collect skeleton arrows"
    );
    assert_eq!(state.players[&player].inventory.slots, inventory);
    for _ in 0..=ARROW_STUCK_TICKS {
        run_tick(&mut state, &[]);
    }
    assert!(
        state.entities.slot_of(arrow).is_none(),
        "uncollectable arrow eventually despawns"
    );
}

/// Arrows fall, stick in the first block they meet (shown to the client as
/// in-ground), and despawn after a minute.
#[test]
fn arrow_sticks_in_a_block_then_despawns() {
    let (mut state, player, centre) = setup(9.0);
    let from = [centre[0], centre[1] + 1.0, centre[2]];
    let arrow = launch_arrow(&mut state, player, from, [0.5, -0.3, 0.0], 2.0).expect("launch");
    for _ in 0..20 {
        let _ = run_tick(&mut state, &[]);
    }
    let slot = state.entities.slot_of(arrow).expect("arrow still there");
    assert_eq!(state.entities.vel(slot), [0.0; 3], "stopped");
    assert!(
        state.entities.pos(slot)[1] < from[1],
        "it fell before it stuck"
    );
    assert_eq!(
        extra(&state, arrow, ARROW_IN_GROUND_INDEX),
        Some(EntityDataValue::Boolean(true))
    );
    for _ in 0..=ARROW_STUCK_TICKS {
        let _ = run_tick(&mut state, &[]);
    }
    assert!(state.entities.slot_of(arrow).is_none(), "despawned");
}

/// A stronger arrow hits the first thing in its way, not the owner.
#[test]
fn arrow_damage_is_ceil_of_speed_times_base_and_scaled_by_difficulty() {
    let (mut state, player, centre) = setup(0.0);
    let from = [centre[0] - 3.0, centre[1] + 1.2, centre[2]];
    // 1.6 blocks/tick * 2 = 3.2 -> 4, easy scales 4 to 3.
    launch_arrow(&mut state, EntityId(0), from, [1.6, 0.0, 0.0], 2.0).expect("launch");
    for _ in 0..4 {
        let _ = run_tick(&mut state, &[]);
    }
    assert_eq!(health(&state, player), 17.0);
}

/// A spider claws its prey (2 damage) at night but ignores it by day.
#[test]
fn spider_attacks_at_night_and_is_neutral_by_day() {
    let (mut state, player, centre) = setup(0.0);
    let spider = type_id(&state, "minecraft:spider");
    state.spawn_mob(spider, [centre[0] + 1.5, centre[1], centre[2]], 16.0);
    state.day_time = 6_000;
    for _ in 0..40 {
        let _ = run_tick(&mut state, &[]);
    }
    assert_eq!(health(&state, player), 20.0, "neutral in daylight");
    state.day_time = 18_000;
    for _ in 0..40 {
        let _ = run_tick(&mut state, &[]);
    }
    let lost = 20.0 - health(&state, player);
    assert!(lost >= 2.0, "hostile at night");
    assert_eq!(lost % 2.0, 0.0, "2 point bites, easy keeps 2");
}

/// The creeper starts a fuse next to the player, swells (entity data the
/// client animates from), then blasts: the platform cracks open, bedrock-class
/// blocks survive and the player is hurt.
#[test]
fn creeper_fuse_then_explosion_breaks_blocks_and_hurts_the_player() {
    let (mut state, player, centre) = setup(2.0);
    let obsidian = state.registry.states.obsidian;
    for dx in -12..=12 {
        for dz in -12..=12 {
            state.world.set_block(
                centre[0] as i32 + dx,
                Y - 1,
                centre[2] as i32 + dz,
                obsidian,
            );
        }
    }
    state.world.fold_overlay();
    let creeper_type = type_id(&state, "minecraft:creeper");
    let creeper = state.spawn_mob(creeper_type, centre, 20.0);
    let mut blasts = Vec::new();
    for tick in 1..=FUSE_TICKS + 5 {
        let (output, _, _) = run_tick(&mut state, &[]);
        blasts.extend(output.changes.explosions.iter().cloned());
        if tick == 10 {
            assert!(state.entities.slot_of(creeper).is_some(), "fuse burning");
            assert_eq!(
                extra(&state, creeper, CREEPER_SWELL_DIR_INDEX),
                Some(EntityDataValue::VarInt(1)),
                "clients see it swell"
            );
            assert!(state.hostile.fuse(creeper) > 0);
        }
    }
    assert!(state.entities.slot_of(creeper).is_none(), "exploded");
    assert_eq!(blasts.len(), 1);
    assert!(blasts[0].blocks > 0);
    assert!(
        blasts[0]
            .knockback
            .iter()
            .any(|(entity, push)| *entity == player && push[0] < 0.0),
        "the player is pushed away from the blast"
    );
    let (x, z) = (centre[0] as i32, centre[2] as i32);
    let air = state.registry.states.air;
    assert_eq!(state.world.block_at(x, Y, z), Some(air), "stone under it");
    assert_eq!(
        state.world.block_at(x + 11, Y, z),
        Some(state.registry.states.stone),
        "far stone stays"
    );
    for dx in -3..=3 {
        for dz in -3..=3 {
            assert_eq!(
                state.world.block_at(x + dx, Y - 1, z + dz),
                Some(obsidian),
                "obsidian is blast proof"
            );
        }
    }
    assert!(health(&state, player) < 20.0, "the blast hurt the player");
}

#[test]
fn mob_griefing_false_preserves_creeper_blast_damage_but_not_block_damage() {
    let (mut state, player, centre) = setup(2.5);
    state
        .gamerules
        .insert("mob_griefing".into(), "false".into());
    let creeper_type = type_id(&state, "minecraft:creeper");
    let creeper = state.spawn_mob(creeper_type, centre, 20.0);
    let mut blasts = Vec::new();
    for _ in 0..FUSE_TICKS + 5 {
        let (output, _, _) = run_tick(&mut state, &[]);
        blasts.extend(output.changes.explosions.iter().cloned());
    }
    assert!(state.entities.slot_of(creeper).is_none());
    assert_eq!(blasts.len(), 1);
    assert_eq!(blasts[0].blocks, 0);
    assert_eq!(
        state.world.block_at(centre[0] as i32, Y, centre[2] as i32),
        Some(state.registry.states.stone)
    );
    assert!(health(&state, player) < 20.0);
}

/// The `[mob_griefing] creeper_block_damage = false` switch narrows the
/// gamerule without touching blast damage to entities.
#[test]
fn creeper_block_damage_switch_stops_blocks_but_not_damage() {
    let (mut state, player, centre) = setup(2.5);
    state.mob_griefing.creeper_block_damage = false;
    let creeper_type = type_id(&state, "minecraft:creeper");
    let creeper = state.spawn_mob(creeper_type, centre, 20.0);
    let mut blasts = Vec::new();
    for _ in 0..FUSE_TICKS + 5 {
        let (output, _, _) = run_tick(&mut state, &[]);
        blasts.extend(output.changes.explosions.iter().cloned());
    }
    assert!(state.entities.slot_of(creeper).is_none());
    assert_eq!(blasts.len(), 1);
    assert_eq!(blasts[0].blocks, 0, "the switch removes no blocks");
    assert_eq!(
        state.world.block_at(centre[0] as i32, Y, centre[2] as i32),
        Some(state.registry.states.stone),
        "the platform survives"
    );
    assert!(health(&state, player) < 20.0, "the blast still hurts");
}

/// The ghast switch gates ghast blasts only; a creeper blast still breaks.
#[test]
fn ghast_block_damage_switch_gates_only_ghast_blasts() {
    let (mut state, _player, centre) = setup(2.5);
    state.mob_griefing.ghast_block_damage = false;
    crate::explosion::apply(
        &mut state,
        None,
        centre,
        4.0,
        Some(crate::command_rules::MobGrief::Ghast),
        None,
    );
    assert_eq!(
        state.world.block_at(centre[0] as i32, Y, centre[2] as i32),
        Some(state.registry.states.stone),
        "a disabled ghast blast leaves the platform"
    );

    crate::explosion::apply(
        &mut state,
        None,
        centre,
        4.0,
        Some(crate::command_rules::MobGrief::Creeper),
        None,
    );
    let air = state.registry.states.air;
    let breaks = state
        .merge_block_writes
        .iter()
        .filter(|(_, block)| *block == air)
        .count();
    assert!(breaks > 0, "another mob's blast still removes blocks");
}

/// The fuse burns down when the player gets clear (more than 7 blocks).
#[test]
fn creeper_fuse_cancels_when_the_player_leaves() {
    let (mut state, player, centre) = setup(9.0);
    let creeper_type = type_id(&state, "minecraft:creeper");
    let spawn = [centre[0] - 7.0, centre[1], centre[2]];
    let creeper = state.spawn_mob(creeper_type, spawn, 20.0);
    for _ in 0..10 {
        let _ = run_tick(&mut state, &[]);
    }
    assert!(state.hostile.fuse(creeper) >= 5, "fuse is burning");
    move_player(&mut state, player, [centre[0] + 9.0, centre[1], centre[2]]);
    let mut blasts = 0;
    for _ in 0..20 {
        let (output, _, _) = run_tick(&mut state, &[]);
        blasts += output.changes.explosions.len();
    }
    assert!(state.entities.slot_of(creeper).is_some(), "still alive");
    assert_eq!(state.hostile.fuse(creeper), 0, "fuse burnt out");
    assert_eq!(
        extra(&state, creeper, CREEPER_SWELL_DIR_INDEX),
        Some(EntityDataValue::VarInt(-1))
    );
    assert_eq!(blasts, 0);
    assert_eq!(health(&state, player), 20.0);
}

/// Natural night spawns cover the new hostile types.
#[test]
fn natural_spawns_include_skeleton_spider_and_creeper() {
    // Biome admission is fail-closed: an arbitrary worldgen seed is not a
    // guarantee of eligible biome, support or enough room below the mob cap.
    struct SpawnPlatform {
        air: u32,
        stone: u32,
    }
    impl mc_world::ChunkGenerator for SpawnPlatform {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            let mut chunk = mc_world::Chunk::empty(
                pos,
                mc_world::BlockStateId(self.air),
                mc_data::Identifier::parse("minecraft:plains").expect("biome"),
            );
            for x in 0..16 {
                for z in 0..16 {
                    chunk.set_block(x, 64, z, mc_world::BlockStateId(self.stone));
                }
            }
            chunk
        }
    }
    let registry = Arc::new(CoreRegistry::load(None).expect("registry loads"));
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(SpawnPlatform {
        air: registry.states.air,
        stone: registry.states.stone,
    });
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 65, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 5, 512);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "bait".to_owned(),
            permission_level: 0,
        }],
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let player = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("joined");
    // Combat is covered separately; a dead observer stops spawn passes and
    // creeper explosions would destroy this test's eligible platform.
    state.players.get_mut(&player).expect("player").game_mode = crate::commands::GameMode::Creative;
    let plains = mc_data::Identifier::parse("minecraft:plains").expect("biome");
    let creeper = state
        .registry
        .biome_spawns
        .exact_entries(&plains, "monster")
        .iter()
        .find(|entry| entry.entity_type.as_str() == "minecraft:creeper")
        .expect("embedded plains includes the vanilla creeper pack");
    assert_eq!(
        (creeper.weight, creeper.min_count, creeper.max_count),
        (100, 4, 4)
    );
    let wanted: Vec<u32> = ["zombie", "skeleton", "spider", "creeper"]
        .iter()
        .map(|name| type_id(&state, &format!("minecraft:{name}")))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    state.day_time = 14_000;
    for _ in 0..1_600 {
        state.day_time = 14_000;
        let (output, _, _) = run_tick(&mut state, &[]);
        for entity in output.snapshot.entities.iter() {
            if wanted.contains(&entity.type_id) {
                seen.insert(entity.type_id);
            }
        }
    }
    assert_eq!(seen.len(), wanted.len(), "spawned types: {seen:?}");
}

#[test]
fn accepted_arrow_hit_pushes_along_flight_after_the_arrow_is_removed() {
    use crate::boundary::PlayerChange;
    let (mut state, player, centre) = setup(0.0);
    let victim = state.entities.slot_of(player).unwrap();
    state.entities.set_ground(victim, true);
    state.difficulty = crate::survival::Difficulty::Normal;
    let shooter_type = type_id(&state, "minecraft:skeleton");
    let shooter = state.spawn_mob(shooter_type, [centre[0] + 8.0, centre[1], centre[2]], 20.0);
    let from = [centre[0] - 0.6, centre[1] + 1.0, centre[2] - 0.8];
    let arrow = launch_arrow(&mut state, shooter, from, [1.2, 0.0, 1.6], 2.0).unwrap();
    let (output, drains, _) = run_tick(&mut state, &[]);
    assert!(
        state.entities.slot_of(arrow).is_none(),
        "source arrow already despawned"
    );
    // Arrows are runtime-only: unlike journaled creeper removals they do
    // not emit tombstones. Edge entity tracking removes them from this
    // published snapshot's absence.
    assert!(
        !output
            .snapshot
            .entities
            .iter()
            .any(|entity| entity.id == arrow)
    );
    assert_eq!(health(&state, player), 16.0);
    let motion = drains
        .iter()
        .flat_map(|(_, changes)| changes)
        .find_map(|change| match change {
            PlayerChange::Motion {
                entity, velocity, ..
            } if *entity == player => Some(*velocity),
            _ => None,
        })
        .expect("accepted arrow sends victim motion");
    for (actual, expected) in motion.into_iter().zip([4.8, 8.0, 6.4]) {
        assert!((actual - expected).abs() < 1.0e-6, "{motion:?}");
    }
    // Another arrow inside the window can increase damage, but cannot
    // renew the base impulse or hurt animation.
    launch_arrow(&mut state, shooter, from, [1.2, 0.0, 1.6], 3.0).unwrap();
    let (output, drains, _) = run_tick(&mut state, &[]);
    assert_eq!(health(&state, player), 14.0);
    assert!(
        !output
            .changes
            .entity_events
            .contains(&crate::boundary::EntityEvent::Hurt { entity: player })
    );
    assert!(
        !drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change, PlayerChange::Motion { .. }),)
    );
}

#[test]
fn fresh_hits_publish_damage_and_excess_hits_journal_mob_health_without_wire_events() {
    use crate::boundary::EntityEvent;
    use crate::survival::DamageKind;
    let (mut state, _, centre) = setup(8.0);
    let mob = state.spawn_mob(type_id(&state, "minecraft:cow"), centre, 20.0);
    let slot = state.entities.slot_of(mob).unwrap();
    state.entities.row_mut(slot).controls.no_ai = true;
    let mut output = run_tick(&mut state, &[]).0;
    Arc::get_mut(&mut output.changes)
        .expect("unique tick change set")
        .entities_spawned
        .push(mob);
    let mut recovered = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &[(
            output.tick,
            output.tick,
            crate::core::journal_entries_for_tick(&mut state, &output),
        )],
    );
    let changes = Arc::get_mut(&mut output.changes).expect("unique tick change set");
    changes.entities_spawned.clear();
    changes.entities_appearance_changed.clear();
    // (amount, admitted, fresh): a stronger hit inside the hurt window lands
    // its health delta but, like vanilla, broadcasts no damage/hurt event.
    for (amount, accepted, fresh) in [(2.0, true, true), (2.0, false, false), (4.0, true, false)] {
        state.merge_damage.push((
            slot,
            crate::tick::DamagePiece {
                physical: None,
                custom_armor: false,
                amount,
                kind: DamageKind::Cactus,
                source: None,
            },
        ));
        let (_, events, _, _) = state.commit_merge_effects();
        assert_eq!(
            events.contains(&EntityEvent::Damage {
                entity: mob,
                source_type_id: DamageKind::Cactus.type_id(None, false),
                cause: None,
                direct: None,
                source_position: None,
            }),
            fresh
        );
        assert_eq!(events.contains(&EntityEvent::Hurt { entity: mob }), fresh);
        let changes = Arc::get_mut(&mut output.changes).expect("unique tick change set");
        changes.entity_events = events;
        changes.entities_appearance_changed = std::mem::take(&mut state.appearance_changed);
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        if accepted {
            let health = state.entities.row(slot).health;
            assert!(entries.iter().any(|entry| match entry {
                crate::journal::JournalEntry::EntityVitals { id, health: h, .. } => {
                    *id == mob.0 && *h == health
                }
                crate::journal::JournalEntry::Entity { id, health: h, .. } => {
                    *id == mob.0 && *h == health
                }
                _ => false,
            }));
        }
        recovered =
            crate::journal::replay_batches(recovered, &[(output.tick, output.tick, entries)]);
    }
    assert_eq!(state.entities.row(slot).health, 16.0);
    assert_eq!(recovered.entities[&mob.0].health, 16.0);
}

#[test]
fn targetless_creeper_still_unwinds_fuse_and_resets_retained_swell_metadata() {
    use crate::hostile::Special;
    let (mut state, player, centre) = setup(2.0);
    let creeper = state.spawn_mob(type_id(&state, "minecraft:creeper"), centre, 20.0);
    let specials = [(creeper, Special::Swell)];
    let mut intents = Vec::new();
    crate::hostile::run(&mut state, &mut intents, &specials);
    assert_eq!(state.hostile.fuse(creeper), 1);
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
    crate::hostile::run(&mut state, &mut intents, &specials);
    assert_eq!(state.hostile.fuse(creeper), 0);
    assert_eq!(
        extra(&state, creeper, CREEPER_SWELL_DIR_INDEX),
        Some(EntityDataValue::VarInt(-1))
    );
    let slot = state.entities.slot_of(creeper).unwrap();
    let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
    appearance
        .extra
        .retain(|(index, _)| *index != CREEPER_SWELL_DIR_INDEX);
    appearance
        .extra
        .push((CREEPER_SWELL_DIR_INDEX, EntityDataValue::VarInt(1)));
    crate::hostile::run(&mut state, &mut intents, &specials);
    assert_eq!(state.hostile.fuse(creeper), 0);
    assert_eq!(
        extra(&state, creeper, CREEPER_SWELL_DIR_INDEX),
        Some(EntityDataValue::VarInt(-1)),
        "restart-local absence of a fuse must still repair retained swelling metadata"
    );
}

/// Empty-target Ranged skips only without an in-progress bow: a fresh
/// skeleton never starts a draw, one mid-draw keeps its `bows` entry.
#[test]
fn targetless_ranged_skips_only_without_in_progress_bow() {
    use crate::hostile::Special;
    let (mut state, player, centre) = setup(8.0);
    state.day_time = 14_000;
    let skeleton = state.spawn_mob(type_id(&state, "minecraft:skeleton"), centre, 20.0);
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
    assert!(!crate::hostile::has_live_targets(&state));
    assert!(
        state
            .hostile
            .idle_without_targets(skeleton, Special::Ranged, false)
    );
    // Fresh targetless skeleton: no draw can start, serial pass stays idle.
    let specials = [(skeleton, Special::Ranged)];
    let mut intents = Vec::new();
    crate::hostile::run(&mut state, &mut intents, &specials);
    // No live targets also means no in-progress bow, so the serial pass must
    // leave bow state untouched (still skippable on both flag values).
    for neutral in [false, true] {
        assert!(
            state
                .hostile
                .idle_without_targets(skeleton, Special::Ranged, neutral),
            "Ranged ignores the metadata flag: still skippable without a bow"
        );
    }
    assert!(intents.is_empty());
    // Re-admit the target: one live-target pass inserts a bow draw (spawned
    // mobs start with cooldown 0), so the skeleton is no longer skippable.
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Survival;
    assert!(crate::hostile::has_live_targets(&state));
    crate::hostile::run(&mut state, &mut intents, &specials);
    assert!(
        !state
            .hostile
            .idle_without_targets(skeleton, Special::Ranged, false)
    );
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
    assert!(!crate::hostile::has_live_targets(&state));
    assert!(
        !state
            .hostile
            .idle_without_targets(skeleton, Special::Ranged, true)
    );
}

/// Swell needs fuse 0 AND neutral swell metadata to skip; a positive fuse or
/// retained swelling metadata stays in the serial pass to unwind/reset.
#[test]
fn targetless_swell_skips_only_with_fuse_zero_and_neutral_metadata() {
    use crate::hostile::Special;
    let (mut state, player, centre) = setup(2.0);
    let creeper = state.spawn_mob(type_id(&state, "minecraft:creeper"), centre, 20.0);
    let slot = state.entities.slot_of(creeper).unwrap();
    assert!(state.hostile.idle_without_targets(
        creeper,
        Special::Swell,
        crate::hostile::swell_metadata_neutral(state.entities.row(slot))
    ));
    // Positive fuse is never skippable, even with neutral metadata: light it
    // next to the live player, then prove the predicate sees the fuse.
    crate::hostile::run(&mut state, &mut Vec::new(), &[(creeper, Special::Swell)]);
    assert_eq!(state.hostile.fuse(creeper), 1);
    assert!(
        !state
            .hostile
            .idle_without_targets(creeper, Special::Swell, true)
    );
    // A targetless serial pass must still reach `swell` and unwind the fuse
    // to 0 instead of skipping.
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
    crate::hostile::run(&mut state, &mut Vec::new(), &[(creeper, Special::Swell)]);
    assert_eq!(state.hostile.fuse(creeper), 0);
    // Retained swelling metadata is never skippable, even with fuse 0.
    let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
    appearance
        .extra
        .retain(|(index, _)| *index != CREEPER_SWELL_DIR_INDEX);
    appearance
        .extra
        .push((CREEPER_SWELL_DIR_INDEX, EntityDataValue::VarInt(1)));
    assert!(!state.hostile.idle_without_targets(
        creeper,
        Special::Swell,
        crate::hostile::swell_metadata_neutral(state.entities.row(slot))
    ));
    // Through `run`: retained swelling metadata still reaches the serial pass
    // without targets and resets to idle (fuse stays 0).
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
    let specials = [(creeper, Special::Swell)];
    let mut intents = Vec::new();
    crate::hostile::run(&mut state, &mut intents, &specials);
    assert_eq!(state.hostile.fuse(creeper), 0);
    assert_eq!(
        extra(&state, creeper, CREEPER_SWELL_DIR_INDEX),
        Some(EntityDataValue::VarInt(-1))
    );
}

/// `has_live_targets` agrees with `player_targets` presence and excludes
/// creative/spectator/dead-row players without changing tie order.
#[test]
fn has_live_targets_matches_player_targets_presence() {
    let (mut state, player, _) = setup(2.0);
    assert!(crate::hostile::has_live_targets(&state));
    for mode in [
        crate::commands::GameMode::Creative,
        crate::commands::GameMode::Spectator,
    ] {
        state.players.get_mut(&player).unwrap().game_mode = mode;
        assert!(
            !crate::hostile::has_live_targets(&state),
            "{mode:?} admits no targets"
        );
    }
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Adventure;
    assert!(crate::hostile::has_live_targets(&state));
    // Dead entity row excludes the player exactly like `player_targets`.
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.body_mut(slot).alive = false;
    assert!(!crate::hostile::has_live_targets(&state));
}

#[test]
fn locked_session_admits_no_monster_targets_and_unlock_restores() {
    let (mut state, player, _) = setup(2.0);
    state.require_plugin_login = true;
    let connection = state.players[&player].connection;
    assert!(crate::hostile::has_live_targets(&state));
    state.locked_sessions.insert(connection);
    assert!(
        !crate::hostile::has_live_targets(&state),
        "locked session is not a live target"
    );
    state.locked_sessions.remove(&connection);
    assert!(crate::hostile::has_live_targets(&state));
}

/// A wet enderman stages its drowning damage by dense slot before the serial
/// hostile pass; the exploding creeper that pass removes sits ahead of it, so
/// swap-removal moves the enderman and its staged slot would equal the store
/// length when `end_fight::crystal_hits` reads it right after merge
/// (`EntityStore::body` index out of bounds in the live 20-bot session).
#[test]
fn staged_damage_follows_a_slot_moved_by_the_exploding_creeper_removal() {
    let (mut state, player, centre) = setup(2.0);
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    // No block loot: the store must end exactly at the staged slot.
    state
        .gamerules
        .insert("mob_griefing".into(), "false".into());
    state.weather = (crate::commands::Weather::Rain, 1_000_000);
    let creeper_type = type_id(&state, "minecraft:creeper");
    let enderman_type = type_id(&state, "minecraft:enderman");
    let creeper = state.spawn_mob(creeper_type, centre, 20.0);
    let enderman = state.spawn_mob(
        enderman_type,
        [centre[0] + 6.0, centre[1], centre[2] + 6.0],
        4_000.0,
    );
    let slot = state.entities.slot_of(enderman).unwrap();
    state.entities.row_mut(slot).controls.no_ai = true;
    for _ in 1..=FUSE_TICKS + 5 {
        run_tick(&mut state, &[]);
    }
    assert!(state.entities.slot_of(creeper).is_none(), "exploded");
    let slot = state.entities.slot_of(enderman).expect("enderman survives");
    assert!(state.entities.row(slot).health < 4_000.0, "rain hurt it");
    assert!(state.players.contains_key(&player));
}
