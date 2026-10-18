//! chunks.rs unit tests: chunk packet assembly from snapshot chunks.

use super::*;
use mc_data::items::{ItemRegistry, ItemReport};
use mc_protocol::packets::Packet;
use mc_world::chunk::{Chunk, ChunkPos};

fn items() -> ItemRegistry {
    ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:stone").expect("static id parses"),
        protocol_id: 1,
    }])
}

fn reports() -> (Registry, mc_world::BlockRegistry) {
    let biomes = test_biome_registry(&["minecraft:plains"]);
    let blocks =
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("required blocks build");
    (biomes, blocks)
}

fn be_types() -> mc_data::block_entity_types::BlockEntityTypeRegistry {
    mc_data::block_entity_types::solaris_required_block_entity_types()
}

#[test]
fn encodes_air_chunk_packet() {
    let (biomes, blocks) = reports();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        Identifier::parse("minecraft:plains").expect("static id parses"),
    );
    chunk.section_lights.fill(mc_world::chunk::SectionLight {
        block: Some(mc_world::chunk::LightSection::uniform(0)),
        sky: Some(mc_world::chunk::LightSection::uniform(0xFF)),
    });
    let packet = encode_chunk_packet(
        ChunkPos { x: 0, z: 0 },
        &chunk,
        EncodeInputs {
            biomes: &biomes,
            blocks: &blocks,
            items: &items(),
            block_entity_types: &be_types(),
        },
    )
    .expect("air chunk encodes");
    assert_eq!(packet.chunk_x, 0);
    assert_eq!(packet.chunk_z, 0);
    assert!(!packet.data.is_empty());
    // Fresh empty chunks carry no named heightmaps; the packet is still
    // valid with an empty heightmap list (client uses defaults).
    let _ = packet.heightmaps;
}

#[test]
fn unknown_biome_fails_closed() {
    let mut biomes = test_biome_registry(&["minecraft:plains"]);
    biomes.entries.clear();
    let (_, blocks) = reports();
    let chunk = Chunk::empty(
        ChunkPos { x: 1, z: 2 },
        mc_world::BlockStateId(0),
        Identifier::parse("minecraft:plains").expect("static id parses"),
    );
    assert!(
        encode_chunk_packet(
            ChunkPos { x: 1, z: 2 },
            &chunk,
            EncodeInputs {
                biomes: &biomes,
                blocks: &blocks,
                items: &items(),
                block_entity_types: &be_types(),
            },
        )
        .is_err()
    );
}

#[test]
fn biome_registry_probe_orders_names() {
    let reg = test_biome_registry(&["minecraft:plains", "minecraft:desert"]);
    assert_eq!(reg.entries.len(), 2);
    assert_eq!(reg.entries[1].as_str(), "minecraft:desert");
}

// --- G5 radius split: sim 6->2->6 must not move edge wire -------------------
//
// Residency (`World::has_chunk`, core `desired_chunks`) is the union of the
// client-view square (radius `config_view_distance`, +1 margin), the active
// simulation set, tickets and dirty pins. Shrinking only the simulation
// radius must not evict or re-send anything the stationary viewer holds:
// 0 `ForgetLevelChunk`, 0 fresh `LevelChunkWithLight`, 0 repairs
// (full-chunk resends) after the initial stable view, with the resident
// *positions* identical across the whole 6->2->6 swing. The snapshots below
// are the real core residency lists (`world.snapshot()` after
// `desired_chunks` + blocking admission), not hand-built chunk sets.
// (Per-tick activation/inhabited bookkeeping is storage/journal metadata by
// design — never the wire revision the edge plans against — so the test
// pins positions, while the zero-chunk-packet asserts prove no wire-visible
// content moved under the held view.)

use mc_core::boundary::{ConnectionId, EntityId, PlayerInput};
use mc_core::config::CoreConfig;
use mc_core::tick::{TickState, run_tick};
use mc_core::world::ChunkWorld;
use mc_world::{BlockStateId, ChunkGenerator};

struct FlatStone {
    air: BlockStateId,
    stone: BlockStateId,
}

impl ChunkGenerator for FlatStone {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(
            pos,
            self.air,
            Identifier::parse("minecraft:plains").expect("static id parses"),
        );
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 64, z, self.stone);
            }
        }
        chunk
    }
}

fn radius_state(view: i32, sim: i32) -> (TickState, EntityId) {
    let directory = std::env::temp_dir().join(format!("g5-radius-edge-{}", uuid::Uuid::new_v4()));
    let mut config = CoreConfig::new(directory.clone(), 42);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config.view_distance = 0;
    config.simulation_distance = 0;
    config.checkpoint_every_ticks = u64::MAX;
    let core = mc_core::Core::open(config).expect("core opens for registry");
    let registry = std::sync::Arc::clone(core.registry());
    core.shutdown();
    drop(core);
    std::fs::remove_dir_all(&directory).ok();
    let generator: std::sync::Arc<dyn ChunkGenerator> = std::sync::Arc::new(FlatStone {
        air: BlockStateId(registry.states.air),
        stone: BlockStateId(registry.states.stone),
    });
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        BlockStateId(registry.states.air),
        [8, 65, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, sim, 4096);
    state.config_view_distance = view;
    state.config_sim_distance = sim;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::from_u128(0x6_2_6),
            name: "stationary-viewer".into(),
            permission_level: 0,
        }],
    );
    let actor = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("viewer joined");
    let slot = state
        .entities
        .slots()
        .find(|&slot| state.entities.body(slot).id == actor)
        .expect("viewer row");
    state.entities.set_pos(slot, [8.0, 65.0, 8.0]);
    // Settle the join (teleport confirm + residency), then hold still.
    run_tick(
        &mut state,
        &[PlayerInput::ConfirmTeleport {
            connection: ConnectionId(1),
            teleport_id: 1,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    (state, actor)
}

fn settle_viewer(
    output: &mc_core::boundary::TickOutput,
    actor: EntityId,
) -> crate::plan::ViewerState {
    let mut viewer = crate::plan::ViewerState::new(ConnectionId(1), actor, [8.0, 65.0, 8.0]);
    // `plan_viewer` consumes `world_credit` by value each call; restore it so
    // the paced admission (16 chunks/plan) drains instead of stalling.
    let cache = crate::plan::ChunkPacketCache::new();
    // Drain paced admission until the full view square is held; resident
    // margin chunks stay server-side and are not part of the client view.
    for _ in 0..64 {
        viewer.world_credit = 65_536;
        let planned = crate::plan::plan_viewer(output, &mut viewer, 8, &cache);
        if planned
            .iter()
            .all(|packet| packet.id != mc_protocol::packets::play::LevelChunkWithLight::ID)
        {
            break;
        }
    }
    assert_eq!(
        viewer.loaded_chunks.len(),
        17 * 17,
        "stable view holds every view-square chunk, not the resident margin"
    );
    viewer
}

fn wire_counts(packets: &[crate::plan::PlannedPacket]) -> (usize, usize, usize) {
    use mc_protocol::packets::play::{ForgetLevelChunk, LevelChunkWithLight};
    let forgets = packets
        .iter()
        .filter(|packet| packet.id == ForgetLevelChunk::ID)
        .count();
    let chunks = packets
        .iter()
        .filter(|packet| packet.id == LevelChunkWithLight::ID)
        .count();
    // Repairs ride the same chunk packet as admissions; after a stable view
    // every chunk packet is a repair (the viewer already holds them all).
    (forgets, chunks, chunks)
}

/// Stationary viewer, sim 6->2->6: residency still covers the full view
/// square (core `desired_chunks` is view-driven, not sim-driven) and the
/// edge plans zero forgets, zero chunk packets and zero repairs on every
/// swing. Residency is pinned by *position* (activation bookkeeping is
/// storage metadata by design and never bumps the wire revision the edge
/// plans against).
#[test]
fn stationary_viewer_sim_swing_sends_no_wire() {
    let (mut state, actor) = radius_state(8, 6);
    let baseline = run_tick(&mut state, &[]).0;
    let mut resident: Vec<(i32, i32)> = baseline
        .snapshot
        .chunks
        .iter()
        .map(|chunk| (chunk.pos.x, chunk.pos.z))
        .collect();
    resident.sort_unstable();
    // The view square (radius 8 + margin 1) is resident even though sim is 6.
    assert!(
        resident.len() >= 19 * 19,
        "view square resident: {}",
        resident.len()
    );
    let mut viewer = settle_viewer(&baseline, actor);
    let cache = crate::plan::ChunkPacketCache::new();
    // Steady state first: no wire without any change at all.
    viewer.world_credit = 65_536;
    let (forgets, chunks, _) =
        wire_counts(&crate::plan::plan_viewer(&baseline, &mut viewer, 8, &cache));
    assert_eq!((forgets, chunks), (0, 0), "stable view is wire-quiet");
    for sim in [2, 6] {
        state.config_sim_distance = sim;
        let output = run_tick(&mut state, &[]).0;
        // Residency is unchanged: the same positions held throughout.
        let mut now: Vec<(i32, i32)> = output
            .snapshot
            .chunks
            .iter()
            .map(|chunk| (chunk.pos.x, chunk.pos.z))
            .collect();
        now.sort_unstable();
        assert_eq!(now, resident, "sim {sim} must not move residency");
        viewer.world_credit = 65_536;
        let planned = crate::plan::plan_viewer(&output, &mut viewer, 8, &cache);
        let (forgets, chunks, repairs) = wire_counts(&planned);
        assert_eq!(forgets, 0, "sim {sim}: 0 ForgetLevelChunk");
        assert_eq!(chunks, 0, "sim {sim}: 0 LevelChunkWithLight");
        assert_eq!(repairs, 0, "sim {sim}: 0 repairs");
    }
}
