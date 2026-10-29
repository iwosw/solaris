//! Real packet codecs and credit-aware viewer lifecycle regressions.

use super::*;
use std::sync::{Arc, LazyLock};

use mc_core::boundary::{
    BlockChange, ChangeSet, EntityId, PlayerChange, SnapshotChunk, TickOutput, WorldSnapshot,
};
use mc_protocol::frame::{Compression, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{BlockUpdate, ForgetLevelChunk, LevelChunkWithLight};
use mc_world::chunk::{BlockPos, ChunkPos, LightSection};
use mc_world::light::{ChunkLight, compute_chunk_light};

use crate::plan::{
    ChunkPacketCache, PlanRegistries, ViewerState, fill_chunk_packets, plan_player_changes,
    plan_viewer,
};

struct Fixtures {
    blocks: mc_world::BlockRegistry,
    biomes: mc_data::Registry,
    items: mc_data::items::ItemRegistry,
    block_entities: mc_data::block_entity_types::BlockEntityTypeRegistry,
    torch: u32,
    dark: [Arc<Chunk>; 2],
    lit: [Arc<Chunk>; 2],
}

static FIXTURES: LazyLock<Fixtures> = LazyLock::new(|| {
    let blocks =
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap();
    let torch = blocks
        .block(&mc_data::Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default;
    let table = mc_data::block_light::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/vanilla/reports/block_light.json"
    ))
    .unwrap();
    let empty = |x| {
        Chunk::empty(
            ChunkPos { x, z: 0 },
            mc_world::BlockStateId(0),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    };
    let bake = |mut chunks: [Chunk; 2]| {
        let lights: [ChunkLight; 2] = std::array::from_fn(|index| {
            let mut neighbours = [[None; 3]; 3];
            neighbours[1][1] = Some(&chunks[index]);
            neighbours[1][if index == 0 { 2 } else { 0 }] = Some(&chunks[1 - index]);
            compute_chunk_light(neighbours, &table)
        });
        for (chunk, light) in chunks.iter_mut().zip(&lights) {
            chunk.set_baked_light(light);
        }
        chunks.map(Arc::new)
    };
    let dark = bake([empty(0), empty(1)]);
    let mut source = empty(0);
    source.set_block(15, 64, 8, torch);
    let lit = bake([source, empty(1)]);
    Fixtures {
        blocks,
        biomes: crate::chunks::test_biome_registry(&["minecraft:plains"]),
        items: mc_data::items::solaris_required_items(),
        block_entities: mc_data::block_entity_types::solaris_required_block_entity_types(),
        torch: torch.0,
        dark,
        lit,
    }
});

fn registries() -> PlanRegistries<'static> {
    PlanRegistries {
        biomes: &FIXTURES.biomes,
        blocks: &FIXTURES.blocks,
        items: &FIXTURES.items,
        block_entity_types: &FIXTURES.block_entities,
    }
}

fn output(chunks: &[Arc<Chunk>; 2], revision: u64, block: Option<u32>) -> TickOutput {
    let mut changes = ChangeSet::default();
    if let Some(state) = block {
        changes.blocks.push(BlockChange {
            position: BlockPos { x: 15, y: 64, z: 8 },
            state,
        });
    }
    TickOutput {
        tick: revision,
        snapshot: Arc::new(WorldSnapshot {
            tick: revision,
            chunks: Arc::new(
                chunks
                    .iter()
                    .enumerate()
                    .map(|(index, chunk)| SnapshotChunk {
                        pos: chunk.pos,
                        revision: revision * 2 + index as u64,
                        chunk: Arc::clone(chunk),
                    })
                    .collect(),
            ),
            ..Default::default()
        }),
        changes: Arc::new(changes),
        times: Default::default(),
    }
}

fn quiet(output: &TickOutput) -> TickOutput {
    TickOutput {
        changes: Arc::new(ChangeSet::default()),
        ..output.clone()
    }
}

fn viewer() -> ViewerState {
    ViewerState::new(ConnectionId(7), EntityId(7), [8.0, 64.0, 8.0])
}

fn body(packet: &PlannedPacket) -> bytes::Bytes {
    let mut bytes = bytes::BytesMut::from(packet.data.as_ref());
    let frame = try_decode_frame(&mut bytes, Compression::Threshold(256))
        .unwrap()
        .unwrap();
    assert_eq!(frame.id, packet.id);
    assert!(bytes.is_empty());
    frame.body
}

fn updates(packets: &[PlannedPacket]) -> Vec<LightUpdate> {
    packets
        .iter()
        .filter(|packet| packet.id == LightUpdate::ID)
        .map(|packet| {
            assert!(!packet.reliable);
            assert!(packet.chunk.is_none());
            let mut bytes = body(packet);
            let update = LightUpdate::decode(&mut bytes).unwrap();
            assert!(bytes.is_empty());
            update
        })
        .collect()
}

fn load(output: &TickOutput, viewer: &mut ViewerState, cache: &mut ChunkPacketCache) {
    let packets = plan_viewer(output, viewer, 1, cache);
    let mut plans = [(viewer.connection, packets)];
    assert!(fill_chunk_packets(output, &mut plans, &registries(), cache).is_empty());
    let chunks: Vec<_> = plans[0]
        .1
        .iter()
        .filter(|packet| packet.id == LevelChunkWithLight::ID)
        .map(|packet| {
            let mut bytes = body(packet);
            let decoded = LevelChunkWithLight::decode(&mut bytes).unwrap();
            assert!(bytes.is_empty());
            decoded
        })
        .collect();
    assert_eq!(chunks.len(), 2);
    for chunk in chunks {
        assert_eq!(
            chunk.light,
            wire_light(&output.snapshot.chunks[chunk.chunk_x as usize].chunk)
        );
    }
}

/// `chunk_light` as a 26.3 client decodes it: `BitSet.valueOf` drops
/// trailing zero words, so an all-zero mask arrives as an empty one.
fn wire_light(chunk: &Chunk) -> LightData {
    let mut light = chunk_light(chunk);
    for mask in [
        &mut light.sky_y_mask,
        &mut light.block_y_mask,
        &mut light.empty_sky_y_mask,
        &mut light.empty_block_y_mask,
    ] {
        while mask.last() == Some(&0) {
            mask.pop();
        }
    }
    light
}

fn block_nibble(light: &LightData, x: usize, y: i32, z: usize) -> u8 {
    let local_y = (y + 64) as usize;
    let slot = local_y / 16 + 1;
    let mask = light.block_y_mask[0] as u64;
    if mask & (1 << slot) == 0 {
        assert_ne!(light.empty_block_y_mask[0] as u64 & (1 << slot), 0);
        return 0;
    }
    let array = (mask & ((1 << slot) - 1)).count_ones() as usize;
    let index = (local_y % 16) * 256 + z * 16 + x;
    (light.block_updates[array][index / 2] >> ((index % 2) * 4)) & 15
}

#[test]
fn light_wire_masks_nibbles_and_varint_coordinates_round_trip() {
    let light = chunk_light(&FIXTURES.lit[0]);
    assert_eq!(light.sky_y_mask, vec![(1_i64 << 26) - 2]);
    assert_eq!(light.empty_sky_y_mask, vec![1]);
    assert_eq!(light.sky_updates.len(), 25);
    assert!(light.sky_updates.iter().all(|array| array.len() == 2048));
    assert_eq!(light.sky_updates.last().unwrap(), &vec![0xFF; 2048]);
    assert_eq!(light.block_y_mask[0] & light.empty_block_y_mask[0], 0);
    assert_eq!(
        light.block_y_mask[0] | light.empty_block_y_mask[0],
        (1_i64 << 26) - 1
    );
    assert_eq!(block_nibble(&light, 15, 64, 8), 14);
    assert_eq!(block_nibble(&chunk_light(&FIXTURES.lit[1]), 0, 64, 8), 13);
    let packet = LightUpdate {
        chunk_x: -1,
        chunk_z: 128,
        light,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(&bytes[..7], &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0x80, 0x01]);
    let mut input = bytes.as_slice();
    assert_eq!(LightUpdate::decode(&mut input).unwrap(), packet);
    assert!(input.is_empty());
}

#[test]
fn viewer_light_baseline_shares_nibble_storage_until_snapshot_mutation() {
    let mut baseline = LightBaseline::default();
    let source = &FIXTURES.lit[0];
    baseline.record((0, 0), source, 1);
    let allocation = source.section_lights[8]
        .block
        .as_ref()
        .unwrap()
        .shared_allocation();
    assert!(allocation.is_some());
    let held = &baseline.sent[&(0, 0)];
    assert_eq!(
        held.sent[8].block.as_ref().unwrap().shared_allocation(),
        allocation
    );
    assert_eq!(
        held.observed[8].block.as_ref().unwrap().shared_allocation(),
        allocation
    );
    let mut next = (**source).clone();
    next.section_lights[8].block.as_mut().unwrap().set(0, 1);
    assert_ne!(
        next.section_lights[8]
            .block
            .as_ref()
            .unwrap()
            .shared_allocation(),
        allocation
    );
    assert!(baseline.observe((0, 0), &next));
    let mut packets = Vec::new();
    baseline.update((0, 0), &next, 2, ConnectionId(7), &mut packets, &mut 0);
    assert!(packets.is_empty());
    assert_eq!(
        baseline.sent[&(0, 0)].sent[8]
            .block
            .as_ref()
            .unwrap()
            .shared_allocation(),
        allocation
    );
    baseline.update((0, 0), &next, 2, ConnectionId(7), &mut packets, &mut 1);
    assert_eq!(updates(&packets)[0].light, wire_light(&next));
    baseline.update((0, 0), &next, 3, ConnectionId(7), &mut packets, &mut 1);
    assert_eq!(updates(&packets).len(), 1);
}

#[test]
fn viewer_light_baseline_diffs_a_chunk_only_when_its_revision_moves() {
    let mut baseline = LightBaseline::default();
    let source = &FIXTURES.lit[0];
    baseline.record((0, 0), source, 1);
    let mut next = (**source).clone();
    next.section_lights[8].block.as_mut().unwrap().set(0, 1);
    let mut packets = Vec::new();
    // Core bumps the revision on every light change: the same revision is
    // never diffed, so this (deliberately inconsistent) input sends nothing.
    baseline.update((0, 0), &next, 1, ConnectionId(7), &mut packets, &mut 1);
    assert!(packets.is_empty());
    // A new revision with identical light is diffed once, sends nothing and
    // is then remembered as verified: a changed chunk at that revision is skipped.
    baseline.update((0, 0), source, 2, ConnectionId(7), &mut packets, &mut 1);
    assert!(packets.is_empty());
    baseline.update((0, 0), &next, 2, ConnectionId(7), &mut packets, &mut 1);
    assert!(packets.is_empty());
    // Real light change at a new revision, without credit: not lost.
    baseline.update((0, 0), &next, 3, ConnectionId(7), &mut packets, &mut 0);
    assert!(packets.is_empty());
    baseline.update((0, 0), &next, 3, ConnectionId(7), &mut packets, &mut 1);
    assert_eq!(updates(&packets).len(), 1);
    assert_eq!(updates(&packets)[0].light, wire_light(&next));
}

#[test]
fn torch_placement_and_removal_publish_blocks_and_neighbour_light_without_full_chunks() {
    let mut viewer = viewer();
    let mut cache = ChunkPacketCache::new();
    load(&output(&FIXTURES.dark, 1, None), &mut viewer, &mut cache);
    let placed = output(&FIXTURES.lit, 2, Some(FIXTURES.torch));
    let packets = plan_viewer(&placed, &mut viewer, 1, &cache);
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet.id == BlockUpdate::ID)
            .count(),
        1
    );
    assert!(
        packets
            .iter()
            .all(|packet| packet.id != LevelChunkWithLight::ID)
    );
    let block = packets
        .iter()
        .find(|packet| packet.id == BlockUpdate::ID)
        .unwrap();
    assert_eq!(
        BlockUpdate::decode(&mut body(block)).unwrap().state_id,
        FIXTURES.torch as i32
    );
    let light = updates(&packets);
    assert_eq!(light.len(), 2);
    assert_eq!(light[0].light, wire_light(&FIXTURES.lit[0]));
    assert_eq!(light[1].light, wire_light(&FIXTURES.lit[1]));
    assert!(updates(&plan_viewer(&quiet(&placed), &mut viewer, 1, &cache)).is_empty());
    let removed = output(&FIXTURES.dark, 3, Some(0));
    let packets = plan_viewer(&removed, &mut viewer, 1, &cache);
    assert!(packets.iter().any(|packet| packet.id == BlockUpdate::ID));
    assert!(
        packets
            .iter()
            .all(|packet| packet.id != LevelChunkWithLight::ID)
    );
    let light = updates(&packets);
    assert_eq!(light.len(), 2);
    for update in light {
        assert!(update.light.block_y_mask.is_empty());
        assert_eq!(update.light.empty_block_y_mask, vec![(1_i64 << 26) - 1]);
        assert!(update.light.block_updates.is_empty());
    }
}

#[test]
fn credit_debt_survives_quiet_ticks_and_uses_latest_baked_arrays() {
    let mut viewer = viewer();
    let mut cache = ChunkPacketCache::new();
    load(&output(&FIXTURES.dark, 1, None), &mut viewer, &mut cache);
    viewer.world_credit = 0;
    let placed = output(&FIXTURES.lit, 2, Some(FIXTURES.torch));
    let deferred = plan_viewer(&placed, &mut viewer, 1, &cache);
    assert!(deferred.iter().all(|packet| packet.reliable));
    assert_eq!(viewer.loaded_chunks.len(), 2);
    viewer.world_credit = 1;
    let quiet = quiet(&placed);
    let block = plan_viewer(&quiet, &mut viewer, 1, &cache);
    assert_eq!(block.iter().filter(|packet| !packet.reliable).count(), 1);
    assert_eq!(block[0].id, BlockUpdate::ID);
    let source = updates(&plan_viewer(&quiet, &mut viewer, 1, &cache));
    assert_eq!(source.len(), 1);
    assert_eq!(source[0].chunk_x, 0);
    let mut newest = (*FIXTURES.lit[1]).clone();
    newest.section_lights[8].block = Some(LightSection::uniform(0x22));
    let latest = TickOutput {
        snapshot: Arc::new(WorldSnapshot {
            chunks: Arc::new(vec![
                quiet.snapshot.chunks[0].clone(),
                SnapshotChunk {
                    pos: newest.pos,
                    revision: 9,
                    chunk: Arc::new(newest),
                },
            ]),
            ..Default::default()
        }),
        ..quiet.clone()
    };
    let neighbour = updates(&plan_viewer(&latest, &mut viewer, 1, &cache));
    assert_eq!(neighbour.len(), 1);
    assert_eq!(neighbour[0].chunk_x, 1);
    assert_eq!(
        neighbour[0].light,
        wire_light(&latest.snapshot.chunks[1].chunk)
    );
    assert!(updates(&plan_viewer(&latest, &mut viewer, 1, &cache)).is_empty());
}

#[test]
fn unsent_torch_light_reverted_to_baseline_cancels_debt_without_chunk_resend() {
    let mut viewer = viewer();
    let mut cache = ChunkPacketCache::new();
    load(&output(&FIXTURES.dark, 1, None), &mut viewer, &mut cache);
    viewer.world_credit = 0;
    plan_viewer(
        &output(&FIXTURES.lit, 2, Some(FIXTURES.torch)),
        &mut viewer,
        1,
        &cache,
    );
    viewer.world_credit = 4;
    let packets = plan_viewer(&output(&FIXTURES.dark, 3, Some(0)), &mut viewer, 1, &cache);
    assert!(packets.iter().any(|packet| packet.id == BlockUpdate::ID));
    assert!(updates(&packets).is_empty());
    assert!(
        packets
            .iter()
            .all(|packet| packet.id != LevelChunkWithLight::ID)
    );
}

#[test]
fn skipped_output_repairs_with_full_chunks_even_when_credit_arrives_later() {
    let mut viewer = viewer();
    let mut cache = ChunkPacketCache::new();
    load(&output(&FIXTURES.dark, 1, None), &mut viewer, &mut cache);
    let held = viewer.loaded_chunks.clone();
    viewer.skipped = true;
    viewer.world_credit = 0;
    let placed = output(&FIXTURES.lit, 3, Some(FIXTURES.torch));
    let packets = plan_viewer(&placed, &mut viewer, 1, &cache);
    assert!(updates(&packets).is_empty());
    assert!(!held.is_empty());
    assert_eq!(
        viewer.loaded_chunks, held,
        "zero credit retains held revisions and repair debt"
    );
    viewer.world_credit = 8;
    load(&quiet(&placed), &mut viewer, &mut cache);
    for chunk in placed.snapshot.chunks.iter() {
        assert_eq!(
            viewer.loaded_chunks[&(chunk.pos.x, chunk.pos.z)],
            chunk.revision
        );
    }
    assert!(updates(&plan_viewer(&quiet(&placed), &mut viewer, 1, &cache)).is_empty());
}

#[test]
fn unload_clears_light_debt_and_readmission_full_chunk_covers_latest_light() {
    let mut viewer = viewer();
    let mut cache = ChunkPacketCache::new();
    load(&output(&FIXTURES.dark, 1, None), &mut viewer, &mut cache);
    viewer.world_credit = 0;
    let placed = output(&FIXTURES.lit, 2, Some(FIXTURES.torch));
    plan_viewer(&placed, &mut viewer, 1, &cache);
    viewer.position[0] = 1000.0;
    viewer.world_credit = 8;
    let packets = plan_viewer(&quiet(&placed), &mut viewer, 1, &cache);
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet.id == ForgetLevelChunk::ID)
            .count(),
        2
    );
    assert!(updates(&packets).is_empty());
    viewer.position[0] = 8.0;
    load(&quiet(&placed), &mut viewer, &mut cache);
    assert!(updates(&plan_viewer(&quiet(&placed), &mut viewer, 1, &cache)).is_empty());
}

fn weather_events(packets: &[PlannedPacket]) -> Vec<(u8, f32)> {
    packets
        .iter()
        .filter(|packet| packet.id == GameEvent::ID)
        .map(|packet| {
            assert!(packet.reliable);
            let event = GameEvent::decode(&mut body(packet)).unwrap();
            (event.event, event.value)
        })
        .collect()
}

#[test]
fn weather_fades_match_threshold_state_and_duplicate_snapshot_ordering() {
    let mut viewer = viewer();
    let change = |raining, rain_level, thunder_level| PlayerChange::WeatherChanged {
        raining,
        rain_level,
        thunder_level,
    };
    plan_player_changes(&[change(false, 0.0, 0.0)], &mut viewer);
    let kind_only = plan_player_changes(&[change(true, 0.0, 0.0)], &mut viewer);
    assert!(kind_only.is_empty());
    assert_eq!(
        weather_events(&plan_player_changes(
            &[change(true, 0.19, 0.0)],
            &mut viewer
        )),
        vec![(7, 0.19)]
    );
    assert_eq!(
        weather_events(&plan_player_changes(
            &[change(true, 0.20, 0.0)],
            &mut viewer
        )),
        vec![(7, 0.20), (1, 0.0), (7, 0.20), (8, 0.0)]
    );
    assert_eq!(
        weather_events(&plan_player_changes(
            &[change(true, 0.21, 0.01)],
            &mut viewer
        )),
        vec![(7, 0.21), (8, 0.01)]
    );
    assert!(plan_player_changes(&[change(false, 0.21, 0.01)], &mut viewer).is_empty());
    assert_eq!(
        weather_events(&plan_player_changes(
            &[change(false, 0.19, 0.0)],
            &mut viewer
        )),
        vec![(7, 0.19), (8, 0.0), (2, 0.0), (7, 0.19), (8, 0.0)]
    );
}

#[test]
fn complete_weather_ramps_keep_exact_f32_levels_and_one_state_event_per_threshold() {
    let mut viewer = viewer();
    let change = |raining, rain_level, thunder_level| PlayerChange::WeatherChanged {
        raining,
        rain_level,
        thunder_level,
    };
    plan_player_changes(&[change(false, 0.0, 0.0)], &mut viewer);
    let mut rain = 0.0_f32;
    let mut starts = 0;
    for _ in 0..101 {
        let before = rain;
        rain = (rain + 0.01).min(1.0);
        let events = weather_events(&plan_player_changes(
            &[change(true, rain, 0.0)],
            &mut viewer,
        ));
        let mut expected = vec![(7, rain)];
        if before < 0.2 && rain >= 0.2 {
            expected.extend([(1, 0.0), (7, rain), (8, 0.0)]);
            starts += 1;
        }
        assert_eq!(events, expected);
    }
    assert_eq!(starts, 1);
    assert_eq!(rain, 1.0);
    let mut thunder = 0.0_f32;
    for _ in 0..101 {
        thunder = (thunder + 0.01).min(1.0);
        assert_eq!(
            weather_events(&plan_player_changes(
                &[change(true, rain, thunder)],
                &mut viewer
            )),
            vec![(8, thunder)]
        );
    }
    assert_eq!(thunder, 1.0);
    let mut stops = 0;
    for _ in 0..101 {
        let before = rain;
        let previous_thunder = thunder;
        rain = (rain - 0.01).max(0.0);
        thunder = (thunder - 0.01).max(0.0);
        let mut expected = Vec::new();
        if rain.to_bits() != before.to_bits() {
            expected.push((7, rain));
        }
        if thunder.to_bits() != previous_thunder.to_bits() {
            expected.push((8, thunder));
        }
        if before >= 0.2 && rain < 0.2 {
            expected.extend([(2, 0.0), (7, rain), (8, thunder)]);
            stops += 1;
        }
        assert_eq!(
            weather_events(&plan_player_changes(
                &[change(false, rain, thunder)],
                &mut viewer
            )),
            expected
        );
    }
    assert_eq!(stops, 1);
    assert_eq!(rain, 0.0);
    assert_eq!(thunder, 0.0);
}

#[test]
fn skipped_weather_snapshot_reconciles_reliably_without_world_credit() {
    let mut viewer = viewer();
    let cache = ChunkPacketCache::new();
    viewer.world_credit = 0;
    let mut weather = WorldSnapshot::default();
    plan_viewer(
        &TickOutput {
            tick: 1,
            snapshot: Arc::new(weather),
            changes: Arc::new(ChangeSet::default()),
            times: Default::default(),
        },
        &mut viewer,
        0,
        &cache,
    );
    weather = WorldSnapshot {
        raining: true,
        rain_level: 0.7,
        thunder_level: 0.3,
        ..Default::default()
    };
    viewer.skipped = true;
    let wet = TickOutput {
        tick: 2,
        snapshot: Arc::new(weather),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    };
    assert_eq!(
        weather_events(&plan_viewer(&wet, &mut viewer, 0, &cache)),
        vec![(7, 0.7), (8, 0.3), (1, 0.0), (7, 0.7), (8, 0.3)]
    );
    assert!(weather_events(&plan_viewer(&wet, &mut viewer, 0, &cache)).is_empty());
}

#[test]
fn respawn_reconciles_a_dry_snapshot_with_a_full_reliable_weather_sync() {
    let mut viewer = viewer();
    plan_player_changes(
        &[PlayerChange::WeatherChanged {
            raining: true,
            rain_level: 1.0,
            thunder_level: 0.5,
        }],
        &mut viewer,
    );
    plan_player_changes(
        &[PlayerChange::Respawned {
            teleport_id: 2,
            dimension: Default::default(),
            position: [8.0, 64.0, 8.0],
            yaw: 0.0,
            pitch: 0.0,
            game_mode: 0,
        }],
        &mut viewer,
    );
    viewer.world_credit = 0;
    let dry = TickOutput {
        tick: 1,
        snapshot: Arc::new(WorldSnapshot::default()),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    };
    assert_eq!(
        weather_events(&plan_viewer(&dry, &mut viewer, 0, &ChunkPacketCache::new())),
        vec![(2, 0.0), (7, 0.0), (8, 0.0)]
    );
}

#[test]
fn unbaked_snapshot_encodes_empty_light_without_panicking() {
    let mut chunk = (*FIXTURES.dark[0]).clone();
    chunk.section_lights.clear();
    let light = chunk_light(&chunk);
    assert!(light.sky_y_mask.is_empty());
    assert!(light.block_y_mask.is_empty());
    assert!(light.empty_sky_y_mask.is_empty());
    assert!(light.empty_block_y_mask.is_empty());
    assert!(light.sky_updates.is_empty());
    assert!(light.block_updates.is_empty());
}
