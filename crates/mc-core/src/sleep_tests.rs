//! Beds on a stone platform in open sky: sleeping, refusals, skipping the
//! night, leaving the bed and the respawn point.

use std::sync::Arc;

use crate::boundary::{
    BlockPos, ConnectionId, EntityDataValue, EntityId, Hand, PlayerChange, PlayerInput,
};
use crate::commands::{GameMode, Weather};
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;

/// Platform height: far above the generated terrain.
const Y: i32 = 200;
const NIGHT: i64 = 13_000;
const DAY: i64 = 6_000;
const SLEEPING_POSE: u8 = 2;

fn test_state() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry loads"));
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().expect("test terrain spawn");
    let world_spawn = [spawn.block_x, spawn.surface_y + 1, spawn.block_z];
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        world_spawn,
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn join(state: &mut TickState, connection: u64, uuid: uuid::Uuid) -> EntityId {
    let _ = run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(connection),
            uuid,
            name: format!("sleeper{connection}"),
            permission_level: 0,
        }],
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state
        .connections
        .entity_of(ConnectionId(connection))
        .expect("joined")
}

fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(name).expect("id");
    let default = state.registry.blocks.block(&id).expect("block").default;
    let mut full = state
        .registry
        .blocks
        .by_id(default)
        .expect("default")
        .properties
        .clone();
    for (key, value) in props {
        if let Some(entry) = full.iter_mut().find(|(name, _)| name == key) {
            entry.1 = (*value).to_owned();
        }
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &full)
        .expect("state")
        .0
}

/// One player on a stone platform with a red bed: foot at `foot`, head east
/// of it.
struct Rig {
    state: TickState,
    player: EntityId,
    foot: BlockPos,
    head: BlockPos,
}

impl Rig {
    fn new() -> Self {
        let mut state = test_state();
        let player = join(&mut state, 1, uuid::Uuid::new_v4());
        let slot = state.entities.slot_of(player).expect("slot");
        let at = state.entities.pos(slot);
        let foot = BlockPos {
            x: at[0].floor() as i32,
            y: Y + 1,
            z: at[2].floor() as i32,
        };
        let head = BlockPos {
            x: foot.x + 1,
            ..foot
        };
        let stone = state.registry.states.stone;
        for dx in -6..=6 {
            for dz in -6..=6 {
                state.world.set_block(foot.x + dx, Y, foot.z + dz, stone);
            }
        }
        let bed = |part| {
            block(
                &state,
                "minecraft:red_bed",
                &[("facing", "east"), ("part", part)],
            )
        };
        let (foot_state, head_state) = (bed("foot"), bed("head"));
        state.world.set_block(foot.x, foot.y, foot.z, foot_state);
        state.world.set_block(head.x, head.y, head.z, head_state);
        state.world.fold_overlay();
        // On the platform, beside the bed at the vanilla 3-block reach edge,
        // so mobs near it stay in range of a player (mobs far from every
        // player despawn).
        state.entities.set_pos(
            slot,
            [
                f64::from(foot.x) + 0.5,
                f64::from(Y + 1),
                f64::from(foot.z) - 2.5,
            ],
        );
        Self {
            state,
            player,
            foot,
            head,
        }
    }

    fn use_bed(&mut self, connection: u64) -> Vec<(ConnectionId, Vec<PlayerChange>)> {
        self.use_bed_at(connection, self.foot)
    }

    fn use_bed_at(
        &mut self,
        connection: u64,
        position: BlockPos,
    ) -> Vec<(ConnectionId, Vec<PlayerChange>)> {
        let (_, drains, _) = run_tick(
            &mut self.state,
            &[PlayerInput::UseItemOn {
                connection: ConnectionId(connection),
                hand: Hand::Main,
                position,
                face: 1,
                cursor: [0.5; 3],
                sequence: 1,
            }],
        );
        drains
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            let _ = run_tick(&mut self.state, &[]);
        }
    }

    fn pose(&self, entity: EntityId) -> u8 {
        let slot = self.state.entities.slot_of(entity).expect("slot");
        self.state.entities.row(slot).appearance.pose
    }

    /// The published sleeping position of `entity`.
    fn lying_in(&self, entity: EntityId) -> Option<Option<[i32; 3]>> {
        let slot = self.state.entities.slot_of(entity).expect("slot");
        self.state
            .entities
            .row(slot)
            .appearance
            .extra
            .iter()
            .find_map(|(index, value)| match value {
                EntityDataValue::OptionalBlockPos(pos)
                    if *index == crate::sleep::SLEEPING_POS_DATA_INDEX =>
                {
                    Some(*pos)
                }
                _ => None,
            })
    }
}

fn said(drains: &[(ConnectionId, Vec<PlayerChange>)], text: &str) -> bool {
    drains
        .iter()
        .flat_map(|(_, queue)| queue.iter())
        .any(|change| matches!(change, PlayerChange::Chat { message, .. } if message == text))
}

#[test]
fn bed_advancement_is_queued_only_after_new_lie_down() {
    let mut rig = Rig::new();
    let registry = Arc::get_mut(&mut rig.state.registry).expect("fixture registry is unshared");
    Arc::make_mut(&mut registry.advancements)
        .definitions
        .insert(
            "test:sleep".into(),
            serde_json::from_value(serde_json::json!({
                "criteria": { "sleep": { "trigger": "minecraft:slept_in_bed" } },
                "requirements": [["sleep"]],
            }))
            .expect("sleep advancement fixture"),
        );
    rig.state.advancement_events.clear();
    rig.state.day_time = DAY;
    assert!(crate::sleep::use_bed(
        &mut rig.state,
        rig.player,
        ConnectionId(1),
        rig.foot,
    ));
    assert!(rig.state.advancement_events.is_empty());
    rig.state.day_time = NIGHT;
    assert!(crate::sleep::use_bed(
        &mut rig.state,
        rig.player,
        ConnectionId(1),
        rig.foot,
    ));
    assert_eq!(rig.state.advancement_events.len(), 1);
    assert!(rig.state.sleepers.contains_key(&rig.player));
    assert!(crate::sleep::use_bed(
        &mut rig.state,
        rig.player,
        ConnectionId(1),
        rig.foot,
    ));
    assert_eq!(rig.state.advancement_events.len(), 1);
}

#[test]
fn sleeping_at_night_sets_respawn_and_skips_the_night() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    rig.state.weather = (Weather::Rain, 5_000);
    let drains = rig.use_bed(1);
    let head = rig.head;
    assert!(!said(&drains, "You can sleep only at night"));
    assert_eq!(
        rig.state
            .sleepers
            .get(&rig.player)
            .map(|sleeper| sleeper.bed),
        Some(head)
    );
    assert_eq!(rig.pose(rig.player), SLEEPING_POSE);
    assert_eq!(
        rig.lying_in(rig.player),
        Some(Some([head.x, head.y, head.z]))
    );
    let slot = rig.state.entities.slot_of(rig.player).expect("slot");
    assert_eq!(
        rig.state.entities.pos(slot),
        [
            f64::from(head.x) + 0.5,
            f64::from(head.y) + 0.6875,
            f64::from(head.z) + 0.5
        ]
    );
    assert_eq!(
        rig.state.players[&rig.player].respawn_point,
        Some([head.x, head.y, head.z])
    );
    // 99 more ticks: still the same night, still in bed.
    rig.run(98);
    assert!(rig.state.sleepers.contains_key(&rig.player));
    assert_eq!(rig.state.day_time, NIGHT + 99);
    // The 100th tick of sleep skips to the next morning and wakes everyone.
    rig.run(1);
    assert_eq!(rig.state.day_time, 24_000);
    assert_eq!(rig.state.weather.0, Weather::Clear);
    assert!(rig.state.sleepers.is_empty());
    assert_eq!(rig.pose(rig.player), 0);
    assert_eq!(rig.lying_in(rig.player), Some(None));
}

#[test]
fn sleeping_from_the_head_half_works_too() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    let head = rig.head;
    rig.use_bed_at(1, head);
    assert_eq!(
        rig.state
            .sleepers
            .get(&rig.player)
            .map(|sleeper| sleeper.bed),
        Some(head)
    );
}

#[test]
fn sleeping_is_refused_by_day() {
    let mut rig = Rig::new();
    rig.state.day_time = DAY;
    let drains = rig.use_bed(1);
    assert!(said(&drains, "You can sleep only at night"));
    assert!(rig.state.sleepers.is_empty());
    assert_eq!(rig.pose(rig.player), 0);
    assert_eq!(
        rig.state.players[&rig.player].respawn_point,
        Some([rig.head.x, rig.head.y, rig.head.z])
    );
    // The last tick before dusk and the first of dawn are still day.
    rig.state.day_time = 12_540;
    assert!(said(&rig.use_bed(1), "You can sleep only at night"));
    rig.state.day_time = 23_460;
    assert!(said(&rig.use_bed(1), "You can sleep only at night"));
}

#[test]
fn a_thunderstorm_allows_sleeping_by_day() {
    let mut rig = Rig::new();
    rig.state.day_time = DAY;
    crate::weather::set_command(&mut rig.state, Weather::Thunder, Some(5_000));
    let drains = rig.use_bed(1);
    assert!(!said(&drains, "You can sleep only at night"));
    assert!(rig.state.sleepers.contains_key(&rig.player));
    rig.run(99);
    assert_eq!(rig.state.day_time, 24_000);
    assert_eq!(rig.state.weather.0, Weather::Clear);
}

#[test]
fn sleeping_is_refused_with_a_monster_near() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    let zombie = rig.state.registry.zombie_type_id;
    let near = [
        f64::from(rig.head.x) + 0.5,
        f64::from(rig.head.y),
        f64::from(rig.head.z) + 6.5,
    ];
    let monster = rig.state.spawn_mob(zombie, near, 20.0);
    let drains = rig.use_bed(1);
    assert!(said(
        &drains,
        "You may not rest now; there are monsters nearby"
    ));
    assert!(rig.state.sleepers.is_empty());
    assert_eq!(
        rig.state.players[&rig.player].respawn_point,
        Some([rig.head.x, rig.head.y, rig.head.z])
    );
    // Out of range (9 blocks away) it no longer matters.
    let slot = rig.state.entities.slot_of(monster).expect("zombie");
    let far = [near[0], near[1], f64::from(rig.head.z) + 9.5];
    rig.state.entities.set_pos(slot, far);
    let drains = rig.use_bed(1);
    assert!(!said(
        &drains,
        "You may not rest now; there are monsters nearby"
    ));
    assert!(rig.state.sleepers.contains_key(&rig.player));
}

#[test]
fn small_slime_does_not_block_sleep() {
    // BED-MONSTER-ELIGIBILITY: harmless small slimes never block sleep.
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    let slime = rig
        .state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:slime").unwrap())
        .unwrap();
    let near = [
        f64::from(rig.head.x) + 0.5,
        f64::from(rig.head.y),
        f64::from(rig.head.z) + 2.5,
    ];
    rig.state.spawn_mob(slime, near, 4.0);
    let drains = rig.use_bed(1);
    assert!(
        !said(&drains, "You may not rest now; there are monsters nearby"),
        "small slime must not block sleep"
    );
    assert!(rig.state.sleepers.contains_key(&rig.player));
}

#[test]
fn sealed_bed_refuses_entry_as_obstructed() {
    // BED-OBSTRUCTED-ENTRY: no wake-up space means refused entry.
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    let stone = rig.state.registry.states.stone;
    // Seal the perimeter at bed level and the exit tier above both halves.
    for dx in -1..=2 {
        for dz in -1..=1 {
            if dz != 0 || !(0..=1).contains(&dx) {
                rig.state
                    .world
                    .set_block(rig.foot.x + dx, rig.foot.y, rig.foot.z + dz, stone);
            }
            rig.state
                .world
                .set_block(rig.foot.x + dx, rig.foot.y + 1, rig.foot.z + dz, stone);
            rig.state
                .world
                .set_block(rig.foot.x + dx, rig.foot.y + 2, rig.foot.z + dz, stone);
        }
    }
    rig.state.world.fold_overlay();
    let drains = rig.use_bed(1);
    assert!(said(&drains, "This bed is obstructed"));
    assert!(rig.state.sleepers.is_empty());
}

#[test]
fn distant_bed_use_is_too_far() {
    // BED-SLEEP-REACH: vanilla 3-block sleeping limit beats packet reach.
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    let slot = rig.state.entities.slot_of(rig.player).expect("slot");
    rig.state.entities.set_pos(
        slot,
        [
            f64::from(rig.foot.x) - 4.5,
            f64::from(rig.foot.y),
            f64::from(rig.foot.z),
            // 4.5 blocks from the foot, beyond the 3-block sleep limit but
            // inside packet reach.
        ],
    );
    let drains = rig.use_bed(1);
    assert!(said(&drains, "You are too far away"));
    assert!(rig.state.sleepers.is_empty());
}

#[test]
fn night_is_not_skipped_while_a_player_is_awake() {
    let mut rig = Rig::new();
    let other = join(&mut rig.state, 2, uuid::Uuid::new_v4());
    rig.state.day_time = NIGHT;
    rig.use_bed(1);
    rig.run(150);
    assert!(rig.state.sleepers.contains_key(&rig.player));
    assert_eq!(rig.state.day_time, NIGHT + 151);
    // A spectator does not hold the night back.
    rig.state.players.get_mut(&other).expect("other").game_mode = GameMode::Spectator;
    rig.run(1);
    assert_eq!(rig.state.day_time, 24_000);
    assert!(rig.state.sleepers.is_empty());
}

#[test]
fn a_bed_holds_one_sleeper() {
    let mut rig = Rig::new();
    let other = join(&mut rig.state, 2, uuid::Uuid::new_v4());
    let player_slot = rig.state.entities.slot_of(rig.player).expect("slot");
    let position = rig.state.entities.pos(player_slot);
    let other_slot = rig.state.entities.slot_of(other).expect("slot");
    rig.state.entities.set_pos(other_slot, position);
    rig.state.day_time = NIGHT;
    rig.use_bed(1);
    let drains = rig.use_bed(2);
    assert!(said(&drains, "This bed is occupied"));
    assert_eq!(rig.state.sleepers.len(), 1);
}

#[test]
fn leaving_the_bed_wakes_the_player() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    rig.use_bed(1);
    let _ = run_tick(
        &mut rig.state,
        &[PlayerInput::LeaveBed {
            connection: ConnectionId(1),
        }],
    );
    assert!(rig.state.sleepers.is_empty());
    assert_eq!(rig.pose(rig.player), 0);
    assert_eq!(rig.lying_in(rig.player), Some(None));
    // The respawn point stays.
    assert!(rig.state.players[&rig.player].respawn_point.is_some());
    assert_eq!(rig.state.day_time, NIGHT + 2);
}

#[test]
fn breaking_the_bed_or_taking_a_hit_wakes_the_sleeper() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    rig.use_bed(1);
    let air = rig.state.registry.states.air;
    let head = rig.head;
    rig.state.world.set_block(head.x, head.y, head.z, air);
    rig.state.world.fold_overlay();
    rig.run(1);
    assert!(rig.state.sleepers.is_empty());

    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    rig.use_bed(1);
    let slot = rig.state.entities.slot_of(rig.player).expect("slot");
    rig.state.entities.row_mut(slot).health -= 2.0;
    rig.run(1);
    assert!(rig.state.sleepers.is_empty());
}

#[test]
fn respawn_uses_the_bed_while_it_stands() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    rig.use_bed(1);
    let head = rig.head;
    let slot = rig.state.entities.slot_of(rig.player).expect("slot");
    rig.state.entities.body_mut(slot).alive = false;
    rig.state.entities.row_mut(slot).health = 0.0;
    rig.state
        .players
        .get_mut(&rig.player)
        .unwrap()
        .vitals
        .health = 0.0;
    let _ = run_tick(
        &mut rig.state,
        &[PlayerInput::Respawn {
            connection: ConnectionId(1),
        }],
    );
    assert!(rig.state.sleepers.is_empty());
    let slot = rig.state.entities.slot_of(rig.player).expect("slot");
    let at = rig.state.entities.pos(slot);
    // Beside the bed, on the platform.
    assert_eq!(at[1], f64::from(Y + 1));
    assert!((at[0] - (f64::from(head.x) + 0.5)).abs() <= 1.0, "{at:?}");
    assert!((at[2] - (f64::from(head.z) + 0.5)).abs() <= 1.0, "{at:?}");
    assert_ne!(
        [at[0], at[2]],
        [f64::from(head.x) + 0.5, f64::from(head.z) + 0.5]
    );

    // Bed gone: the natural spawn.
    let air = rig.state.registry.states.air;
    for pos in [rig.foot, rig.head] {
        rig.state.world.set_block(pos.x, pos.y, pos.z, air);
    }
    rig.state.world.fold_overlay();
    let natural = crate::natural_spawn::player_spawn(
        &rig.state,
        u64::from(rig.player.0 as u32) ^ rig.state.tick,
    );
    rig.state.entities.body_mut(slot).alive = false;
    rig.state.entities.row_mut(slot).health = 0.0;
    rig.state
        .players
        .get_mut(&rig.player)
        .unwrap()
        .vitals
        .health = 0.0;
    let _ = run_tick(
        &mut rig.state,
        &[PlayerInput::Respawn {
            connection: ConnectionId(1),
        }],
    );
    let slot = rig.state.entities.slot_of(rig.player).expect("slot");
    assert_eq!(rig.state.entities.pos(slot), natural);
}

#[test]
fn the_respawn_point_survives_leaving_and_rejoining() {
    let uuid = uuid::Uuid::new_v4();
    let mut state = test_state();
    let player = join(&mut state, 1, uuid);
    state
        .players
        .get_mut(&player)
        .expect("player")
        .respawn_point = Some([4, 70, -3]);
    let record = crate::core::player_record_for(&state, player, state.tick).expect("record");
    assert_eq!(record.respawn, Some([4, 70, -3]));
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: ConnectionId(1),
        }],
    );
    let player = join(&mut state, 2, uuid);
    assert_eq!(state.players[&player].respawn_point, Some([4, 70, -3]));
}

#[test]
fn sleeping_percentage_command_changes_the_next_deep_sleep_threshold() {
    let mut rig = Rig::new();
    join(&mut rig.state, 2, uuid::Uuid::new_v4());
    rig.state.day_time = NIGHT;
    rig.state.weather = (Weather::Rain, 5_000);
    rig.use_bed(1);
    rig.run(100);
    assert!(rig.state.sleepers.contains_key(&rig.player));
    assert!(
        rig.state.day_time < 24_000,
        "default 100 percent needs both players"
    );
    run_tick(
        &mut rig.state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            command: "gamerule playersSleepingPercentage 50".into(),
            permission_level: 4,
        }],
    );
    assert!(rig.state.day_time >= 24_000);
    assert!(rig.state.sleepers.is_empty());
    assert_eq!(rig.state.weather.0, Weather::Clear);
}

#[test]
fn sleeping_percentage_above_hundred_disables_skip_and_zero_still_needs_a_sleeper() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    for value in [101, 0] {
        run_tick(
            &mut rig.state,
            &[PlayerInput::Command {
                connection: ConnectionId(1),
                command: format!("gamerule players_sleeping_percentage {value}"),
                permission_level: 4,
            }],
        );
        if value == 101 {
            rig.use_bed(1);
            rig.run(100);
            assert!(rig.state.sleepers.contains_key(&rig.player));
            assert!(rig.state.day_time < 24_000);
        } else {
            assert!(
                rig.state.day_time >= 24_000,
                "zero still requires one deep sleeper"
            );
            assert!(rig.state.sleepers.is_empty());
        }
    }
    rig.state.day_time = NIGHT;
    rig.run(100);
    assert!(
        rig.state.day_time < 24_000,
        "zero does not skip an empty bed"
    );
}

#[test]
fn sleep_statistics_only_award_accepted_entry_and_reset_rest_timer() {
    use mc_data::statistics::CustomStat;
    let mut rig = Rig::new();
    rig.state
        .players
        .get_mut(&rig.player)
        .unwrap()
        .statistics
        .set_custom(CustomStat::TimeSinceRest, 500);
    rig.state.day_time = DAY;
    crate::sleep::use_bed(&mut rig.state, rig.player, ConnectionId(1), rig.foot);
    assert_eq!(
        rig.state.players[&rig.player]
            .statistics
            .get_custom(CustomStat::SleepInBed),
        0
    );
    assert_eq!(
        rig.state.players[&rig.player]
            .statistics
            .get_custom(CustomStat::TimeSinceRest),
        500
    );
    rig.state.day_time = NIGHT;
    crate::sleep::use_bed(&mut rig.state, rig.player, ConnectionId(1), rig.foot);
    assert_eq!(
        rig.state.players[&rig.player]
            .statistics
            .get_custom(CustomStat::SleepInBed),
        1
    );
    assert_eq!(
        rig.state.players[&rig.player]
            .statistics
            .get_custom(CustomStat::TimeSinceRest),
        0
    );
    crate::sleep::use_bed(&mut rig.state, rig.player, ConnectionId(1), rig.foot);
    assert_eq!(
        rig.state.players[&rig.player]
            .statistics
            .get_custom(CustomStat::SleepInBed),
        1
    );
}

#[test]
fn weather_gap_daytime_bed_sets_spawn_without_awarding_sleep() {
    let mut rig = Rig::new();
    rig.state.day_time = DAY;
    crate::sleep::use_bed(&mut rig.state, rig.player, ConnectionId(1), rig.foot);
    assert_eq!(
        rig.state.players[&rig.player].respawn_point,
        Some([rig.head.x, rig.head.y, rig.head.z])
    );
    assert!(rig.state.sleepers.is_empty());
}

#[test]
fn weather_gap_sleeping_fits_low_ceiling_and_wakes_at_safe_exit() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    let stone = rig.state.registry.states.stone;
    rig.state
        .world
        .set_block(rig.head.x, rig.head.y + 2, rig.head.z, stone);
    rig.use_bed(1);
    let slot = rig.state.entities.slot_of(rig.player).unwrap();
    assert_eq!(rig.state.entities.body(slot).height, 0.2);
    rig.run(20);
    assert!(rig.state.sleepers.contains_key(&rig.player));
    let exit = crate::sleep::respawn_position(&rig.state, rig.player).unwrap();
    crate::sleep::wake(&mut rig.state, rig.player);
    assert_eq!(rig.state.entities.pos(slot), exit);
    assert_eq!(rig.state.entities.body(slot).height, 1.8);
}

#[test]
fn weather_gap_bed_exit_includes_foot_perimeter() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    rig.use_bed(1);
    let stone = rig.state.registry.states.stone;
    for [dx, dz] in [[1, 0], [-1, 0], [0, 1], [0, -1], [0, 0]] {
        rig.state
            .world
            .set_block(rig.head.x + dx, rig.head.y + 1, rig.head.z + dz, stone);
    }
    let exit = crate::sleep::respawn_position(&rig.state, rig.player).expect("foot exit");
    assert!(exit[0] <= f64::from(rig.foot.x) + 0.5 || exit[2] != f64::from(rig.head.z) + 0.5);
}

#[test]
fn weather_gap_clear_countdown_survives_sleep() {
    let mut rig = Rig::new();
    rig.state.day_time = NIGHT;
    rig.state.weather = (Weather::Clear, 10_000);
    rig.use_bed(1);
    rig.run(99);
    assert_eq!(rig.state.weather, (Weather::Clear, 9_900));
    assert!(rig.state.sleepers.is_empty());
}
