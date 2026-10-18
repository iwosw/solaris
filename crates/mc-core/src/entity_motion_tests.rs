//! Behavior tests for the production native movement/control kernels.
use crate::blueprint::MovementClass;
use crate::entity_motion::{MotionInput, breathe, look, panic, steer, step, wander};
use crate::species::AquaticControl;
use mc_physics::{Aabb, BlockMaterial, BlockSampler, EntityBody, Vec3};

struct Habitat<F>(F);
impl<F: Fn(i32, i32, i32) -> BlockMaterial> BlockSampler for Habitat<F> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        (self.0)(x, y, z)
    }
}

fn input(movement: MovementClass, position: Vec3) -> MotionInput {
    MotionInput {
        body: EntityBody {
            position,
            velocity: Vec3::ZERO,
            aabb: Aabb {
                half_width: 0.3,
                height: 0.6,
            },
            on_ground: movement == MovementClass::Walker,
        },
        movement,
        drive: [0.0; 3],
        impulse: [0.0; 2],
        rotation: [0.0; 3],
        look_target: None,
        climbs_walls: false,
        aquatic_control: AquaticControl::Other,
        air_supply: 300,
        levitation: 0.0,
        slow_falling: false,
        id: 1,
        tick: 0,
    }
}

#[test]
fn ground_moves_on_support_and_stops_at_a_tall_wall() {
    let world = Habitat(|x, y, _| {
        if y < 0 || (x == 1 && y < 5) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    });
    let mut state = input(MovementClass::Walker, Vec3::new(0.0, 0.0, 0.0));
    state.drive = [4.0, 0.0, 0.0];
    let moved = step(state, &world, |_, _| true);
    assert!(moved.body.position.x > 0.0);
    assert!(moved.body.on_ground);
    let mut state = input(MovementClass::Walker, Vec3::new(0.65, 0.0, 0.0));
    state.drive = [4.0, 0.0, 0.0];
    let blocked = step(state, &world, |_, _| true);
    assert!(blocked.body.position.x <= 0.7 + 1.0e-7);
    assert_eq!(blocked.body.velocity.x, 0.0);
    assert_eq!(blocked.body.position.y, 0.0);
}

#[test]
fn ground_obstacle_jump_finishes_climbing_a_full_block() {
    let world = Habitat(|x, y, _| {
        if y < 0 || (x == 1 && y == 0) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    });
    let mut body = input(MovementClass::Walker, Vec3::new(0.65, 0.0, 0.0)).body;
    let mut jumped = false;
    let mut atop = false;
    for tick in 0..32 {
        let mut state = input(MovementClass::Walker, body.position);
        state.body = body;
        state.drive = [4.0, 0.0, 0.0];
        state.tick = tick;
        body = step(state, &world, |_, _| true).body;
        jumped |= body.velocity.y > 0.0;
        atop |= body.position.y >= 1.0 && body.position.x > 1.0;
    }
    assert!(jumped);
    assert!(
        atop,
        "ground jump must cross, not repeatedly bump, a one-block obstacle"
    );
}

#[test]
fn low_ceiling_prevents_ground_obstacle_jump() {
    let world = Habitat(|x, y, _| {
        if y < 0 || y == 1 || (x == 1 && y == 0) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    });
    let mut state = input(MovementClass::Walker, Vec3::new(0.65, 0.0, 0.0));
    state.drive = [4.0, 0.0, 0.0];
    let result = step(state, &world, |_, _| true);
    assert_eq!(result.body.position.y, 0.0);
    assert!(result.body.position.x <= 0.7 + 1.0e-7);
}

#[test]
fn spider_wall_climb_is_bounded_and_clipped_at_ceiling() {
    let world = Habitat(|x, y, _| {
        if y < 0 || y == 2 || (x == 1 && y < 5) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    });
    let mut body = input(MovementClass::Walker, Vec3::new(0.65, 0.0, 0.0)).body;
    for tick in 0..16 {
        let before = body.position.y;
        let mut state = input(MovementClass::Walker, body.position);
        state.body = body;
        state.drive = [4.0, 0.0, 0.0];
        state.climbs_walls = true;
        state.tick = tick;
        body = step(state, &world, |_, _| true).body;
        assert!(body.position.y - before <= 0.2 + 1.0e-7);
        assert!(body.position.y + body.aabb.height <= 2.0 + 1.0e-7);
    }
    assert!(body.position.y > 1.0);
}

#[test]
fn missing_current_chunk_freezes_every_native_kind_without_air_loss() {
    let world = Habitat(|_, _, _| BlockMaterial::Solid);
    for kind in [
        MovementClass::Walker,
        MovementClass::Swimmer,
        MovementClass::Flyer,
        MovementClass::Amphibious,
        MovementClass::Stationary,
    ] {
        let mut state = input(kind, Vec3::new(18.0, 5.0, 0.0));
        state.drive = [4.0, 3.0, 2.0];
        state.body.velocity = Vec3::new(4.0, -6.0, 2.0);
        state.air_supply = -19;
        let result = step(state, &world, |_, _| false);
        assert_eq!(result.body.position, Vec3::new(18.0, 5.0, 0.0));
        assert_eq!(result.body.velocity, Vec3::ZERO);
        assert_eq!(result.air_supply, -19);
        assert!(!result.drowning);
    }
}

#[test]
fn spider_does_not_climb_missing_neighbor_chunk_boundary() {
    let world = Habitat(|x, y, _| {
        if x >= 16 || y < 0 {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    });
    let mut state = input(MovementClass::Walker, Vec3::new(15.65, 0.0, 0.0));
    state.climbs_walls = true;
    state.drive = [4.0, 0.0, 0.0];
    let result = step(state, &world, |x, _| x < 16);
    assert_eq!(result.body.position.y, 0.0);
    assert!(result.body.position.x <= 15.7 + 1.0e-7);
}

#[test]
fn flying_moves_vertically_without_gravity_and_clips_ceiling() {
    let air = Habitat(|_, _, _| BlockMaterial::Air);
    let mut state = input(MovementClass::Flyer, Vec3::new(0.0, 1.0, 0.0));
    state.drive = [2.0, 3.0, 0.0];
    let result = step(state, &air, |_, _| true);
    assert_eq!(result.body.velocity, Vec3::new(2.0, 3.0, 0.0));
    assert_eq!(result.body.position, Vec3::new(0.1, 1.15, 0.0));
    assert!(result.rotation[1] < 0.0);
    let ceiling = Habitat(|_, y, _| {
        if y >= 2 {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    });
    let mut state = input(MovementClass::Flyer, Vec3::new(0.0, 1.35, 0.0));
    state.drive = [0.0, 4.0, 0.0];
    let result = step(state, &ceiling, |_, _| true);
    assert!(result.body.position.y <= 1.4 + 1.0e-7);
    assert_eq!(result.body.velocity.y, 0.0);
    let idle = step(
        input(MovementClass::Flyer, Vec3::new(0.0, 5.0, 0.0)),
        &air,
        |_, _| true,
    );
    assert_eq!(idle.body.position.y, 5.0);
}

#[test]
fn aquatic_and_amphibious_steer_vertically_in_water() {
    let water = Habitat(|_, _, _| BlockMaterial::Water);
    for kind in [MovementClass::Swimmer, MovementClass::Amphibious] {
        let mut state = input(kind, Vec3::new(0.0, 1.0, 0.0));
        state.drive = [2.0, -3.0, 1.0];
        state.air_supply = 0;
        let result = step(state, &water, |_, _| true);
        assert!(result.body.position.x > 0.0);
        assert!(result.body.position.y < 1.0);
        assert!(result.body.position.z > 0.0);
        assert_eq!(result.air_supply, 300);
        assert!(!result.drowning);
    }
}

#[test]
fn aquatic_navigation_stays_in_known_water_and_avoids_dry_or_solid_destination() {
    for boundary in [
        BlockMaterial::Air,
        BlockMaterial::Lava,
        BlockMaterial::Solid,
    ] {
        let world = Habitat(|x, y, _| {
            if x >= 1 || y >= 2 {
                boundary
            } else {
                BlockMaterial::Water
            }
        });
        let mut state = input(MovementClass::Swimmer, Vec3::new(0.65, 1.35, 0.0));
        state.drive = [4.0, 4.0, 0.0];
        let result = step(state, &world, |_, _| true);
        assert!(result.body.position.x + result.body.aabb.half_width <= 1.0 + 1.0e-7);
        assert!(result.body.position.y + result.body.aabb.height <= 2.0 + 1.0e-7);
        assert_ne!(
            result.body.velocity,
            Vec3::ZERO,
            "bounded alternatives should escape the shore"
        );
    }
}

#[test]
fn fish_flops_on_land_while_squid_does_not_and_amphibious_walks() {
    let ground = Habitat(|_, y, _| {
        if y < 0 {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    });
    let mut fish = input(MovementClass::Swimmer, Vec3::ZERO);
    fish.aquatic_control = AquaticControl::Fish;
    fish.body.on_ground = true;
    let fish = step(fish, &ground, |_, _| true);
    assert!(fish.body.position.y > 0.0);
    assert!(fish.body.velocity.y > 0.0);
    let mut squid = input(MovementClass::Swimmer, Vec3::ZERO);
    squid.aquatic_control = AquaticControl::Squid;
    squid.body.on_ground = true;
    squid.drive = [3.0, 3.0, 0.0];
    let squid = step(squid, &ground, |_, _| true);
    assert_eq!(squid.body.position, Vec3::ZERO);
    let mut amphibious = input(MovementClass::Amphibious, Vec3::ZERO);
    amphibious.body.on_ground = true;
    amphibious.drive = [3.0, 3.0, 0.0];
    let amphibious = step(amphibious, &ground, |_, _| true);
    assert!(amphibious.body.position.x > 0.0);
    assert_eq!(amphibious.body.position.y, 0.0);
    assert_eq!(amphibious.air_supply, 300);
}

#[test]
fn stationary_never_wanders_or_integrates_but_can_look_at_target() {
    let air = Habitat(|_, _, _| BlockMaterial::Air);
    for tick in 0..240 {
        assert_eq!(wander(MovementClass::Stationary, 1, tick, 4.0), [0.0; 3]);
    }
    let mut state = input(MovementClass::Stationary, Vec3::new(0.0, 5.0, 0.0));
    state.drive = [3.0, 3.0, 3.0];
    state.impulse = [4.0, 4.0];
    state.look_target = Some([1.0, 6.0, 1.0]);
    let result = step(state, &air, |_, _| true);
    assert_eq!(result.body.position, Vec3::new(0.0, 5.0, 0.0));
    assert_eq!(result.body.velocity, Vec3::ZERO);
    assert!(result.rotation[0] < 0.0 && result.rotation[1] < 0.0);
}

#[test]
fn aquatic_air_damage_boundary_resets_and_amphibious_never_suffocates_on_land() {
    let mut air = 300;
    for _ in 0..319 {
        (air, _) = breathe(air, false, MovementClass::Swimmer);
    }
    assert_eq!(air, -19);
    assert_eq!(breathe(air, false, MovementClass::Swimmer), (0, true));
    assert_eq!(breathe(air, true, MovementClass::Swimmer), (300, false));
    assert_eq!(breathe(air, false, MovementClass::Amphibious), (300, false));
}

#[test]
fn look_and_move_controls_use_target_height_and_bound_turn_rate() {
    let drive = steer(MovementClass::Flyer, [0.0; 3], [0.0, 2.0, 2.0], 4.0);
    assert!(drive[1] > 0.0 && drive[2] > 0.0);
    let ground = steer(MovementClass::Walker, [0.0; 3], [0.0, 2.0, 2.0], 4.0);
    assert_eq!(ground[1], 0.0);
    let rotation = look([0.0; 3], Vec3::ZERO, Vec3::ZERO, Some([2.0, 2.0, 0.0]));
    assert_eq!(rotation[0], -90.0);
    assert_eq!(rotation[1], -40.0);
    assert_eq!(rotation[2], rotation[0]);
}

#[test]
fn episodic_wander_produces_vertical_drive_only_for_three_dimensional_kinds() {
    for kind in [
        MovementClass::Flyer,
        MovementClass::Swimmer,
        MovementClass::Amphibious,
    ] {
        assert!((0..240).any(|tick| wander(kind, 1, tick, 4.0)[1].abs() > 0.001));
    }
    assert!((0..240).all(|tick| wander(MovementClass::Walker, 1, tick, 4.0)[1] == 0.0));
}

struct NativeHabitat {
    air: u32,
    ground: u32,
    water: u32,
}

impl mc_world::ChunkGenerator for NativeHabitat {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        );
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 64, z, mc_world::BlockStateId(self.ground));
                if (8..16).contains(&(pos.x * 16 + x as i32)) {
                    for y in 65..73 {
                        chunk.set_block(x, y, z, mc_world::BlockStateId(self.water));
                    }
                }
            }
        }
        chunk
    }
}

fn native_state() -> crate::tick::TickState {
    use std::sync::Arc;
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(NativeHabitat {
        air: registry.states.air,
        ground: registry.states.stone,
        water: registry.states.water,
    });
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [4, 65, 4],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = crate::tick::TickState::new(registry, world, 1, 512);
    state.tickets.insert(
        "motion".to_owned(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 1),
    );
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    state
}

fn spawn_named(
    state: &mut crate::tick::TickState,
    name: &str,
    position: [f64; 3],
) -> crate::entities::EntitySlot {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, position, 20.0);
    state.entities.slot_of(id).unwrap()
}

#[test]
fn native_tick_projects_blueprint_movement_and_rotation_in_each_habitat() {
    let mut state = native_state();
    let cow = spawn_named(&mut state, "minecraft:cow", [4.0, 65.0, 4.0]);
    let cod = spawn_named(&mut state, "minecraft:cod", [12.0, 68.0, 4.0]);
    let bat = spawn_named(&mut state, "minecraft:bat", [4.0, 69.0, 4.0]);
    let frog = spawn_named(&mut state, "minecraft:frog", [4.0, 65.0, 4.0]);
    let stand = spawn_named(&mut state, "minecraft:armor_stand", [6.0, 65.0, 4.0]);
    let slots = [cow, cod, bat, frog, stand];
    let starts = slots.map(|slot| state.entities.pos(slot));
    let mut moved = [false; 5];
    let mut vertical = [false; 5];
    for _ in 0..120 {
        crate::tick::run_tick(&mut state, &[]);
        for (index, slot) in slots.into_iter().enumerate() {
            let position = state.entities.pos(slot);
            moved[index] |= position != starts[index];
            vertical[index] |= (position[1] - starts[index][1]).abs() > 0.001;
        }
    }
    assert_eq!(moved, [true, true, true, true, false]);
    assert!(vertical[1] && vertical[2]);
    assert_eq!(state.entities.rot(stand), [0.0; 3]);
    assert_ne!(state.entities.rot(cod), [0.0; 3]);
    assert_ne!(state.entities.rot(bat), [0.0; 3]);
}

#[test]
fn native_tick_commits_aquatic_dry_air_damage_and_retains_counter() {
    let mut state = native_state();
    let cod = spawn_named(&mut state, "minecraft:cod", [4.0, 65.0, 4.0]);
    state.entities.body_mut(cod).air_supply = -19;
    state.entities.set_ground(cod, true);
    crate::tick::run_tick(&mut state, &[]);
    assert_eq!(state.entities.body(cod).air_supply, 0);
    assert_eq!(state.entities.row(cod).health, 18.0);
}

#[test]
fn panic_has_no_wander_idle_window_and_moves_away_from_known_attacker() {
    for tick in 0..240 {
        let drive = panic(MovementClass::Walker, 1, tick, 5.0, [0.0; 3], None);
        // The mth sin/cos tables are not exactly unit length (angle step 2π/65536).
        assert!((mc_physics::mth::length2(drive[0], drive[2]) - 5.0).abs() < 1.0e-3);
    }
    assert_eq!(
        panic(
            MovementClass::Walker,
            1,
            60,
            5.0,
            [1.0, 0.0, 0.0],
            Some([0.0; 3])
        ),
        [5.0, 0.0, 0.0]
    );
    assert_eq!(
        panic(
            MovementClass::Stationary,
            1,
            60,
            5.0,
            [1.0, 0.0, 0.0],
            Some([0.0; 3])
        ),
        [0.0; 3]
    );
}

#[test]
fn native_flee_fire_goal_panics_after_hurt_or_while_burning_even_during_wander_idle() {
    for burning in [false, true] {
        let mut state = native_state();
        let cow = spawn_named(&mut state, "minecraft:cow", [4.0, 65.0, 4.0]);
        let id = state.entities.body(cow).id;
        let speed = state.entities.row(cow).speed;
        state.tick = 60;
        assert_eq!(
            wander(MovementClass::Walker, id.0, state.tick, speed),
            [0.0; 3]
        );
        let body = state.entities.body_mut(cow);
        if burning {
            body.burn_ticks = 20;
        } else {
            body.hurt_at = Some(60);
        }
        state.entities.set_ground(cow, true);
        crate::tick::run_tick(&mut state, &[]);
        let velocity = state.entities.vel(cow);
        let expected = f64::from(speed) * if burning { 2.0 } else { 1.25 };
        assert!((mc_physics::mth::length2(velocity[0], velocity[2]) - expected).abs() < 1.0e-7);
    }
}

struct CowPenTerrain {
    air: u32,
    stone: u32,
}

impl mc_world::ChunkGenerator for CowPenTerrain {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        );
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 48, z, mc_world::BlockStateId(self.stone));
                // The same pen spans air over low terrain and a hill to clear.
                if pos == (mc_world::ChunkPos { x: 0, z: 0 })
                    && (8..=11).contains(&x)
                    && (4..=5).contains(&z)
                {
                    for y in 49..=65 {
                        chunk.set_block(x, y, z, mc_world::BlockStateId(self.stone));
                    }
                }
            }
        }
        chunk
    }
}

#[test]
fn same_tick_fill_and_summon_keeps_cows_healthy_above_the_pen_floor() {
    use std::sync::Arc;
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = Arc::new(CowPenTerrain {
        air: registry.states.air,
        stone: registry.states.stone,
    });
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 64, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = crate::tick::TickState::new(registry, world, 1, 512);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state
        .gamerules
        .insert("random_tick_speed".into(), "0".into());
    state.tickets.insert(
        "cow-pen".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 1),
    );
    state.world.ensure_blocking(&state.desired_chunks());
    // coreperf sends fill and the first summons together in a 32-command batch.
    let commands = [
        "fill 0 63 0 15 63 15 minecraft:grass_block",
        "fill 0 64 0 15 68 15 minecraft:air",
        "fill -1 64 -1 16 65 -1 minecraft:stone",
        "fill -1 64 16 16 65 16 minecraft:stone",
        "fill -1 64 0 -1 65 15 minecraft:stone",
        "fill 16 64 0 16 65 15 minecraft:stone",
        "summon minecraft:cow 4.5 64 4.5",
        "summon minecraft:cow 6.5 64 4.5",
        "summon minecraft:cow 8.5 64 4.5",
        "summon minecraft:cow 10.5 64 4.5",
    ];
    let inputs: Vec<_> = commands
        .into_iter()
        .map(|text| {
            crate::boundary::PlayerInput::ConsoleCommand(
                crate::command_console::ConsoleCommand::new(text.into()).0,
            )
        })
        .collect();
    crate::tick::run_tick(&mut state, &inputs);
    let cows: Vec<_> = state
        .entities
        .slots()
        .map(|slot| state.entities.body(slot).id)
        .collect();
    assert_eq!(cows.len(), 4);
    let mut first_breach = None;
    let mut first_damage = None;
    for _ in 0..80 {
        for &id in &cows {
            let slot = state.entities.slot_of(id).expect("cow survives");
            let pos = state.entities.pos(slot);
            let width = state.entities.body(slot).half_width;
            if first_breach.is_none()
                && (pos[1] < 64.0
                    || pos[0] - width < 0.0
                    || pos[0] + width > 16.0
                    || pos[2] - width < 0.0
                    || pos[2] + width > 16.0)
            {
                first_breach = Some((state.tick, id, pos));
            }
        }
        let mut hazards = Vec::new();
        crate::contact_hazards::run(&mut state, &mut hazards);
        if first_damage.is_none() {
            first_damage = hazards
                .into_iter()
                .next()
                .map(|intent| (state.tick, intent.kind));
        }
        crate::tick::run_tick(&mut state, &[]);
    }
    let final_cows: Vec<_> = cows
        .iter()
        .map(|&id| {
            let slot = state.entities.slot_of(id).expect("cow survives");
            (
                id,
                state.entities.pos(slot),
                state.entities.row(slot).health,
            )
        })
        .collect();
    eprintln!(
        "cow-pen: ticks={} first_breach={first_breach:?} first_damage={first_damage:?} final={final_cows:?}",
        state.tick
    );
    assert!(
        first_breach.is_none(),
        "accepted command blocks must contain cows before physics"
    );
    assert!(
        first_damage.is_none(),
        "cows must not suffocate in cleared terrain"
    );
    assert!(final_cows.iter().all(|(_, _, health)| *health == 10.0));
}
