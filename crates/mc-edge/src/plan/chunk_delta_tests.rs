//! Delta-driven chunk fanout against the full-walk oracle: two caches, one
//! with the publish delta and one without, plan identical viewers over the
//! same outputs and must emit byte-identical packets and end in identical
//! viewer state.
use super::*;
use crate::fanout::{AddressedPacket, Priority, publish_with_registries};
use chunk_delta::{ChunkChangeKind, ChunkDeltaLog, ChunkPlanStats};
use mc_core::boundary::{BlockChange, ChangeSet, TickOutput, WorldSnapshot};
use mc_core::dimensions::Dimension;
use mc_world::chunk::{BlockPos, ChunkPos, LightSection};
use std::collections::BTreeMap;

struct Registries {
    biomes: mc_data::Registry,
    blocks: mc_world::BlockRegistry,
    items: mc_data::items::ItemRegistry,
    block_entity_types: mc_data::block_entity_types::BlockEntityTypeRegistry,
}

impl Registries {
    fn new() -> Self {
        Self {
            biomes: crate::chunks::test_biome_registry(&["minecraft:plains"]),
            blocks: mc_world::BlockRegistry::from_report(
                &mc_data::blocks::solaris_required_blocks_report(),
            )
            .expect("blocks build"),
            items: mc_data::items::solaris_required_items(),
            block_entity_types: mc_data::block_entity_types::solaris_required_block_entity_types(),
        }
    }

    fn view(&self) -> PlanRegistries<'_> {
        PlanRegistries {
            biomes: &self.biomes,
            blocks: &self.blocks,
            items: &self.items,
            block_entity_types: &self.block_entity_types,
        }
    }
}

/// Resident world with the core's snapshot sharing: the chunk list stays the
/// same `Arc` until residency or a revision changes.
struct World {
    chunks: BTreeMap<(i32, i32), (u64, Arc<mc_world::chunk::Chunk>)>,
    list: Arc<Vec<mc_core::boundary::SnapshotChunk>>,
    changes: ChangeSet,
    revision: u64,
    tick: u64,
}

impl World {
    fn square(min: i32, max: i32) -> Self {
        let mut world = Self {
            chunks: BTreeMap::new(),
            list: Arc::default(),
            changes: ChangeSet::default(),
            revision: 0,
            tick: 0,
        };
        for x in min..max {
            for z in min..max {
                world.load(x, z);
            }
        }
        world.publish();
        world
    }

    fn next_revision(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    fn load(&mut self, x: i32, z: i32) {
        let revision = self.next_revision();
        let (_, chunk) = chunk_at(x, z);
        self.chunks.insert((x, z), (revision, chunk));
        self.list = Arc::default();
    }

    fn unload(&mut self, x: i32, z: i32) {
        self.chunks.remove(&(x, z));
        self.list = Arc::default();
    }

    fn resident(&self, x: i32, z: i32) -> bool {
        self.chunks.contains_key(&(x, z))
    }

    fn edit(&mut self, x: i32, z: i32, mutate: impl FnOnce(&mut mc_world::chunk::Chunk)) {
        let revision = self.next_revision();
        let (held, chunk) = self.chunks.get_mut(&(x, z)).expect("resident chunk");
        let mut next = (**chunk).clone();
        mutate(&mut next);
        *chunk = Arc::new(next);
        *held = revision;
        self.list = Arc::default();
    }

    /// A block change with the matching change-set entry.
    fn place(&mut self, x: i32, z: i32, state: u32) {
        self.edit(x, z, |chunk| {
            chunk.set_block(1, 64, 1, mc_world::BlockStateId(state));
        });
        self.changes.blocks.push(BlockChange {
            position: BlockPos {
                x: x * 16 + 1,
                y: 64,
                z: z * 16 + 1,
            },
            state,
        });
    }

    /// Light moves without any block change, like a neighbour's emitter.
    fn relight(&mut self, x: i32, z: i32, level: u8) {
        self.edit(x, z, |chunk| {
            chunk.section_lights[8].block = Some(LightSection::uniform(level));
        });
    }

    /// A new revision whose content and light are unchanged: only a full
    /// resend can explain it to a client.
    fn touch(&mut self, x: i32, z: i32) {
        self.edit(x, z, |_| {});
    }

    /// Close the tick: one output carrying this tick's change set.
    fn publish(&mut self) -> TickOutput {
        self.tick += 1;
        if self.list.is_empty() {
            self.list = Arc::new(
                self.chunks
                    .iter()
                    .map(
                        |(&(x, z), (revision, chunk))| mc_core::boundary::SnapshotChunk {
                            pos: ChunkPos { x, z },
                            revision: *revision,
                            chunk: Arc::clone(chunk),
                        },
                    )
                    .collect(),
            );
        }
        TickOutput {
            tick: self.tick,
            snapshot: Arc::new(WorldSnapshot {
                tick: self.tick,
                day_time: 1000 + self.tick as i64,
                day_time_rate: 1.0,
                chunks: Arc::clone(&self.list),
                ..Default::default()
            }),
            changes: Arc::new(std::mem::take(&mut self.changes)),
            times: Default::default(),
        }
    }
}

/// One viewer set per cache; both see exactly the same inputs.
struct Side {
    cache: ChunkPacketCache,
    viewers: Vec<ViewerState>,
    /// Wall time spent inside `publish_with_registries` (exploratory, debug build).
    elapsed: std::time::Duration,
}

struct Rig {
    registries: Registries,
    delta: Side,
    full: Side,
    view_distance: i32,
}

/// Per-viewer inputs for one publish.
#[derive(Clone)]
struct Inputs {
    credit: Vec<usize>,
    skipped: Vec<bool>,
    included: Vec<bool>,
}

impl Inputs {
    fn all(viewers: usize) -> Self {
        Self {
            credit: vec![usize::MAX; viewers],
            skipped: vec![false; viewers],
            included: vec![true; viewers],
        }
    }
}

type Wire = Vec<(u64, Vec<u8>, bool)>;

fn wire(packets: Vec<AddressedPacket>) -> Wire {
    packets
        .into_iter()
        .map(|packet| {
            (
                packet.connection.0,
                packet.data.to_vec(),
                packet.priority == Priority::Reliable,
            )
        })
        .collect()
}

impl Rig {
    fn new(view_distance: i32, positions: &[[f64; 3]]) -> Self {
        let side = |delta: bool| Side {
            cache: {
                let mut cache = ChunkPacketCache::new();
                if !delta {
                    cache.deltas = ChunkDeltaLog::disabled();
                }
                cache
            },
            viewers: positions
                .iter()
                .enumerate()
                .map(|(i, &position)| viewer_at(i as u64 + 1, position))
                .collect(),
            elapsed: std::time::Duration::ZERO,
        };
        Self {
            registries: Registries::new(),
            delta: side(true),
            full: side(false),
            view_distance,
        }
    }

    fn each(&mut self, mut f: impl FnMut(usize, &mut ViewerState)) {
        for side in [&mut self.delta, &mut self.full] {
            for (i, viewer) in side.viewers.iter_mut().enumerate() {
                f(i, viewer);
            }
        }
    }

    /// Publish `output` to both sides; the wire and the end state must agree.
    fn publish(&mut self, output: &TickOutput, inputs: &Inputs) -> Wire {
        let mut wires = Vec::new();
        for side in [&mut self.delta, &mut self.full] {
            for (i, viewer) in side.viewers.iter_mut().enumerate() {
                viewer.world_credit = inputs.credit[i];
                viewer.skipped |= inputs.skipped[i];
            }
            let included = &inputs.included;
            let started = std::time::Instant::now();
            let packets = publish_with_registries(
                output,
                side.viewers
                    .iter_mut()
                    .enumerate()
                    .filter(|(i, _)| included[*i])
                    .map(|(_, viewer)| viewer),
                self.view_distance,
                &self.registries.view(),
                &mut side.cache,
            );
            side.elapsed += started.elapsed();
            wires.push(wire(packets));
        }
        let full = wires.pop().unwrap();
        let delta = wires.pop().unwrap();
        assert_eq!(
            delta.len(),
            full.len(),
            "tick {}: packet count differs",
            output.tick
        );
        for (n, (a, b)) in delta.iter().zip(&full).enumerate() {
            assert_eq!(a, b, "tick {}: packet {n} differs", output.tick);
        }
        for (a, b) in self.delta.viewers.iter().zip(&self.full.viewers) {
            let sorted = |viewer: &ViewerState| {
                let mut held: Vec<_> = viewer.loaded_chunks.iter().map(|(p, r)| (*p, *r)).collect();
                held.sort_unstable();
                held
            };
            assert_eq!(
                sorted(a),
                sorted(b),
                "viewer {} held chunks",
                a.connection.0
            );
            assert_eq!(
                a.chunk_repairs, b.chunk_repairs,
                "viewer {} repairs",
                a.connection.0
            );
            assert_eq!(
                a.pending_blocks, b.pending_blocks,
                "viewer {} owed",
                a.connection.0
            );
            assert_eq!(a.skipped, b.skipped);
        }
        delta
    }

    fn stats(side: &Side) -> ChunkPlanStats {
        let mut total = ChunkPlanStats::default();
        for viewer in &side.viewers {
            total.add(viewer.chunk_stats);
        }
        total
    }

    fn reset_stats(&mut self) {
        self.each(|_, viewer| viewer.chunk_stats = ChunkPlanStats::default());
        self.delta.elapsed = std::time::Duration::ZERO;
        self.full.elapsed = std::time::Duration::ZERO;
    }

    /// Publish until no viewer receives a chunk any more.
    fn settle(&mut self, world: &mut World) {
        let inputs = Inputs::all(self.delta.viewers.len());
        for _ in 0..64 {
            let output = world.publish();
            let packets = self.publish(&output, &inputs);
            if packets.is_empty() {
                return;
            }
        }
        panic!("viewers never settled");
    }
}

fn chunk_packets(wire: &Wire) -> usize {
    wire.iter()
        .filter(|(_, data, _)| {
            let mut framed = bytes::BytesMut::from(&data[..]);
            mc_protocol::frame::try_decode_frame(
                &mut framed,
                mc_protocol::frame::Compression::Threshold(256),
            )
            .ok()
            .flatten()
            .is_some_and(|frame| frame.id == mc_protocol::packets::play::LevelChunkWithLight::ID)
        })
        .count()
}

/// `count` x `count` stationary viewers one chunk apart: heavily overlapping windows.
fn grid(count: i32) -> Vec<[f64; 3]> {
    (0..count)
        .flat_map(|x| (0..count).map(move |z| [f64::from(x * 16 + 8), 64.0, f64::from(z * 16 + 8)]))
        .collect()
}

fn sample(label: &str, rig: &Rig, edits: usize, ticks: usize) -> (ChunkPlanStats, ChunkPlanStats) {
    let (delta, full) = (Rig::stats(&rig.delta), Rig::stats(&rig.full));
    let viewers = rig.delta.viewers.len() as f64;
    let per = |value: u64| value as f64 / viewers / ticks as f64;
    println!(
        "[delta-fanout] {label}: edited chunks/tick={edits} | full-walk probes/viewer/plan={:.1} visited={:.1} | delta probes/viewer/plan={:.1} visited={:.1} | delta_plans={} full_walks={} | exploratory debug publish ms/tick: full-walk={:.2} delta={:.2}",
        per(full.probes),
        per(full.visited),
        per(delta.probes),
        per(delta.visited),
        delta.delta_plans,
        delta.full_walks,
        rig.full.elapsed.as_secs_f64() * 1e3 / ticks as f64,
        rig.delta.elapsed.as_secs_f64() * 1e3 / ticks as f64,
    );
    (delta, full)
}

/// Deterministic xorshift so the scenario is reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound
    }
}

#[test]
fn delta_lists_loads_revisions_and_unloads_in_order() {
    let mut world = World::square(0, 3);
    let mut log = ChunkDeltaLog::default();
    log.begin();
    let first = log.delta(&world.publish()).unwrap();
    assert!(first.changes().is_empty(), "no base yet: nothing to follow");
    log.begin();
    assert!(log.delta(&world.publish()).unwrap().changes().is_empty());
    world.place(1, 1, 1);
    world.unload(2, 2);
    world.load(5, 0);
    world.touch(0, 0);
    log.begin();
    let delta = log.delta(&world.publish()).unwrap();
    let listed: Vec<_> = delta
        .changes()
        .iter()
        .map(|change| (change.pos, change.kind))
        .collect();
    assert_eq!(
        listed,
        [
            ((0, 0), ChunkChangeKind::Revised),
            ((1, 1), ChunkChangeKind::Revised),
            ((2, 2), ChunkChangeKind::Unloaded),
            ((5, 0), ChunkChangeKind::Loaded),
        ]
    );
    let revisions: Vec<_> = delta
        .changes()
        .iter()
        .map(|change| change.revision)
        .collect();
    assert_eq!(revisions[2], 0);
    assert!(revisions[0] != 0 && revisions[1] != 0 && revisions[3] != 0);
}

#[test]
fn skipped_publish_has_no_base_and_overflow_lists_nothing() {
    let mut world = World::square(0, 20);
    let mut log = ChunkDeltaLog::default();
    log.begin();
    let first = log.delta(&world.publish()).unwrap();
    let synced = first.sync(Dimension::Overworld, 0, true);
    log.begin();
    world.place(3, 3, 1);
    let next = log.delta(&world.publish()).unwrap();
    assert!(next.follows(&synced, Dimension::Overworld));
    assert!(!next.follows(&synced, Dimension::Nether), "other dimension");
    let mut foreign = ChunkDeltaLog::default();
    foreign.begin();
    let foreign = foreign
        .delta(&world.publish())
        .unwrap()
        .sync(Dimension::Overworld, 0, true);
    assert!(!next.follows(&foreign, Dimension::Overworld), "other cache");
    // A publish this log never saw: the next delta cannot name its base.
    log.begin();
    log.begin();
    let gap = log.delta(&world.publish()).unwrap();
    assert!(!gap.follows(
        &next.sync(Dimension::Overworld, 0, true),
        Dimension::Overworld
    ));
    for x in 0..20 {
        for z in 0..20 {
            world.touch(x, z);
        }
    }
    log.begin();
    let overflow = log.delta(&world.publish()).unwrap();
    assert!(overflow.changes().is_empty(), "overflow drops the list");
    assert!(!overflow.follows(
        &gap.sync(Dimension::Overworld, 0, true),
        Dimension::Overworld
    ));
}

#[test]
fn idle_viewers_probe_nothing_and_edits_cost_in_proportion_to_changes() {
    let (viewers, vd) = (10, 8);
    let mut world = World::square(-vd - 1, viewers + vd + 1);
    let mut rig = Rig::new(vd, &grid(viewers));
    rig.settle(&mut world);
    let inputs = Inputs::all(rig.delta.viewers.len());
    let resident = world.chunks.len();

    // Idle: nothing changes.
    rig.reset_stats();
    let ticks = 10;
    for _ in 0..ticks {
        let output = world.publish();
        assert!(
            rig.publish(&output, &inputs).iter().all(|packet| packet.2),
            "idle sends no world packet"
        );
    }
    let (delta, full) = sample("idle", &rig, 0, ticks);
    assert_eq!(
        (delta.probes, delta.visited),
        (0, 0),
        "idle viewers do not probe"
    );
    assert_eq!(delta.delta_plans, delta.plans);
    assert!(full.probes > 1000 * ticks as u64);

    // 1% of the resident chunks edited per tick.
    let edits = resident / 100;
    rig.reset_stats();
    let mut changed = 0;
    for tick in 0..ticks {
        for n in 0..edits {
            let (x, z) = nth_chunk(&world, (tick * edits + n) * 7 + tick);
            world.place(x, z, 1 + (tick % 2) as u32);
        }
        let output = world.publish();
        let wire = rig.publish(&output, &inputs);
        changed += chunk_packets(&wire);
    }
    assert_eq!(changed, 0, "block edits are explained by block updates");
    let (delta, full) = sample("1% block edits", &rig, edits, ticks);
    let viewers = rig.delta.viewers.len() as u64;
    // Probes follow the edited chunks, not the window: a full walk probes
    // every held chunk several times per plan.
    assert_eq!(delta.delta_plans, delta.plans);
    assert!(
        delta.probes * 10 < full.probes,
        "delta {} vs full-walk {} probes",
        delta.probes,
        full.probes
    );
    assert!(delta.probes <= viewers * ticks as u64 * edits as u64 * 14);
    let one_percent = delta;

    // Three times the edits cost about three times the probes.
    let edits = edits * 3;
    rig.reset_stats();
    for tick in 0..ticks {
        for n in 0..edits {
            let (x, z) = nth_chunk(&world, (tick * edits + n) * 5 + tick);
            world.place(x, z, 1 + (tick % 2) as u32);
        }
        let output = world.publish();
        rig.publish(&output, &inputs);
    }
    let (delta, _) = sample("3% block edits", &rig, edits, ticks);
    assert!(
        delta.probes > one_percent.probes * 2 && delta.probes < one_percent.probes * 4,
        "3% probes {} vs 1% probes {}",
        delta.probes,
        one_percent.probes
    );

    // One viewer in ten steps a chunk east/west every tick: those walk
    // their new window in full, the other ninety stay at the edit cost.
    rig.reset_stats();
    for tick in 0..ticks {
        let step = if tick % 2 == 0 { 16.0 } else { -16.0 };
        rig.each(|i, viewer| {
            if i % 10 == 0 {
                viewer.position[0] += step;
            }
        });
        let output = world.publish();
        rig.publish(&output, &inputs);
    }
    let subset = |side: &Side, movers: bool| {
        let mut total = ChunkPlanStats::default();
        for (i, viewer) in side.viewers.iter().enumerate() {
            if (i % 10 == 0) == movers {
                total.add(viewer.chunk_stats);
            }
        }
        total
    };
    for (label, movers, count) in [("moving", true, 10.0), ("stationary", false, 90.0)] {
        let (delta, full) = (subset(&rig.delta, movers), subset(&rig.full, movers));
        let per = |value: u64| value as f64 / count / ticks as f64;
        println!(
            "[delta-fanout] {label} viewers (no edits): full-walk probes/viewer/plan={:.1} | delta probes/viewer/plan={:.1} delta_plans={} full_walks={}",
            per(full.probes),
            per(delta.probes),
            delta.delta_plans,
            delta.full_walks
        );
        if movers {
            assert_eq!(
                delta.full_walks, delta.plans,
                "a moved window is walked in full"
            );
        } else {
            assert_eq!((delta.probes, delta.visited), (0, 0));
        }
    }
}

/// The `n`th resident chunk in position order.
fn nth_chunk(world: &World, n: usize) -> (i32, i32) {
    *world.chunks.keys().nth(n % world.chunks.len()).unwrap()
}

#[test]
fn moving_viewer_walks_its_new_window_then_goes_quiet_again() {
    let mut world = World::square(-6, 14);
    let mut rig = Rig::new(3, &grid(4));
    rig.settle(&mut world);
    rig.reset_stats();
    let inputs = Inputs::all(16);
    rig.each(|i, viewer| {
        if i == 0 {
            viewer.position[0] += 16.0;
        }
    });
    for _ in 0..8 {
        let output = world.publish();
        rig.publish(&output, &inputs);
    }
    let moved = rig.delta.viewers[0].chunk_stats;
    let still = rig.delta.viewers[1].chunk_stats;
    assert!(
        moved.full_walks >= 1,
        "a moved window is reconciled in full"
    );
    assert!(moved.delta_plans >= 1, "then follows the delta again");
    assert_eq!((still.probes, still.visited), (0, 0));
}

#[test]
fn light_repair_unload_reload_and_credit_debt_match_the_full_walk() {
    let mut world = World::square(-5, 10);
    let mut rig = Rig::new(3, &grid(4));
    rig.settle(&mut world);
    let all = Inputs::all(16);

    // Light-only revision: a LightUpdate, never a chunk resend.
    world.relight(1, 1, 0x33);
    let output = world.publish();
    let wire = rig.publish(&output, &all);
    assert!(!wire.is_empty());
    assert_eq!(chunk_packets(&wire), 0);

    // Revision without any explanation: full chunk resend (repair).
    world.touch(2, 2);
    let output = world.publish();
    assert!(chunk_packets(&rig.publish(&output, &all)) > 0);

    // Unload then reload with a new revision.
    world.unload(1, 2);
    let output = world.publish();
    rig.publish(&output, &all);
    world.load(1, 2);
    let output = world.publish();
    assert!(chunk_packets(&rig.publish(&output, &all)) > 0);

    // Light debt alone: no credit when the light moves, credit later with
    // nothing else changing. The owed update is still sent.
    world.relight(2, 1, 0x44);
    let output = world.publish();
    let mut dry = all.clone();
    dry.credit = vec![0; 16];
    rig.publish(&output, &dry);
    let output = world.publish();
    let wire = rig.publish(&output, &all);
    assert!(
        wire.iter().any(|packet| !packet.2),
        "the owed light update is sent once credit returns"
    );

    // No credit while a block, a light edit and a repair land: debts stay
    // owed and are paid in the same order once credit returns.
    let mut starved = all.clone();
    starved.credit = vec![0; 16];
    world.place(0, 0, 1);
    world.relight(1, 0, 0x22);
    world.touch(3, 3);
    let output = world.publish();
    rig.publish(&output, &starved);
    let output = world.publish();
    rig.publish(&output, &starved);
    let mut trickle = all.clone();
    trickle.credit = vec![1; 16];
    for _ in 0..12 {
        let output = world.publish();
        rig.publish(&output, &trickle);
    }
    rig.settle(&mut world);
    let output = world.publish();
    assert!(rig.publish(&output, &all).is_empty());
}

#[test]
fn skipped_missing_and_overflowing_publishes_reconcile_in_full() {
    let mut world = World::square(-5, 22);
    let mut rig = Rig::new(3, &grid(3));
    rig.settle(&mut world);
    let all = Inputs::all(9);

    // The pump skipped outputs for viewer 0: its block changes never arrived.
    world.place(1, 1, 1);
    let output = world.publish();
    let mut skipped = all.clone();
    skipped.skipped[0] = true;
    rig.publish(&output, &skipped);

    // Viewer 1 is absent from a publish and returns one later.
    world.place(2, 2, 1);
    let output = world.publish();
    let mut absent = all.clone();
    absent.included[1] = false;
    rig.publish(&output, &absent);
    world.place(0, 2, 1);
    let output = world.publish();
    rig.publish(&output, &all);

    // More changed chunks than the delta lists.
    for x in -5..22 {
        for z in -5..22 {
            world.place(x, z, 1);
        }
    }
    let output = world.publish();
    rig.publish(&output, &all);
    let output = world.publish();
    rig.publish(&output, &all);
}

#[test]
fn unsent_chunk_and_respawn_force_a_full_walk() {
    let mut world = World::square(-5, 10);
    let mut rig = Rig::new(3, &grid(2));
    rig.settle(&mut world);
    let all = Inputs::all(4);
    rig.each(|i, viewer| {
        if i == 0 {
            viewer.chunk_unsent((0, 0));
        }
    });
    let output = world.publish();
    let wire = rig.publish(&output, &all);
    assert_eq!(chunk_packets(&wire), 1, "the unsent chunk is planned again");
    rig.each(|i, viewer| {
        if i == 1 {
            viewer.forget_all = true;
        }
    });
    let output = world.publish();
    rig.publish(&output, &all);
    rig.settle(&mut world);
}

#[test]
fn random_world_matches_the_full_walk_every_tick() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut world = World::square(-6, 12);
    let positions = grid(4);
    let mut rig = Rig::new(3, &positions);
    rig.settle(&mut world);
    let mut unloaded = Vec::new();
    for _ in 0..80 {
        for _ in 0..rng.next(4) {
            let (x, z) = nth_chunk(&world, rng.next(1000) as usize);
            world.place(x, z, 1 + rng.next(3) as u32);
        }
        if rng.next(4) == 0 {
            let (x, z) = nth_chunk(&world, rng.next(1000) as usize);
            world.relight(x, z, rng.next(15) as u8 + 1);
        }
        if rng.next(8) == 0 {
            let (x, z) = nth_chunk(&world, rng.next(1000) as usize);
            world.touch(x, z);
        }
        if rng.next(8) == 0 {
            let (x, z) = nth_chunk(&world, rng.next(1000) as usize);
            world.unload(x, z);
            unloaded.push((x, z));
        }
        if rng.next(2) == 0
            && let Some((x, z)) = unloaded.pop()
            && !world.resident(x, z)
        {
            world.load(x, z);
        }
        let mut inputs = Inputs::all(positions.len());
        for i in 0..positions.len() {
            inputs.credit[i] = match rng.next(20) {
                0..=2 => 0,
                3..=5 => 1 + rng.next(6) as usize,
                _ => usize::MAX,
            };
            inputs.skipped[i] = rng.next(40) == 0;
            inputs.included[i] = rng.next(30) != 0;
        }
        match rng.next(30) {
            0 => rig.each(|i, viewer| {
                if i == 3 {
                    viewer.position[0] += 16.0;
                }
            }),
            1 => rig.each(|i, viewer| {
                if i == 5 {
                    viewer.position[2] -= 16.0;
                }
            }),
            _ => {}
        }
        let output = world.publish();
        rig.publish(&output, &inputs);
    }
    let delta = Rig::stats(&rig.delta);
    let full = Rig::stats(&rig.full);
    assert!(
        delta.delta_plans > 0,
        "the scenario must exercise the delta path"
    );
    assert!(delta.full_walks > 0, "and the fallback");
    assert!(delta.probes < full.probes);
}
