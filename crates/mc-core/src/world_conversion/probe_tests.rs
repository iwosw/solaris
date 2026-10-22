//! Probe read-back: a barrel's typed slots, a stored player inventory and a
//! stored item entity all come from the checkpoint image, never from live state.
use super::probe::{ProbeBlock, ProbeEntity, ProbeItem};
use super::*;
use crate::journal::{JournalWriter, ReplayPlayer, player_to_tag};
use crate::native_checkpoint::tests::{fixture_store, read_manifest, write_manifest};
use mc_nbt::Tag;
use mc_world::native::{NativeCheckpoint, NativeEntitySnapshot, NativePlayerSnapshot};
use mc_world::{BlockPos, BlockStateId, ChestBlockEntity, Chunk, ChunkPos, FurnaceSlot};

const SEED: i64 = 42;
const GRAVE: [i32; 3] = [0, 64, 0];

fn identifier(value: &str) -> mc_data::Identifier {
    mc_data::Identifier::parse(value).expect("identifier")
}

fn dirt(registry: &crate::registry::CoreRegistry, count: i32) -> mc_data::ItemStack {
    mc_data::ItemStack {
        count,
        item_id: registry
            .items
            .id_of(&identifier("minecraft:dirt"))
            .expect("dirt item"),
        ..mc_data::ItemStack::EMPTY
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    world: PathBuf,
    config: CoreConfig,
    tick: u64,
}

/// Stopped native world with a barrel of 7 dirt at [`GRAVE`], one stored player
/// carrying the given inventory and one optional stored item entity.
fn world(inventory: Vec<mc_data::ItemStack>, dropped: Option<mc_data::ItemStack>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let world = dir.path().join("world");
    let config = CoreConfig::new(world.clone(), SEED);
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let store = fixture_store(&config).unwrap();
    write_manifest(&config, &read_manifest(&config).unwrap()).unwrap();
    let barrel = registry
        .blocks
        .by_name_and_props(
            &identifier("minecraft:barrel"),
            &[
                ("facing".into(), "north".into()),
                ("open".into(), "false".into()),
            ],
        )
        .expect("barrel default state");
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(registry.states.air),
        identifier("minecraft:plains"),
    );
    chunk
        .set_block(0, GRAVE[1], 0, barrel)
        .expect("in geometry");
    let mut slots: [FurnaceSlot; 27] = std::array::from_fn(|_| FurnaceSlot::default());
    slots[0] = FurnaceSlot {
        count: 7,
        item_id: dirt(&registry, 7).item_id,
        ..FurnaceSlot::default()
    };
    chunk.chests.insert(
        BlockPos {
            x: GRAVE[0],
            y: GRAVE[1],
            z: GRAVE[2],
        },
        ChestBlockEntity {
            slots,
            loot_table: None,
            loot_seed: 0,
        },
    );
    store
        .fill_generated(0, &[chunk], &registry.blocks, &registry.items)
        .unwrap();
    let image = store.read_checkpoint().unwrap();
    let tick = image.tick;
    let mut player = ReplayPlayer {
        opaque_player_fields: Vec::new(),
        statistics: Default::default(),
        advancement_progress: crate::journal::empty_advancement_progress(),
        unlocked_recipes: Default::default(),
        dimension: Default::default(),
        pos: [0.5, 80.0, 0.5],
        yaw: 0.0,
        pitch: 0.0,
        health: 20.0,
        food: 20,
        saturation: 5.0,
        exhaustion: 0.0,
        air: 300,
        experience_level: 0,
        experience_progress: 0.0,
        experience_total: 0,
        game_mode: 0,
        selected: 0,
        inventory: vec![mc_data::ItemStack::EMPTY; 41],
        carried: mc_data::ItemStack::EMPTY,
        grid: Vec::new(),
        respawn: None,
        respawn_dimension: Default::default(),
        respawn_forced: false,
        respawn_yaw: 0.0,
        respawn_pitch: 0.0,
        active_effects: Default::default(),
        enchantment_seed: 0,
        warden_warning: Default::default(),
        current_impulse_impact_pos: None,
        fall_flying: false,
        burn_ticks: 0,
        portal: Default::default(),
        tick,
    };
    for (slot, stack) in inventory.into_iter().enumerate() {
        player.inventory[slot] = stack;
    }
    let players = vec![NativePlayerSnapshot {
        uuid: *uuid::Uuid::from_u128(0x50).as_bytes(),
        record: crate::native_checkpoint::tag_record(
            tick,
            &player_to_tag(&player, &registry.items).unwrap(),
        )
        .unwrap(),
    }];
    let entities: Vec<NativeEntitySnapshot> = dropped
        .map(|stack| {
            let fields = mc_data::entity_nbt::entity_to_fields(
                &registry.items,
                &mc_data::entity_nbt::VanillaEntity {
                    id: "minecraft:item".into(),
                    uuid: uuid::Uuid::from_u128(0x40),
                    pos: [1.5, 80.0, 1.5],
                    item: Some(stack),
                    wire_id: 9,
                    ..Default::default()
                },
            )
            .unwrap();
            NativeEntitySnapshot {
                identity: *uuid::Uuid::from_u128(0x40).as_bytes(),
                dimension: 0,
                position: ChunkPos { x: 0, z: 0 },
                generated_slot: None,
                journal_lsn: tick,
                payload: Some(Tag::Compound(fields)),
            }
        })
        .into_iter()
        .collect();
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick,
                previous_tick: tick,
                chunks: &[],
                entities: &entities,
                players: &players,
                globals: &image.globals,
            },
            &registry.blocks,
            &registry.items,
        )
        .unwrap();
    drop(store);
    Fixture {
        _dir: dir,
        world,
        config,
        tick,
    }
}

fn read(path: &Path) -> ProbeReceipt {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn probe_reports_grave_slots_player_inventory_and_stored_drops() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let fixture = world(vec![dirt(&registry, 2)], Some(dirt(&registry, 3)));
    let before = hashes(&fixture.world).unwrap();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("probe.json");
    let receipt = probe_native(&fixture.config, &fixture.world, &[GRAVE], &path).unwrap();
    assert_eq!(receipt.status, "PASS");
    assert!(receipt.read_only);
    assert_eq!(receipt.checkpoint_tick, Some(fixture.tick));
    assert_eq!(receipt.source_unchanged, Some(true));
    assert_eq!(
        receipt.blocks,
        vec![ProbeBlock {
            dimension: "minecraft:overworld".into(),
            x: GRAVE[0],
            y: GRAVE[1],
            z: GRAVE[2],
            state: Some(
                registry
                    .blocks
                    .by_name_and_props(
                        &identifier("minecraft:barrel"),
                        &[
                            ("facing".into(), "north".into()),
                            ("open".into(), "false".into())
                        ],
                    )
                    .unwrap()
                    .0
            ),
            block: Some("minecraft:barrel".into()),
            items: vec![ProbeItem {
                item: "minecraft:dirt".into(),
                count: 7,
                place: "container".into(),
                slot: 0,
            }],
        }]
    );
    assert_eq!(receipt.players.len(), 1);
    assert_eq!(
        receipt.players[0].items,
        vec![ProbeItem {
            item: "minecraft:dirt".into(),
            count: 2,
            place: "inventory".into(),
            slot: 0,
        }]
    );
    assert_eq!(
        receipt.item_entities,
        vec![ProbeEntity {
            type_id: registry
                .entity_types
                .id_of(&identifier("minecraft:item"))
                .unwrap(),
            position: [1.5, 80.0, 1.5],
            item: Some("minecraft:dirt".into()),
            count: 3,
        }]
    );
    assert_eq!(read(&path).blocks, receipt.blocks);
    assert_eq!(
        hashes(&fixture.world).unwrap(),
        before,
        "source bytes unchanged"
    );
}

#[test]
fn probe_reports_no_copies_after_custody() {
    let fixture = world(Vec::new(), None);
    let out = tempfile::tempdir().unwrap();
    let receipt = probe_native(
        &fixture.config,
        &fixture.world,
        &[GRAVE],
        &out.path().join("p.json"),
    )
    .unwrap();
    assert_eq!(receipt.status, "PASS");
    assert_eq!(receipt.players.len(), 1);
    assert!(receipt.players[0].items.is_empty());
    assert!(receipt.item_entities.is_empty());
    assert_eq!(
        receipt.blocks[0].items.len(),
        1,
        "the barrel keeps the custody"
    );
}

#[test]
fn probe_refuses_missing_native_world_without_touching_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir(&source).unwrap();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("p.json");
    let error = probe_native(
        &CoreConfig::new(source.clone(), SEED),
        &source,
        &[GRAVE],
        &path,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("world import-anvil"), "{error}");
    let receipt = read(&path);
    assert_eq!(receipt.status, "FAIL");
    assert!(receipt.blocks.is_empty());
    assert_eq!(fs::read_dir(&source).unwrap().count(), 0);
}

#[test]
fn probe_refuses_journal_tail_instead_of_reporting_a_stale_image() {
    let fixture = world(Vec::new(), None);
    let mut writer = JournalWriter::open(&fixture.world, 1).unwrap();
    writer
        .append(fixture.tick + 1, fixture.tick + 1, &[])
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let before = hashes(&fixture.world).unwrap();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("p.json");
    let error = probe_native(&fixture.config, &fixture.world, &[GRAVE], &path)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("complete batches after checkpoint tick"),
        "{error}"
    );
    assert_eq!(read(&path).status, "FAIL");
    assert_eq!(hashes(&fixture.world).unwrap(), before);
}

#[test]
fn probe_receipt_inside_source_is_refused_without_writing() {
    let fixture = world(Vec::new(), None);
    let path = fixture.world.join("probe.json");
    assert!(probe_native(&fixture.config, &fixture.world, &[GRAVE], &path).is_err());
    assert!(!path.exists());
}
