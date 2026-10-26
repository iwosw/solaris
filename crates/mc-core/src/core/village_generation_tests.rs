use super::*;

#[test]
#[ignore = "requires the local vanilla 26.1.2 content cache"]
fn fresh_v2_sites_describe_generated_villagers_from_the_same_plans() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(directory.path().to_owned(), 81);
    config.vanilla_data_dir = Some(
        std::env::var_os("SOLARIS_CONTENT_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
            }),
    );
    let startup = std::time::Instant::now();
    let registry = Arc::new(CoreRegistry::load(config.vanilla_data_dir.as_deref()).unwrap());
    let store = Arc::new(
        mc_world::native::NativeStore::create(
            &directory.path().join("world.db"),
            crate::native_checkpoint::native_identity(&config),
        )
        .unwrap(),
    );
    let manifest = CheckpointManifest {
        world_spawn: Some([0, 128, 0]),
        ..Default::default()
    };
    let mut state = load_dimension_state(
        &config,
        registry,
        crate::dimensions::Dimension::Overworld,
        &manifest,
        Replay::default(),
        store,
        &std::collections::BTreeMap::new(),
    )
    .unwrap();
    eprintln!("seed81 registry/dimension startup: {:?}", startup.elapsed());
    assert!(state.world.resident_positions().is_empty());
    let sites = state.world.village_sites_in_region((-68, -68), (101, 101));
    assert!(
        !sites.is_empty(),
        "fresh configured v2 world must expose native villages"
    );
    assert!(
        state.world.resident_positions().is_empty(),
        "site listing must not materialize chunks"
    );
    let site = &sites[0];
    assert!(!site.pieces.is_empty());
    let wanted = (site.min.z.div_euclid(16)..=site.max.z.div_euclid(16))
        .flat_map(|z| {
            (site.min.x.div_euclid(16)..=site.max.x.div_euclid(16))
                .map(move |x| (crate::boundary::ChunkPos { x, z }, 0))
        })
        .collect();
    let loaded = state.world.ensure_blocking(&wanted);
    let markers: Vec<_> = state
        .world
        .resident_positions()
        .iter()
        .flat_map(|pos| state.world.chunk(*pos).unwrap().generated_entities())
        .filter(|marker| {
            site.pieces.iter().any(|piece| {
                marker.claim.contains(&format!(
                    "@{}:{}:{}#",
                    piece.position.x, piece.position.y, piece.position.z
                ))
            })
        })
        .collect();
    assert!(
        !markers.is_empty(),
        "listed village {:?} must place authored villagers",
        site.start_chunk
    );
    let expected: std::collections::HashSet<_> = markers
        .iter()
        .map(|marker| crate::villager::generated_uuid(&marker.claim))
        .collect();
    assert_eq!(
        expected.len(),
        markers.len(),
        "each authored villager has a unique claim"
    );
    let mut changes = crate::boundary::ChangeSet::default();
    crate::villager::generated::load(&mut state, &loaded, &mut changes);
    for marker in &markers {
        let uuid = crate::villager::generated_uuid(&marker.claim);
        let slot = state
            .entities
            .slot_of_uuid(uuid)
            .expect("authored villager materialized");
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        assert!(row.villager.is_some());
        assert_eq!(state.entities.pos(slot), marker.position);
        assert!(changes.entities_spawned.contains(&body.id));
    }
    let count = changes.entities_spawned.len();
    crate::villager::generated::load(&mut state, &loaded, &mut changes);
    assert_eq!(
        changes.entities_spawned.len(),
        count,
        "the same site must not duplicate villagers"
    );
    assert_eq!(
        state
            .world
            .village_sites_in_region(site.start_chunk, site.start_chunk),
        vec![site.clone()]
    );
    eprintln!(
        "seed81 listed {} villages; {:?} materialized {} original UUIDs",
        sites.len(),
        site.start_chunk,
        expected.len()
    );
}
