use super::*;
use crate::dimensions::Dimension;
use crate::journal::JournalWriter;
use crate::native_checkpoint::record;
use crate::native_checkpoint::tests::{fixture_store, read_manifest, write_manifest};
use mc_nbt::{ListTag, Tag, tag_type};
use mc_world::native::{NativeCheckpoint, NativeEntitySnapshot};
use mc_world::{BlockStateId, Chunk, ChunkPos};

const SEED: i64 = 42;

fn fields(
    registry: &crate::registry::CoreRegistry,
    id: &str,
    n: u128,
    wire: i32,
) -> Vec<(String, Tag)> {
    mc_data::entity_nbt::entity_to_fields(
        &registry.items,
        &mc_data::entity_nbt::VanillaEntity {
            id: id.into(),
            uuid: uuid::Uuid::from_u128(n),
            wire_id: wire,
            pos: [1.5, 80.0, 1.5],
            ..Default::default()
        },
    )
    .unwrap()
}

fn member(
    mut fields: Vec<(String, Tag)>,
    settlement: i64,
    household: Option<&str>,
) -> Vec<(String, Tag)> {
    let mut runtime = vec![("settlement".to_string(), Tag::Long(settlement))];
    if let Some(household) = household {
        runtime.push(("household".into(), Tag::String(household.into())));
    }
    fields.push(("solaris:VillagerRuntime".into(), Tag::Compound(runtime)));
    fields
}

fn settlement_row(id: u64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "dimension": Dimension::Overworld,
        "value": {
            "id": id, "name": "Falcon", "owner": "operator", "anchor": [0, 80, 0],
            "radius": 32, "revision": 1, "household_sequence": 1,
            "households": {format!("{id}-1"): {
                "id": format!("{id}-1"), "revision": 1, "origin": "adoption",
                "home": null, "needs": null, "policy": null
            }}
        }
    }))
    .unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    world: PathBuf,
    config: CoreConfig,
    tick: u64,
}

type EntityFixture = (u32, i32, i32, u128, Vec<(String, Tag)>);

/// Stopped native world: `entities` as (dimension, chunk x, chunk z, UUID, payload),
/// optional persisted settlement 7, one baseline overworld chunk.
fn world(entities: Vec<EntityFixture>, settlement: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let world = dir.path().join("world");
    let config = CoreConfig::new(world.clone(), SEED);
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let store = fixture_store(&config).unwrap();
    write_manifest(&config, &read_manifest(&config).unwrap()).unwrap();
    let chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(registry.states.air),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    store
        .fill_generated(0, &[chunk], &registry.blocks, &registry.items)
        .unwrap();
    let mut image = store.read_checkpoint().unwrap();
    let tick = image.tick;
    if settlement {
        image
            .globals
            .insert("settlement/7".into(), record(tick, settlement_row(7)));
        image.globals.insert(
            "settlement_sequence".into(),
            record(tick, serde_json::to_vec(&7u64).unwrap()),
        );
    }
    let snapshots: Vec<_> = entities
        .into_iter()
        .map(|(dimension, x, z, uuid, payload)| NativeEntitySnapshot {
            identity: *uuid::Uuid::from_u128(uuid).as_bytes(),
            dimension,
            position: ChunkPos { x, z },
            generated_slot: None,
            journal_lsn: tick,
            payload: Some(Tag::Compound(payload)),
        })
        .collect();
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick,
                previous_tick: tick,
                chunks: &[],
                entities: &snapshots,
                players: &[],
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

fn populated() -> Fixture {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let cow_rider = Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements: vec![Tag::Compound(member(
            fields(&registry, "minecraft:villager", 0x30, 30),
            7,
            Some("7-1"),
        ))],
    });
    let mut cow = fields(&registry, "minecraft:cow", 0x20, 20);
    cow.push(("Passengers".into(), cow_rider));
    world(
        vec![
            (
                0,
                0,
                0,
                0x10,
                member(
                    fields(&registry, "minecraft:villager", 0x10, 10),
                    7,
                    Some("7-1"),
                ),
            ),
            (
                0,
                0,
                0,
                0x11,
                member(fields(&registry, "minecraft:villager", 0x11, 11), 7, None),
            ),
            (0, 0, 0, 0x20, cow),
            (
                1,
                5,
                5,
                0x12,
                member(fields(&registry, "minecraft:villager", 0x12, 12), 99, None),
            ),
        ],
        true,
    )
}

fn read(path: &Path) -> CensusReceipt {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn census_counts_entities_passengers_villagers_chunks_and_persisted_settlements() {
    let fixture = populated();
    let before = hashes(&fixture.world).unwrap();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("census.json");
    let receipt = census_native(&fixture.config, &fixture.world, &path).unwrap();
    assert_eq!(read(&path).counts, receipt.counts);
    assert_eq!(receipt.status, "PASS");
    assert!(receipt.read_only);
    assert_eq!(receipt.checkpoint_tick, Some(fixture.tick));
    assert_eq!(receipt.source_unchanged, Some(true));
    let counts = |key: &str| receipt.counts[key];
    // Three overworld roots + one nether root; the cow carries one passenger.
    assert_eq!(counts("entity_roots"), 4);
    assert_eq!(counts("entities"), 5);
    assert_eq!(counts("villagers"), 4);
    assert_eq!(counts("chunks"), 1);
    assert_eq!(counts("settlements"), 1);
    assert_eq!(counts("households"), 1);
    assert_eq!(counts("settlement_members"), 4);
    assert_eq!(counts("household_members"), 2);
    assert_eq!(counts("settlement_orphan_members"), 1);
    assert_eq!(counts("settlement_sequence"), 7);
    let overworld = &receipt.dimensions["minecraft:overworld"];
    assert_eq!(
        (
            overworld.entity_roots,
            overworld.entities,
            overworld.villagers
        ),
        (3, 4, 3)
    );
    assert_eq!((overworld.chunks, overworld.settlements), (1, 1));
    let nether = &receipt.dimensions["minecraft:the_nether"];
    assert_eq!(
        (nether.entity_roots, nether.entities, nether.villagers),
        (1, 1, 1)
    );
    assert_eq!(
        receipt.dimensions["minecraft:the_end"],
        DimensionCensus::default()
    );
    assert_eq!(
        receipt.settlement_details,
        vec![SettlementCensus {
            id: 7,
            name: "Falcon".into(),
            dimension: "minecraft:overworld".into(),
            anchor: [0, 80, 0],
            radius: 32,
            households: 1,
            members: 3,
            household_members: 2,
        }]
    );
    assert_eq!(
        hashes(&fixture.world).unwrap(),
        before,
        "source bytes unchanged"
    );
}

#[test]
fn census_reports_zero_settlements_when_no_core_rows_exist() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let fixture = world(
        vec![(
            0,
            0,
            0,
            0x10,
            fields(&registry, "minecraft:villager", 0x10, 10),
        )],
        false,
    );
    let out = tempfile::tempdir().unwrap();
    let receipt =
        census_native(&fixture.config, &fixture.world, &out.path().join("c.json")).unwrap();
    assert_eq!(receipt.counts["villagers"], 1);
    assert_eq!(receipt.counts["settlements"], 0);
    assert_eq!(receipt.counts["settlement_members"], 0);
    assert!(receipt.settlement_details.is_empty());
}

#[test]
fn census_refuses_missing_native_world_without_touching_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir(&source).unwrap();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("c.json");
    let error = census_native(&CoreConfig::new(source.clone(), SEED), &source, &path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("world import-anvil"), "{error}");
    let receipt = read(&path);
    assert_eq!(receipt.status, "FAIL");
    assert!(receipt.counts.is_empty());
    assert_eq!(fs::read_dir(&source).unwrap().count(), 0);
}

#[test]
fn census_refuses_journal_tail_instead_of_reporting_stale_counts() {
    let fixture = populated();
    let mut writer = JournalWriter::open(&fixture.world, 1).unwrap();
    writer
        .append(fixture.tick + 1, fixture.tick + 1, &[])
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let before = hashes(&fixture.world).unwrap();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("c.json");
    let error = census_native(&fixture.config, &fixture.world, &path)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("complete batches after checkpoint tick"),
        "{error}"
    );
    let receipt = read(&path);
    assert_eq!(receipt.status, "FAIL");
    assert!(receipt.counts.is_empty());
    assert_eq!(hashes(&fixture.world).unwrap(), before);
}

#[test]
fn census_refuses_entity_payload_uuid_that_differs_from_its_record_key() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let fixture = world(
        vec![(
            0,
            0,
            0,
            0x10,
            fields(&registry, "minecraft:villager", 0x99, 10),
        )],
        false,
    );
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("c.json");
    let error = census_native(&fixture.config, &fixture.world, &path)
        .unwrap_err()
        .to_string();
    for detail in [
        "differs from native entity record key",
        "chunk (0, 0)",
        "minecraft:villager",
    ] {
        assert!(error.contains(detail), "missing {detail:?} in {error}");
    }
    assert_eq!(read(&path).status, "FAIL");
}

#[test]
fn census_receipt_inside_source_is_refused_without_writing() {
    let fixture = populated();
    let path = fixture.world.join("census.json");
    assert!(census_native(&fixture.config, &fixture.world, &path).is_err());
    assert!(!path.exists());
}
