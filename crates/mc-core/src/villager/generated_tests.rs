use super::{VillagerKind, VillagerProfession, conversion_tests::state, generated};
use crate::boundary::{ChangeSet, ChunkPos};
use std::sync::Arc;
struct MarkerGenerator(mc_worldgen::TerrainGenerator);
impl mc_world::ChunkGenerator for MarkerGenerator {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::ChunkGenerator::generate(&self.0, pos);
        chunk.set_generated_entities(&[mc_world::GeneratedEntityMarker {
            claim: "village:0:0:resident:1".into(),
            entity_type: "minecraft:villager".into(),
            position: [0.5, 80., 0.5],
            persistent: false,
            villager_kind: "minecraft:savanna".into(),
            profession: "minecraft:farmer".into(),
            level: 2,
            age: -12000,
            yaw: 93.,
            pitch: 7.,
        }]);
        chunk
    }
}
fn fixture() -> crate::tick::TickState {
    let mut state = state();
    state.world = crate::world::ChunkWorld::new(
        42,
        Arc::new(MarkerGenerator(mc_worldgen::TerrainGenerator::new(
            42,
            Arc::clone(&state.registry.blocks),
        ))),
        &state.registry,
        mc_world::BlockStateId(state.registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let chunk = ChunkPos { x: 0, z: 0 };
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    state
}

#[test]
fn preloaded_village_resident_spawns_once_and_claim_survives_death() {
    let mut state = fixture();
    let chunk = ChunkPos { x: 0, z: 0 };
    let mut changes = ChangeSet::default();
    generated::load(&mut state, &[], &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    let id = changes.entities_spawned[0];
    let slot = state.entities.slot_of(id).unwrap();
    let uuid = state.entities.row(slot).uuid;
    assert_eq!(state.entities.slot_of_uuid(uuid), Some(slot));
    let villager = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(villager.data.kind, VillagerKind::Savanna);
    assert_eq!(villager.data.profession, VillagerProfession::Farmer);
    assert_eq!(villager.data.level, 2);
    assert_eq!(villager.age, -12000);
    assert!(
        !villager.offers.is_empty(),
        "template babies retain merchant offers"
    );
    assert!(
        !super::trades::can_trade(villager),
        "babies cannot open a merchant"
    );
    assert_eq!(
        villager.activity,
        super::brain::schedule(villager.age, villager.data.profession, state.day_time)
    );
    assert_eq!(state.entities.rot(slot)[..2], [93., 7.]);
    assert_eq!(state.new_generated_entity_claims.len(), 1);
    state.remove_entity(slot);
    changes.entities_spawned.clear();
    generated::load(&mut state, &[chunk], &mut changes);
    assert!(changes.entities_spawned.is_empty());
    assert_eq!(state.generated_entity_claims.len(), 1);
}

#[test]
fn generated_uuid_lookup_and_claim_survive_checkpoint_restart() {
    let mut source = fixture();
    let mut changes = ChangeSet::default();
    generated::load(&mut source, &[], &mut changes);
    let id = changes.entities_spawned[0];
    let uuid = source
        .entities
        .row(source.entities.slot_of(id).unwrap())
        .uuid;
    let (_, regions, _) = crate::core::checkpoint_data_for(&source, source.tick);
    let dir = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let mut checkpoint = store.read_checkpoint().unwrap();
    let tick = source.tick.max(1);
    let entities = crate::native_checkpoint::encode_entities(
        crate::dimensions::Dimension::Overworld,
        &regions,
        &source.registry.items,
        tick,
    )
    .unwrap();
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&checkpoint.globals["checkpoint"].payload).unwrap();
    manifest.tick = tick;
    manifest.generated_entity_claims = source.generated_entity_claims.clone();
    for child in manifest.dimensions.values_mut() {
        child.tick = tick;
    }
    for record in checkpoint.globals.values_mut() {
        record.captured_tick = tick;
    }
    checkpoint.globals.get_mut("checkpoint").unwrap().payload =
        serde_json::to_vec(&manifest).unwrap();
    store
        .commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick,
                previous_tick: checkpoint.tick,
                chunks: &[],
                entities: &entities,
                players: &[],
                globals: &checkpoint.globals,
            },
            &source.registry.blocks,
            &source.registry.items,
        )
        .unwrap();
    drop(store);
    let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
    let mut restored = fixture();
    restored.tick = recovered.manifest.tick;
    let replay = &recovered.dimensions[&crate::dimensions::Dimension::Overworld];
    restored.generated_entity_claims = replay.generated_entity_claims.clone();
    for (&id, record) in &replay.entities {
        crate::core::restore_entity(&mut restored, id, record).unwrap();
    }
    assert!(restored.entities.slot_of_uuid(uuid).is_some());
    let mut changes = ChangeSet::default();
    generated::load(&mut restored, &[], &mut changes);
    assert!(changes.entities_spawned.is_empty());
    assert_eq!(restored.entities.len(), 1);
    let slot = restored.entities.slot_of_uuid(uuid).unwrap();
    assert_eq!(restored.entities.row(slot).uuid, uuid);
    let original = source
        .entities
        .row(source.entities.slot_of(id).unwrap())
        .villager
        .as_ref()
        .unwrap();
    let loaded = restored.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(loaded.offers, original.offers);
    assert_eq!(loaded.age, original.age);
    assert_eq!(loaded.activity, original.activity);
}

#[test]
fn uuid_rekey_removes_old_alias_and_swap_remove_preserves_new_lookup() {
    let mut state = fixture();
    let first = state.spawn_mob(state.registry.cow_type_id, [0.5, 80.0, 0.5], 20.0);
    let second = state.spawn_mob(state.registry.cow_type_id, [1.5, 80.0, 0.5], 20.0);
    let slot = state.entities.slot_of(second).unwrap();
    let old = state.entities.row(slot).uuid;
    let uuid = uuid::Uuid::from_u128(42);
    state.entities.set_uuid(slot, uuid);
    assert!(state.entities.slot_of_uuid(old).is_none());
    assert_eq!(state.entities.slot_of_uuid(uuid), Some(slot));
    state.remove_entity(state.entities.slot_of(first).unwrap());
    let moved = state.entities.slot_of(second).unwrap();
    assert_eq!(state.entities.slot_of_uuid(uuid), Some(moved));
    state.entities.set_uuid(moved, uuid);
    assert_eq!(state.entities.slot_of_uuid(uuid), Some(moved));
}

#[test]
fn generated_brain_schedule_and_merchant_cover_adult_baby_and_unemployed() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    for age in [-12000, 0] {
        for profession in VillagerProfession::ALL {
            let mut resident = super::VillagerState::default();
            resident.data.profession = profession;
            resident.data.level = 3;
            resident.age = age;
            resident.activity = super::brain::schedule(age, profession, 4000);
            super::trades::initialize_offers(&mut resident, &registry);
            let employed = !matches!(
                profession,
                VillagerProfession::None | VillagerProfession::Nitwit
            );
            assert_eq!(
                !resident.offers.is_empty(),
                employed,
                "{profession:?}, age {age}"
            );
            assert_eq!(
                resident.activity,
                if age < 0 {
                    super::Activity::Play
                } else if profession == VillagerProfession::Nitwit {
                    super::Activity::Idle
                } else {
                    super::Activity::Work
                }
            );
            let offers = resident.offers.clone();
            super::trades::initialize_offers(&mut resident, &registry);
            assert_eq!(resident.offers, offers, "initialization is idempotent");
        }
    }
}
