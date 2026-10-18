//! Offline native population fixture: no commands, clients, ticks or natural spawning.
//! See ADR 0013 for the opt-in large-world invocation and retained-world path.
//!
//! The default remains a new seed-42 world, 300 villagers and one shared house.
//! To extend an IMPORTED COPY (never the pristine import), explicitly set:
//! SOLARIS_POPULATION_EXISTING=1 SOLARIS_POPULATION_SEED=-712714449760912068
//! SOLARIS_POPULATION_COUNT=5000 SOLARIS_POPULATION_HOUSES=1
//! SOLARIS_POPULATION_ROOT=<copied native world>
//! SOLARIS_POPULATION_ORIGIN_X=8192 SOLARIS_POPULATION_ORIGIN_Y=310
//! SOLARIS_POPULATION_ORIGIN_Z=8192
//! Run this exact test with --nocapture. Origin is the northwest FLOOR block;
//! house mode requires an entirely air-filled, valid-height rectangular volume.
//! Houses occupy 6x6 blocks on a 7-block grid with a continuous stone walkway.
//! Each adult farmer owns its bed head and composter through typed native claims.
//! Durable zero-radius tickets activate the whole grid when the real server next
//! runs, even headlessly. This fixture never runs a tick or creates settlements.
//! Use the real native create/bind commands at the receipted quarter-grid anchors;
//! POI allocation does not automatically create settlements or bind membership.
//! House mode relocates the COPY's world spawn to the grid, disables spawn_mobs
//! and freezes day_time at 6000 through advance_time=false. Pre-existing entities
//! remain unchanged. population.json includes original spawn/time, grid bounds,
//! safe player position and deterministic bind groups.
use super::*;
use std::io::{BufRead, Write};

const CHILD: &str = "SOLARIS_POPULATION_CHILD";
const COUNT: &str = "SOLARIS_POPULATION_COUNT";
const ROOT: &str = "SOLARIS_POPULATION_ROOT";
const READY: &str = "NATIVE_POPULATION_DURABLE";
const EXISTING: &str = "SOLARIS_POPULATION_EXISTING";
const SEED: &str = "SOLARIS_POPULATION_SEED";
const HOUSES: &str = "SOLARIS_POPULATION_HOUSES";

fn opt_in(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) if value == "1" => true,
        Err(std::env::VarError::NotPresent) => false,
        _ => panic!("{name} must be absent or exactly 1"),
    }
}

#[derive(Clone, Copy)]
struct HouseGrid {
    origin: [i32; 3],
    columns: usize,
    rows: usize,
}

impl HouseGrid {
    fn new(count: usize, origin: [i32; 3]) -> Self {
        let side = count.isqrt();
        let columns = side + usize::from(side * side < count);
        let grid = Self {
            origin,
            columns,
            rows: count.div_ceil(columns),
        };
        grid.end();
        grid
    }

    /// Exclusive bounds, including walkways and roof.
    fn end(self) -> [i32; 3] {
        [
            self.origin[0]
                .checked_add(i32::try_from(self.columns.checked_mul(7).unwrap()).unwrap())
                .unwrap(),
            self.origin[1].checked_add(5).unwrap(),
            self.origin[2]
                .checked_add(i32::try_from(self.rows.checked_mul(7).unwrap()).unwrap())
                .unwrap(),
        ]
    }

    fn house(self, index: usize) -> [i32; 3] {
        [
            self.origin[0] + i32::try_from(index % self.columns).unwrap() * 7,
            self.origin[1],
            self.origin[2] + i32::try_from(index / self.columns).unwrap() * 7,
        ]
    }

    fn position(self, index: usize) -> [f64; 3] {
        let [x, y, z] = self.house(index);
        [f64::from(x) + 2.5, f64::from(y) + 1.0, f64::from(z) + 2.5]
    }

    fn centre_index(self, count: usize) -> usize {
        (self.rows / 2 * self.columns + self.columns / 2).min(count - 1)
    }

    fn settlement_centres(self) -> serde_json::Value {
        let mut centres = Vec::new();
        for (north_south, rows) in [
            (0, 0..self.rows.div_ceil(2)),
            (1, self.rows.div_ceil(2)..self.rows),
        ] {
            for (west_east, columns) in [
                (0, 0..self.columns.div_ceil(2)),
                (1, self.columns.div_ceil(2)..self.columns),
            ] {
                if rows.is_empty() || columns.is_empty() {
                    continue;
                }
                // Anchor the operator in a real house's clear interior, not
                // the mathematical midpoint (which can be a solid wall).
                let x = self.origin[0]
                    + i32::try_from((columns.start + columns.end) / 2 * 7).unwrap()
                    + 2;
                let z =
                    self.origin[2] + i32::try_from((rows.start + rows.end) / 2 * 7).unwrap() + 2;
                let half_x = (columns.len() * 7) as f64 / 2.0;
                let half_z = (rows.len() * 7) as f64 / 2.0;
                let radius = (mc_physics::mth::length2(half_x, half_z).ceil() as u32).max(180);
                centres.push(serde_json::json!({
                    "name": format!("falcons_smoke_{}", north_south * 2 + west_east),
                    "anchor": [x, self.origin[1] + 1, z],
                    "radius": radius,
                    "columns": [columns.start, columns.end],
                    "rows": [rows.start, rows.end],
                    "ranges": "start inclusive, end exclusive; column=index%grid.columns, row=index/grid.columns",
                }));
            }
        }
        serde_json::Value::Array(centres)
    }
    fn platform_origin(self) -> [i32; 3] {
        [
            self.origin[0].checked_sub(2).unwrap(),
            self.origin[1],
            self.origin[2].checked_sub(2).unwrap(),
        ]
    }

    fn platform_end(self) -> [i32; 3] {
        let end = self.end();
        [
            end[0].checked_add(2).unwrap(),
            end[1],
            end[2].checked_add(2).unwrap(),
        ]
    }

    fn chunks(self) -> rustc_hash::FxHashMap<crate::boundary::ChunkPos, i32> {
        let origin = self.platform_origin();
        let end = self.platform_end();
        let mut chunks = rustc_hash::FxHashMap::default();
        for x in origin[0].div_euclid(16)..=(end[0] - 1).div_euclid(16) {
            for z in origin[2].div_euclid(16)..=(end[2] - 1).div_euclid(16) {
                chunks.insert(crate::boundary::ChunkPos { x, z }, 0);
            }
        }
        chunks
    }

    fn prepare(self, state: &mut TickState, blocks: &HouseBlocks) {
        let chunks = self.chunks();
        state.world.ensure_blocking(&chunks);
        let origin = self.platform_origin();
        let end = self.platform_end();
        for chunk in chunks.keys() {
            let geometry = state
                .world
                .chunk(*chunk)
                .expect("grid chunk loaded")
                .geometry();
            assert!(
                self.origin[1] >= geometry.min_y() && end[1] <= geometry.max_y(),
                "house grid outside native chunk geometry"
            );
            assert!(
                state
                    .world
                    .chunk(*chunk)
                    .unwrap()
                    .generated_entities()
                    .iter()
                    .all(|marker| state.generated_entity_claims.contains(&marker.claim)),
                "grid chunk {chunk:?} contains unclaimed template inhabitants; choose another origin to keep live population exact"
            );
        }
        // Validate the entire volume before modifying any blocks. Imported
        // buildings/terrain are not disposable fixture space.
        for x in origin[0]..end[0] {
            for z in origin[2]..end[2] {
                for y in self.origin[1]..end[1] {
                    assert_eq!(
                        state.world.block_at(x, y, z),
                        Some(blocks.air),
                        "house grid would overwrite an existing block at {x},{y},{z}; choose another origin"
                    );
                }
            }
        }
        for slot in state.entities.slots() {
            let pos = state.entities.pos(slot);
            let row = state.entities.row(slot);
            assert!(
                pos[0] + state.entities.body(slot).half_width <= f64::from(origin[0])
                    || pos[0] - state.entities.body(slot).half_width >= f64::from(end[0])
                    || pos[2] + state.entities.body(slot).half_width <= f64::from(origin[2])
                    || pos[2] - state.entities.body(slot).half_width >= f64::from(end[2])
                    || pos[1] + state.entities.body(slot).height <= f64::from(self.origin[1])
                    || pos[1] >= f64::from(end[1]),
                "house grid intersects existing entity {}",
                row.uuid
            );
        }
        for x in origin[0]..end[0] {
            for z in origin[2]..end[2] {
                state.world.set_block(x, origin[1], z, blocks.stone);
                if x == origin[0] || x == end[0] - 1 || z == origin[2] || z == end[2] - 1 {
                    for y in origin[1] + 1..origin[1] + 3 {
                        state.world.set_block(x, y, z, blocks.planks);
                    }
                }
            }
        }
        for chunk in chunks.keys() {
            let owner = format!("solaris:population-grid:{}:{}", chunk.x, chunk.z);
            assert!(
                !state.tickets.contains_key(&owner),
                "population ticket already exists"
            );
            state.tickets.insert(owner, (*chunk, 0));
        }
    }
}

fn recover_population(config: &CoreConfig) -> TickState {
    let mut state =
        recover_dimension_state(config, crate::dimensions::Dimension::Overworld).unwrap();
    for dimension in [
        crate::dimensions::Dimension::Nether,
        crate::dimensions::Dimension::End,
    ] {
        state.other_dimensions.insert(
            dimension,
            Box::new(recover_dimension_state(config, dimension).unwrap()),
        );
    }
    state
}

fn population_baseline(state: &TickState) -> serde_json::Value {
    let mut dimensions = serde_json::Map::new();
    for world in std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        let entities: Vec<_> = world
            .entities
            .slots()
            .map(|slot| {
                let row = world.entities.row(slot);
                serde_json::json!({
                    "uuid": row.uuid,
                    "position": world.entities.pos(slot),
                    "health": row.health,
                    "persistent": row.persistent,
                })
            })
            .collect();
        let villagers = world
            .entities
            .slots()
            .filter(|&slot| world.entities.row(slot).villager.is_some())
            .count();
        dimensions.insert(
            world.dimension.name().into(),
            serde_json::json!({
                "entities": entities, "count": world.entities.len(), "villagers": villagers,
            }),
        );
    }
    serde_json::Value::Object(dimensions)
}

/// Default lattice, offset past the 6x6 fixture house at `[0, 200, 0]` (z 6.25+) so no
/// villager is seeded inside its walls.
fn position(index: usize) -> [f64; 3] {
    [
        (index % 1024) as f64 * 0.5 + 0.25,
        201.0,
        (index / 1024) as f64 * 0.5 + 6.25,
    ]
}

fn identity(index: usize) -> uuid::Uuid {
    uuid::Uuid::from_u128(0x4736_504f_5055_4c41_0000_0000_0000_0000 | (index as u128 + 1))
}

fn rss_bytes() -> u64 {
    status_memory_bytes("VmRSS:")
}

fn status_memory_bytes(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix(field)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .expect("Linux process memory reading")
        * 1024
}

struct HouseBlocks {
    stone: u32,
    planks: u32,
    job: u32,
    foot: u32,
    head: u32,
    air: u32,
}

impl HouseBlocks {
    fn new(state: &TickState) -> Self {
        let block = |name: &str| {
            state
                .registry
                .blocks
                .block(&mc_data::Identifier::parse(name).unwrap())
                .unwrap()
                .default
                .0
        };
        let bed_id = mc_data::Identifier::parse("minecraft:red_bed").unwrap();
        let default = state.registry.blocks.block(&bed_id).unwrap().default;
        let mut properties = state
            .registry
            .blocks
            .by_id(default)
            .unwrap()
            .properties
            .clone();
        let mut bed = |part: &str| {
            for (key, value) in &mut properties {
                if key == "facing" {
                    *value = "south".into();
                }
                if key == "part" {
                    *value = part.into();
                }
            }
            state
                .registry
                .blocks
                .by_name_and_props(&bed_id, &properties)
                .unwrap()
                .0
        };
        Self {
            stone: block("minecraft:stone"),
            planks: block("minecraft:oak_planks"),
            job: block("minecraft:composter"),
            foot: bed("foot"),
            head: bed("head"),
            air: state.registry.states.air,
        }
    }
}

/// The original 6x6 house, translated without changing default fixture geometry.
fn house(state: &mut TickState, origin: [i32; 3], blocks: &HouseBlocks) {
    for dx in 0..6 {
        for dz in 0..6 {
            let x = origin[0] + dx;
            let z = origin[2] + dz;
            state.world.set_block(x, origin[1], z, blocks.stone);
            state.world.set_block(x, origin[1] + 4, z, blocks.planks);
            for dy in 1..4 {
                let wall = dx == 0 || dx == 5 || dz == 0 || dz == 5;
                let doorway = dx == 2 && dz == 0 && dy < 3;
                state.world.set_block(
                    x,
                    origin[1] + dy,
                    z,
                    if wall && !doorway {
                        blocks.planks
                    } else {
                        blocks.air
                    },
                );
            }
        }
    }
    let [x, y, z] = origin;
    state.world.set_block(x + 1, y + 1, z + 2, blocks.foot);
    state.world.set_block(x + 1, y + 1, z + 3, blocks.head);
    state.world.set_block(x + 4, y + 1, z + 4, blocks.job);
}

fn assert_house(state: &TickState, origin: [i32; 3], blocks: &HouseBlocks) {
    let [x, y, z] = origin;
    for (pos, expected) in [
        ([x + 1, y, z + 2], blocks.stone),
        ([x + 1, y + 1, z + 2], blocks.foot),
        ([x + 1, y + 1, z + 3], blocks.head),
        ([x + 4, y + 1, z + 4], blocks.job),
        ([x + 1, y + 4, z + 2], blocks.planks),
        ([x, y + 2, z + 2], blocks.planks),
        ([x + 2, y + 1, z], blocks.air),
        ([x + 2, y + 2, z], blocks.air),
    ] {
        assert_eq!(
            state.world.block_at(pos[0], pos[1], pos[2]),
            Some(expected),
            "house at {origin:?}"
        );
    }
    use crate::settlements::allocator::{PoiKind, poi_kind};
    assert!(matches!(
        poi_kind(&state.registry, blocks.head),
        Some(PoiKind::Home)
    ));
    assert!(matches!(
        poi_kind(&state.registry, blocks.job),
        Some(PoiKind::Job(crate::villager::VillagerProfession::Farmer))
    ));
}

#[test]
fn native_offline_population_save_kill_reopen() {
    let count = std::env::var(COUNT)
        .map(|value| value.parse::<usize>().unwrap())
        .unwrap_or(300);
    assert!(count > 0 && count < i32::MAX as usize);
    let existing = opt_in(EXISTING);
    let seed = std::env::var(SEED)
        .map(|value| value.parse::<i64>().unwrap())
        .unwrap_or(42);
    if existing {
        assert!(
            std::env::var_os(ROOT).is_some(),
            "existing mode needs explicit ROOT"
        );
        assert!(
            std::env::var_os(SEED).is_some(),
            "existing mode needs explicit SEED"
        );
    }
    let grid = opt_in(HOUSES).then(|| {
        let coordinate = |axis, default| {
            std::env::var(format!("SOLARIS_POPULATION_ORIGIN_{axis}"))
                .map(|value| value.parse::<i32>().unwrap())
                .unwrap_or(default)
        };
        HouseGrid::new(
            count,
            [coordinate("X", 0), coordinate("Y", 200), coordinate("Z", 0)],
        )
    });
    if std::env::var_os(CHILD).is_some() {
        let root = std::path::PathBuf::from(std::env::var_os(ROOT).unwrap());
        if existing {
            assert!(
                root.join("native/state.redb").is_file(),
                "existing mode requires a native world COPY"
            );
            let root = root.canonicalize().unwrap();
            let pristine = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.analysis/maps/falcons-rock/native-original");
            if let Ok(pristine) = pristine.canonicalize() {
                assert!(
                    !root.starts_with(pristine),
                    "pristine Falcons source is immutable; use a COPY"
                );
            }
        } else {
            assert!(
                !root.join("native/state.redb").exists(),
                "fixture refuses an existing world without SOLARIS_POPULATION_EXISTING=1"
            );
        }
        let config = CoreConfig::new(root.clone(), seed);
        let mut state = recover_population(&config);
        let initial_tick = state.tick;
        for world in
            std::iter::once(&state).chain(state.other_dimensions.values().map(|world| &**world))
        {
            for slot in world.entities.slots() {
                assert_ne!(
                    world.entities.row(slot).uuid.as_u128() >> 64,
                    identity(0).as_u128() >> 64,
                    "deterministic population cohort already exists; refusing duplicate seeding"
                );
            }
        }
        let baseline = population_baseline(&state);
        let baseline_count = state.entities.len();
        let baseline_villagers = baseline[state.dimension.name()]["villagers"]
            .as_u64()
            .unwrap();
        let original_spawn = state.world.world_spawn();
        let original_day_time = state.day_time;
        let blocks = HouseBlocks::new(&state);
        if let Some(grid) = grid {
            grid.prepare(&mut state, &blocks);
            for index in 0..count {
                house(&mut state, grid.house(index), &blocks);
            }
            // House mode is a stable live-correctness population, not a natural
            // spawning/breeding workload. The default storage fixture is unchanged.
            state.gamerules.insert("spawn_mobs".into(), "false".into());
            state
                .gamerules
                .insert("advance_time".into(), "false".into());
            state.day_time = 6000;
            let position = grid.position(grid.centre_index(count));
            state.world.set_world_spawn(
                position.map(|coordinate| coordinate.floor() as i32),
                [0.0; 2],
            );
        } else {
            state.world.ensure_blocking(
                &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
                    .into_iter()
                    .collect(),
            );
            house(&mut state, [0, 200, 0], &blocks);
        }
        let villager_type = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
            .unwrap();
        for index in 0..count {
            let pos = grid.map_or_else(|| position(index), |grid| grid.position(index));
            let id = state.spawn_mob(villager_type, pos, 20.0);
            let slot = state.entities.slot_of(id).unwrap();
            state.entities.set_uuid(slot, identity(index));
            let row = state.entities.row_mut(slot);
            row.persistent = true;
            row.health = 10.0 + (index % 11) as f32;
            let villager = row.villager.as_mut().unwrap();
            if let Some(grid) = grid {
                let [x, y, z] = grid.house(index);
                villager.food_level = 0;
                villager.home = Some(crate::boundary::BlockPos {
                    x: x + 1,
                    y: y + 1,
                    z: z + 3,
                });
                villager.job_site = Some(crate::boundary::BlockPos {
                    x: x + 4,
                    y: y + 1,
                    z: z + 4,
                });
                villager.home_dimension = state.dimension.name().into();
                villager.job_site_dimension = state.dimension.name().into();
                villager.data.profession = crate::villager::VillagerProfession::Farmer;
                villager.data_finalized = true;
                crate::villager::sync_appearance(&mut state, slot);
            } else {
                villager.food_level = (index % 13) as i32;
            }
        }
        let rss = rss_bytes();
        println!("\npopulation seeded count={count} rss_bytes={rss}");
        std::io::stdout().flush().unwrap();
        let mut manifest = read_manifest_for_population(&config);
        let mut writer =
            JournalWriter::open_recovered(&root, 1, manifest.tick, initial_tick).unwrap();
        let tick = initial_tick.checked_add(1).unwrap();
        state.tick = tick;
        let (work_tx, work_rx) = crate::core::journal_work_channel();
        let (done_tx, _) = std::sync::mpsc::channel();
        // Linux resets VmHWM to current RSS without sampling or a polling thread.
        std::fs::write("/proc/self/clear_refs", b"5").unwrap();
        let capture_start = std::time::Instant::now();
        assert!(start_checkpoint(&mut state, &work_tx, &done_tx, tick));
        let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
            panic!("expected checkpoint")
        };
        let capture_seconds = capture_start.elapsed().as_secs_f64();
        let capture_peak_rss = status_memory_bytes("VmHWM:");
        let captured_chunks = job.regions.written().len();
        println!(
            "population eager capture complete seconds={capture_seconds} \
             peak_rss_bytes={capture_peak_rss} captured_chunks={captured_chunks}"
        );
        std::io::stdout().flush().unwrap();
        writer.append(tick, tick, &[]).unwrap();
        std::fs::write("/proc/self/clear_refs", b"5").unwrap();
        let commit_start = std::time::Instant::now();
        write_checkpoint(&root, &job, &state.plugins, &mut writer, &mut manifest).unwrap();
        let commit_seconds = commit_start.elapsed().as_secs_f64();
        let commit_peak_rss = status_memory_bytes("VmHWM:");
        println!(
            "population native commit complete seconds={commit_seconds} \
             peak_rss_bytes={commit_peak_rss}"
        );
        std::io::stdout().flush().unwrap();
        let bytes = std::fs::metadata(root.join("native/state.redb"))
            .unwrap()
            .len();
        let house_count = if grid.is_some() { count } else { 1 };
        let grid_receipt = grid.map(|grid| {
            let end = grid.end();
            serde_json::json!({
                "origin": grid.origin,
                "span": [end[0] - grid.origin[0], 5, end[2] - grid.origin[2]],
                "end_exclusive": end,
                "platform_origin": grid.platform_origin(),
                "platform_end_exclusive": grid.platform_end(),
                "columns": grid.columns, "rows": grid.rows, "cell_spacing": 7,
                "player_position": grid.position(grid.centre_index(count)),
                "cohort_uuid_rule": "4736504f-5055-4c41-0000-000000000000 OR (zero-based index + 1) in low 64 bits",
                "activation_tickets": grid.chunks().len(),
                "boundary": "two-block-high oak-plank wall around continuous stone platform",
            })
        });
        let baseline_total: u64 = baseline
            .as_object()
            .unwrap()
            .values()
            .map(|dimension| dimension["count"].as_u64().unwrap())
            .sum();
        let settlement_anchors = grid
            .map(|grid| grid.settlement_centres())
            .unwrap_or_else(|| serde_json::json!([]));
        // These are targets for the parent's real console bind commands, not
        // memberships manufactured by the offline fixture.
        let cohort: Vec<_> = grid
            .map(|grid| {
                (0..count)
                    .map(|index| {
                        let [x, y, z] = grid.house(index);
                        let north_south =
                            usize::from(index / grid.columns >= grid.rows.div_ceil(2));
                        let west_east =
                            usize::from(index % grid.columns >= grid.columns.div_ceil(2));
                        serde_json::json!({
                            "uuid": identity(index), "index": index,
                            "settlement": format!("falcons_smoke_{}", north_south * 2 + west_east),
                            "home": [x + 1, y + 1, z + 3],
                            "job": [x + 4, y + 1, z + 4],
                            "position": grid.position(index),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        std::fs::write(root.join("population.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "count": count, "added_count": count, "seed": seed, "existing_world": existing,
            "baseline": baseline, "baseline_count": baseline_count,
            "baseline_villagers": baseline_villagers, "expected_count": baseline_count + count,
            "expected_villagers": baseline_villagers + count as u64,
            "baseline_total_entities": baseline_total,
            "expected_total_entities": baseline_total + count as u64,
            "initial_tick": initial_tick, "native_bytes": bytes,
            "native_bytes_per_villager": bytes as f64 / count as f64,
            "live_rss_bytes": rss, "live_rss_bytes_per_villager": rss as f64 / count as f64,
            "rss_scope": "whole process after seeding, before eager checkpoint capture; includes registry/chunks/allocator",
            "checkpoint_capture_seconds": capture_seconds,
            "checkpoint_capture_peak_rss_bytes": capture_peak_rss,
            "checkpoint_captured_chunks": captured_chunks,
            "checkpoint_commit_seconds": commit_seconds,
            "checkpoint_commit_peak_rss_bytes": commit_peak_rss,
            "checkpoint_peak_rss_scope": "child process VmHWM reset before each phase; includes resident file mappings, excludes parent and unmapped filesystem cache",
            "house_count": house_count, "beds": house_count, "job_pois": house_count,
            "grid": grid_receipt, "world_spawn": state.world.world_spawn(),
            "original_world_spawn": original_spawn,
            "spawn_position": state.world.world_spawn(),
            "settlement_anchors": settlement_anchors, "cohort": cohort,
            "cohort_membership_seeded": false,
            "cohort_first_uuid": identity(0), "cohort_last_uuid": identity(count - 1),
            "spawn_mobs": crate::command_rules::boolean(&state, "spawn_mobs"),
            "advance_time": crate::command_rules::boolean(&state, "advance_time"),
            "day_time": state.day_time, "original_day_time": original_day_time,
            "checkpoint_tick": manifest.tick
        })).unwrap()).unwrap();
        println!("\n{READY}");
        std::io::stdout().flush().unwrap();
        // Keep every live owner and writer open: parent must SIGKILL, not shut down.
        loop {
            std::thread::park();
        }
    }

    let temporary = tempfile::tempdir().unwrap();
    let root = std::env::var_os(ROOT)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| temporary.path().join("world"));
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "core::native_population_tests::native_offline_population_save_kill_reopen",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env(ROOT, &root)
        .env(COUNT, count.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    let output = child.stdout.take().unwrap();
    let ready = std::io::BufReader::new(output).lines().any(|line| {
        let line = line.unwrap();
        println!("{line}");
        line == READY
    });
    if !ready {
        panic!(
            "fixture child exited before durable checkpoint: {:?}",
            child.wait().unwrap()
        );
    }
    child.kill().unwrap();
    let status = child.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(9));
    let report = root.join("population.json");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    let config = CoreConfig::new(root.clone(), seed);
    let mut state = recover_population(&config);
    let mut reopened_total = 0;
    for world in
        std::iter::once(&state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        let baseline = &receipt["baseline"][world.dimension.name()];
        let added = if world.dimension == crate::dimensions::Dimension::Overworld {
            count
        } else {
            0
        };
        assert_eq!(
            world.entities.len(),
            baseline["count"].as_u64().unwrap() as usize + added
        );
        let villagers = world
            .entities
            .slots()
            .filter(|&slot| world.entities.row(slot).villager.is_some())
            .count();
        assert_eq!(
            villagers,
            baseline["villagers"].as_u64().unwrap() as usize + added
        );
        reopened_total += world.entities.len();
        for entity in baseline["entities"].as_array().unwrap() {
            let uuid: uuid::Uuid = serde_json::from_value(entity["uuid"].clone()).unwrap();
            let slot = world
                .entities
                .slot_of_uuid(uuid)
                .expect("baseline entity preserved");
            let row = world.entities.row(slot);
            let pos: [f64; 3] = serde_json::from_value(entity["position"].clone()).unwrap();
            assert_eq!(world.entities.pos(slot), pos);
            assert_eq!(serde_json::json!(row.health), entity["health"]);
            assert_eq!(serde_json::json!(row.persistent), entity["persistent"]);
        }
    }
    for index in [0, count / 2, count - 1] {
        let slot = state.entities.slot_of_uuid(identity(index)).unwrap();
        let row = state.entities.row(slot);
        assert_eq!(
            state.entities.pos(slot),
            grid.map_or_else(|| position(index), |grid| grid.position(index))
        );
        assert_eq!(row.health, 10.0 + (index % 11) as f32);
        assert!(row.persistent);
        assert_eq!(
            row.villager.as_ref().unwrap().food_level,
            if grid.is_some() {
                0
            } else {
                (index % 13) as i32
            }
        );
    }
    assert_eq!(state.tick, receipt["checkpoint_tick"].as_u64().unwrap());
    assert_eq!(state.tick, receipt["initial_tick"].as_u64().unwrap() + 1);
    let blocks = HouseBlocks::new(&state);
    if let Some(grid) = grid {
        state.world.ensure_blocking(&grid.chunks());
        for index in 0..count {
            let slot = state.entities.slot_of_uuid(identity(index)).unwrap();
            let row = state.entities.row(slot);
            let villager = row.villager.as_ref().unwrap();
            let [x, y, z] = grid.house(index);
            assert_eq!(
                villager.home,
                Some(crate::boundary::BlockPos {
                    x: x + 1,
                    y: y + 1,
                    z: z + 3
                })
            );
            assert_eq!(
                villager.job_site,
                Some(crate::boundary::BlockPos {
                    x: x + 4,
                    y: y + 1,
                    z: z + 4
                })
            );
            assert_eq!(villager.home_dimension, state.dimension.name());
            assert_eq!(villager.job_site_dimension, state.dimension.name());
            assert_eq!(
                villager.data.profession,
                crate::villager::VillagerProfession::Farmer
            );
            assert!(villager.data_finalized && !row.controls.no_ai);
            assert_house(&state, grid.house(index), &blocks);
        }
        let origin = grid.platform_origin();
        let end = grid.platform_end();
        for x in origin[0]..end[0] {
            for z in origin[2]..end[2] {
                assert_eq!(state.world.block_at(x, origin[1], z), Some(blocks.stone));
                if x == origin[0] || x == end[0] - 1 || z == origin[2] || z == end[2] - 1 {
                    for y in origin[1] + 1..origin[1] + 3 {
                        assert_eq!(state.world.block_at(x, y, z), Some(blocks.planks));
                    }
                }
            }
        }
        for chunk in grid.chunks().keys() {
            assert_eq!(
                state.tickets[&format!("solaris:population-grid:{}:{}", chunk.x, chunk.z)],
                (*chunk, 0)
            );
        }
        assert!(!crate::command_rules::boolean(&state, "spawn_mobs"));
        assert!(!crate::command_rules::boolean(&state, "advance_time"));
        assert_eq!(state.day_time, 6000);
        assert_eq!(
            state.world.world_spawn(),
            grid.position(grid.centre_index(count))
                .map(|coordinate| coordinate.floor() as i32)
        );
    } else {
        state.world.ensure_blocking(
            &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
                .into_iter()
                .collect(),
        );
        assert_house(&state, [0, 200, 0], &blocks);
    }
    receipt["reopened_total_entities"] = serde_json::json!(reopened_total);
    receipt["reopened_villagers"] = serde_json::json!(
        state
            .entities
            .slots()
            .filter(|&slot| state.entities.row(slot).villager.is_some())
            .count()
    );
    receipt["reopened_added_count"] = serde_json::json!(count);
    receipt["reopened_houses"] = receipt["house_count"].clone();
    receipt["reopened_beds"] = receipt["beds"].clone();
    receipt["reopened_job_pois"] = receipt["job_pois"].clone();
    receipt["reopened_count"] = serde_json::json!(state.entities.len());
    receipt["writer_exit_signal"] = serde_json::json!(status.signal());
    receipt["spot_checks"] = serde_json::json!([0, count / 2, count - 1]);
    receipt["house_spot_checks"] = serde_json::json!("floor, roof, bed foot/head, job POI");
    receipt["status"] = serde_json::json!("passed");
    std::fs::write(&report, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    println!("{}", receipt);
}

#[test]
fn native_population_house_grid_has_unique_homes_jobs_and_quarter_coverage() {
    let grid = HouseGrid::new(5000, [8192, 310, 8192]);
    assert_eq!([grid.columns, grid.rows], [71, 71]);
    assert_eq!(grid.end(), [8689, 315, 8689]);
    assert_eq!(grid.platform_origin(), [8190, 310, 8190]);
    assert_eq!(grid.platform_end(), [8691, 315, 8691]);
    let mut uuids = rustc_hash::FxHashSet::default();
    let mut homes = rustc_hash::FxHashSet::default();
    let mut jobs = rustc_hash::FxHashSet::default();
    let centres = grid.settlement_centres();
    for index in 0..5000 {
        let [x, y, z] = grid.house(index);
        assert!(uuids.insert(identity(index)));
        assert!(homes.insert([x + 1, y + 1, z + 3]));
        assert!(jobs.insert([x + 4, y + 1, z + 4]));
        let column = index % grid.columns;
        let row = index / grid.columns;
        let matches: Vec<_> = centres
            .as_array()
            .unwrap()
            .iter()
            .filter(|centre| {
                let columns = centre["columns"].as_array().unwrap();
                let rows = centre["rows"].as_array().unwrap();
                (columns[0].as_u64().unwrap() as usize..columns[1].as_u64().unwrap() as usize)
                    .contains(&column)
                    && (rows[0].as_u64().unwrap() as usize..rows[1].as_u64().unwrap() as usize)
                        .contains(&row)
            })
            .collect();
        assert_eq!(matches.len(), 1);
        let centre = matches[0];
        let anchor: [i32; 3] = serde_json::from_value(centre["anchor"].clone()).unwrap();
        let radius = centre["radius"].as_u64().unwrap() as f64;
        assert!(radius <= 256.0);
        for pos in [
            grid.position(index),
            [f64::from(x + 1), f64::from(y + 1), f64::from(z + 3)],
            [f64::from(x + 4), f64::from(y + 1), f64::from(z + 4)],
        ] {
            let distance_squared: f64 = pos
                .iter()
                .zip(anchor)
                .map(|(coordinate, origin)| (coordinate - f64::from(origin)).powi(2))
                .sum();
            assert!(distance_squared <= radius * radius);
        }
    }
}

fn read_manifest_for_population(config: &CoreConfig) -> CheckpointManifest {
    crate::native_checkpoint::tests::read_manifest(config).unwrap()
}
