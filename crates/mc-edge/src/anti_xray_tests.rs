//! anti_xray.rs unit tests: enclosure rule, per-dimension stand-ins, chunk
//! borders, reveal-on-change, and the shared chunk cache integration.

use std::sync::Arc;

use super::*;
use crate::fanout::publish_with_registries;
use crate::plan::{ChunkPacketCache, PlanRegistries, ViewerState};
use mc_core::boundary::{
    BlockChange, ChangeSet, ConnectionId, EntityId, SnapshotChunk, TickOutput, WorldSnapshot,
};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    LevelChunkWithLight, SectionBlockChange, SectionBlocksUpdate, pack_section_pos,
    pack_section_relative_pos,
};
use mc_world::chunk::{ChunkPos, LightSection, SectionLight};

struct Fixture {
    biomes: mc_data::Registry,
    blocks: BlockRegistry,
    light: BlockLightTable,
    items: mc_data::items::ItemRegistry,
    block_entity_types: mc_data::block_entity_types::BlockEntityTypeRegistry,
}

fn fixture() -> Fixture {
    let report = mc_data::blocks::solaris_required_blocks_report();
    Fixture {
        biomes: crate::chunks::test_biome_registry(&["minecraft:plains"]),
        blocks: BlockRegistry::from_report(&report).expect("blocks build"),
        light: BlockLightTable::conservative_from_blocks_report(&report),
        items: mc_data::items::solaris_required_items(),
        block_entity_types: mc_data::block_entity_types::solaris_required_block_entity_types(),
    }
}

impl Fixture {
    fn id(&self, name: &str) -> BlockStateId {
        self.blocks
            .block(&Identifier::parse(format!("minecraft:{name}")).expect("id"))
            .unwrap_or_else(|| panic!("{name} registered"))
            .default
    }

    fn policy(&self, overworld: bool, nether: bool) -> AntiXray {
        AntiXray::new(
            &self.blocks,
            &self.light,
            &AntiXraySection { overworld, nether },
        )
        .expect("policy builds")
        .expect("a dimension is enabled")
    }

    fn registries(&self) -> PlanRegistries<'_> {
        PlanRegistries {
            biomes: &self.biomes,
            blocks: &self.blocks,
            items: &self.items,
            block_entity_types: &self.block_entity_types,
        }
    }

    /// Chunk whose sections for `y` in `-16..16` are solid `rock` (sections 3
    /// and 4 of the overworld geometry) and the rest air, with baked light.
    fn chunk(&self, x: i32, rock: &str) -> Chunk {
        let air = self.id("air");
        let mut chunk = Chunk::empty(
            ChunkPos { x, z: 0 },
            air,
            Identifier::parse("minecraft:plains").expect("id"),
        );
        let rock = self.id(rock);
        chunk.sections[3] = ChunkSection::filled(rock, air);
        chunk.sections[4] = ChunkSection::filled(rock, air);
        chunk.section_lights.fill(SectionLight {
            block: Some(LightSection::uniform(0)),
            sky: Some(LightSection::uniform(0xFF)),
        });
        chunk
    }
}

fn resident(chunks: Vec<(u64, Chunk)>) -> Vec<SnapshotChunk> {
    let mut chunks: Vec<SnapshotChunk> = chunks
        .into_iter()
        .map(|(revision, chunk)| SnapshotChunk {
            pos: chunk.pos,
            revision,
            chunk: Arc::new(chunk),
        })
        .collect();
    chunks.sort_by_key(|chunk| (chunk.pos.x, chunk.pos.z));
    chunks
}

fn hidden_at(
    overrides: &[Option<ChunkSection>],
    chunk: &Chunk,
    x: u8,
    y: i32,
    z: u8,
) -> BlockStateId {
    let (index, sy) = chunk.geometry().world_y_to_section(y).expect("in range");
    overrides.get(index).and_then(Option::as_ref).map_or_else(
        || chunk.get_block(x, y, z).expect("in range"),
        |section| section.get(x, sy, z),
    )
}

#[test]
fn enclosed_ore_is_replaced_and_exposed_ore_kept() {
    let f = fixture();
    let (stone, diamond, air) = (f.id("stone"), f.id("diamond_ore"), f.id("air"));
    let mut chunk = f.chunk(0, "stone");
    chunk.set_block(5, 10, 5, diamond); // enclosed
    chunk.set_block(9, 10, 9, diamond); // air above
    chunk.set_block(9, 11, 9, air);
    chunk.set_block(12, 10, 12, diamond); // ore on ore, both enclosed
    chunk.set_block(12, 11, 12, diamond);
    let chunks = resident(vec![(1, chunk)]);
    let centre = &chunks[0];
    let overrides = f
        .policy(true, true)
        .hide(Dimension::Overworld, &Surroundings::of(&chunks, centre));
    let chunk = &*centre.chunk;
    assert_eq!(hidden_at(&overrides, chunk, 5, 10, 5), stone);
    assert_eq!(hidden_at(&overrides, chunk, 9, 10, 9), diamond);
    assert_eq!(hidden_at(&overrides, chunk, 12, 10, 12), stone);
    assert_eq!(hidden_at(&overrides, chunk, 12, 11, 12), stone);
    // Only the section holding ores is copied; the authoritative chunk is untouched.
    assert_eq!(overrides.iter().filter(|s| s.is_some()).count(), 1);
    assert_eq!(chunk.get_block(5, 10, 5), Some(diamond));
}

#[test]
fn stand_in_matches_the_surrounding_rock_per_depth_and_kind() {
    let f = fixture();
    let (stone, deepslate) = (f.id("stone"), f.id("deepslate"));
    let mut chunk = f.chunk(0, "stone");
    chunk.set_block(3, 5, 3, f.id("iron_ore"));
    chunk.set_block(3, 5, 8, f.id("deepslate_iron_ore"));
    chunk.set_block(3, -5, 3, f.id("iron_ore"));
    chunk.set_block(3, -5, 8, f.id("deepslate_diamond_ore"));
    let chunks = resident(vec![(1, chunk)]);
    let centre = &chunks[0];
    let overrides = f
        .policy(true, false)
        .hide(Dimension::Overworld, &Surroundings::of(&chunks, centre));
    let chunk = &*centre.chunk;
    assert_eq!(hidden_at(&overrides, chunk, 3, 5, 3), stone);
    assert_eq!(hidden_at(&overrides, chunk, 3, 5, 8), deepslate);
    // Plain ore below y=0 sits in deepslate: a stone block there would give it away.
    assert_eq!(hidden_at(&overrides, chunk, 3, -5, 3), deepslate);
    assert_eq!(hidden_at(&overrides, chunk, 3, -5, 8), deepslate);
}

#[test]
fn nether_ores_become_netherrack_and_disabled_dimensions_are_untouched() {
    let f = fixture();
    let netherrack = f.id("netherrack");
    let mut chunk = f.chunk(0, "netherrack");
    chunk.set_block(4, 5, 4, f.id("nether_quartz_ore"));
    chunk.set_block(4, 5, 9, f.id("ancient_debris"));
    chunk.set_block(8, 5, 9, f.id("diamond_ore"));
    let chunks = resident(vec![(1, chunk)]);
    let centre = &chunks[0];
    let around = Surroundings::of(&chunks, centre);
    let chunk = &*centre.chunk;
    let both = f.policy(true, true);
    let overrides = both.hide(Dimension::Nether, &around);
    assert_eq!(hidden_at(&overrides, chunk, 4, 5, 4), netherrack);
    assert_eq!(hidden_at(&overrides, chunk, 4, 5, 9), netherrack);
    // Overworld ores are not nether ores.
    assert_eq!(hidden_at(&overrides, chunk, 8, 5, 9), f.id("diamond_ore"));
    assert!(both.hide(Dimension::End, &around).is_empty());
    let overworld_only = f.policy(true, false);
    assert!(!overworld_only.enabled(Dimension::Nether));
    assert!(overworld_only.hide(Dimension::Nether, &around).is_empty());
    assert!(
        AntiXray::new(
            &f.blocks,
            &f.light,
            &AntiXraySection {
                overworld: false,
                nether: false
            }
        )
        .expect("builds")
        .is_none()
    );
}

#[test]
fn chunk_border_ores_look_across_into_the_neighbour() {
    let f = fixture();
    let (stone, diamond, air) = (f.id("stone"), f.id("diamond_ore"), f.id("air"));
    let policy = f.policy(true, true);
    let build = |west: Option<Chunk>| {
        let mut chunk = f.chunk(0, "stone");
        chunk.set_block(0, 10, 7, diamond);
        let mut chunks = vec![(1, chunk)];
        chunks.extend(west.map(|west| (1, west)));
        resident(chunks)
    };
    let hide_in = |chunks: &[SnapshotChunk]| {
        let centre = chunks.iter().find(|c| c.pos.x == 0).expect("centre");
        let overrides = policy.hide(Dimension::Overworld, &Surroundings::of(chunks, centre));
        hidden_at(&overrides, &centre.chunk, 0, 10, 7)
    };
    // Unloaded neighbour: nothing is hidden on guesswork.
    assert_eq!(hide_in(&build(None)), diamond);
    assert_eq!(hide_in(&build(Some(f.chunk(-1, "stone")))), stone);
    let mut open = f.chunk(-1, "stone");
    open.set_block(15, 10, 7, air);
    assert_eq!(hide_in(&build(Some(open))), diamond);
}

#[test]
fn mining_next_to_an_enclosed_ore_reveals_exactly_it() {
    let f = fixture();
    let (diamond, air, dirt) = (f.id("diamond_ore"), f.id("air"), f.id("dirt"));
    let policy = f.policy(true, true);
    let mut chunk = f.chunk(0, "stone");
    chunk.set_block(5, 10, 5, diamond);
    chunk.set_block(5, 13, 5, diamond); // stays enclosed
    chunk.set_block(0, 12, 3, diamond);
    chunk.set_block(5, 11, 5, air); // the mined block, above the first ore
    chunk.set_block(8, 10, 8, dirt); // solid-for-solid change: reveals nothing
    chunk.set_block(8, 10, 9, diamond); // beside the dirt: still enclosed
    let mut west = f.chunk(-1, "stone");
    west.set_block(15, 12, 3, air); // mined across the chunk border
    let chunks = resident(vec![(2, chunk), (2, west)]);
    let reveal = |x, y, z| {
        let mut revealed = Vec::new();
        policy.reveal_around(
            Dimension::Overworld,
            &chunks,
            BlockPos { x, y, z },
            &mut revealed,
        );
        revealed
    };
    assert_eq!(reveal(5, 11, 5), [BlockPos { x: 5, y: 10, z: 5 }]);
    assert!(reveal(8, 10, 8).is_empty());
    assert_eq!(reveal(-1, 12, 3), [BlockPos { x: 0, y: 12, z: 3 }]);
    // The ore still enclosed is the only one a fresh chunk packet hides.
    let centre = &chunks[1];
    let overrides = policy.hide(Dimension::Overworld, &Surroundings::of(&chunks, centre));
    assert_eq!(
        hidden_at(&overrides, &centre.chunk, 5, 13, 5),
        f.id("stone")
    );
    assert_eq!(hidden_at(&overrides, &centre.chunk, 5, 10, 5), diamond);
    assert_eq!(hidden_at(&overrides, &centre.chunk, 0, 12, 3), diamond);
    let mut none = Vec::new();
    policy.reveal_around(
        Dimension::End,
        &chunks,
        BlockPos { x: 5, y: 11, z: 5 },
        &mut none,
    );
    assert!(none.is_empty());
}

fn tick(revision: u64, chunks: Vec<SnapshotChunk>, changes: Vec<BlockChange>) -> TickOutput {
    TickOutput {
        tick: revision,
        snapshot: Arc::new(WorldSnapshot {
            tick: revision,
            day_time: 0,
            chunks: Arc::new(chunks),
            day_time_rate: 1.0,
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet {
            blocks: changes,
            ..Default::default()
        }),
        times: Default::default(),
    }
}

fn frames(packets: Vec<crate::fanout::AddressedPacket>, id: i32) -> Vec<bytes::Bytes> {
    packets
        .into_iter()
        .filter_map(|packet| {
            let mut framed = bytes::BytesMut::from(&packet.data[..]);
            let frame = mc_protocol::frame::try_decode_frame(
                &mut framed,
                mc_protocol::frame::Compression::Threshold(256),
            )
            .ok()
            .flatten()?;
            (frame.id == id).then_some(frame.body)
        })
        .collect()
}

fn viewer(connection: u64) -> ViewerState {
    ViewerState::new(
        ConnectionId(connection),
        EntityId(connection as i32),
        [8.0, 64.0, 8.0],
    )
}

/// The chunk body the first viewer receives at view distance 0.
fn chunk_body(output: &TickOutput, cache: &mut ChunkPacketCache, f: &Fixture) -> bytes::Bytes {
    let mut v = viewer(1);
    let mut bodies = frames(
        publish_with_registries(output, [&mut v].into_iter(), 0, &f.registries(), cache),
        LevelChunkWithLight::ID,
    );
    assert_eq!(bodies.len(), 1, "one chunk packet");
    bodies.remove(0)
}

#[test]
fn shared_cache_encodes_the_stand_in_once_for_every_viewer() {
    let f = fixture();
    let (diamond, stone) = (f.id("diamond_ore"), f.id("stone"));
    let policy = Some(Arc::new(f.policy(true, true)));
    let mut ore = f.chunk(0, "stone");
    ore.set_block(5, 10, 5, diamond);
    let mut plain = f.chunk(0, "stone");
    plain.set_block(5, 10, 5, stone);
    let ore_tick = tick(1, resident(vec![(1, ore)]), Vec::new());
    let plain_tick = tick(1, resident(vec![(1, plain)]), Vec::new());

    let mut hiding = ChunkPacketCache::with_anti_xray(policy);
    let hidden = chunk_body(&ore_tick, &mut hiding, &f);
    let expected = chunk_body(&plain_tick, &mut ChunkPacketCache::new(), &f);
    let leaked = chunk_body(&ore_tick, &mut ChunkPacketCache::new(), &f);
    assert_eq!(hidden, expected, "ore is not in the chunk packet");
    assert_ne!(hidden, leaked, "without anti-xray the ore is sent");
    // A second viewer joining later is served the same shared hidden frame.
    assert_eq!(chunk_body(&ore_tick, &mut hiding, &f), hidden);
}

#[test]
fn neighbour_change_invalidates_the_cached_border_chunk() {
    let f = fixture();
    let (diamond, air) = (f.id("diamond_ore"), f.id("air"));
    let mut ore = f.chunk(0, "stone");
    ore.set_block(15, 10, 7, diamond);
    let east_solid = f.chunk(1, "stone");
    let mut east_open = f.chunk(1, "stone");
    east_open.set_block(0, 10, 7, air);
    let mut cache = ChunkPacketCache::with_anti_xray(Some(Arc::new(f.policy(true, true))));
    let ore = Arc::new(ore);
    let chunks = |east_revision, east: Chunk| {
        vec![
            SnapshotChunk {
                pos: ChunkPos { x: 0, z: 0 },
                revision: 1,
                chunk: Arc::clone(&ore),
            },
            SnapshotChunk {
                pos: ChunkPos { x: 1, z: 0 },
                revision: east_revision,
                chunk: Arc::new(east),
            },
        ]
    };
    let enclosed = chunk_body(&tick(1, chunks(1, east_solid), Vec::new()), &mut cache, &f);
    // Only the east neighbour changed (revision 2): the centre chunk's own
    // revision is the same, yet a viewer joining now must see the exposed ore.
    let exposed = chunk_body(&tick(2, chunks(2, east_open), Vec::new()), &mut cache, &f);
    assert_ne!(enclosed, exposed);
    let leaked = chunk_body(
        &tick(2, resident(vec![(1, (*ore).clone())]), Vec::new()),
        &mut ChunkPacketCache::new(),
        &f,
    );
    assert_eq!(exposed, leaked, "exposed ore is sent as the real block");
}

#[test]
fn publish_sends_the_real_ore_when_a_block_beside_it_is_mined() {
    let f = fixture();
    let (diamond, air) = (f.id("diamond_ore"), f.id("air"));
    let mut cache = ChunkPacketCache::with_anti_xray(Some(Arc::new(f.policy(true, true))));
    let mut before = f.chunk(0, "stone");
    before.set_block(5, 10, 5, diamond);
    before.set_block(5, 13, 5, diamond); // two blocks from the mined one: stays hidden
    let mut after = before.clone();
    after.set_block(5, 11, 5, air);
    let mut v = viewer(1);
    let registries = f.registries();
    let first = publish_with_registries(
        &tick(1, resident(vec![(1, before)]), Vec::new()),
        [&mut v].into_iter(),
        0,
        &registries,
        &mut cache,
    );
    assert_eq!(frames(first, LevelChunkWithLight::ID).len(), 1);
    let mined = BlockPos { x: 5, y: 11, z: 5 };
    let second = publish_with_registries(
        &tick(
            2,
            resident(vec![(2, after)]),
            vec![BlockChange {
                position: mined,
                state: air.0,
            }],
        ),
        [&mut v].into_iter(),
        0,
        &registries,
        &mut cache,
    );
    let updates = frames(second, SectionBlocksUpdate::ID);
    assert_eq!(updates.len(), 1, "mine and reveal share one section packet");
    let mut body = updates[0].as_ref();
    let mut update = SectionBlocksUpdate::decode(&mut body).unwrap();
    assert!(body.is_empty());
    assert_eq!(update.section_pos, pack_section_pos(0, 0, 0));
    update.changes.sort_by_key(|change| change.relative_pos);
    assert_eq!(
        update.changes,
        vec![
            SectionBlockChange {
                relative_pos: pack_section_relative_pos(5, 10, 5),
                state_id: diamond.0 as i32,
            },
            SectionBlockChange {
                relative_pos: pack_section_relative_pos(5, 11, 5),
                state_id: air.0 as i32,
            },
        ],
        "publish the mined cell and exposed ore only: no enclosed ore or stand-in"
    );
}
