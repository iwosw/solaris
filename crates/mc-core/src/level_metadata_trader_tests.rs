use super::*;
use crate::mob_lanes::TraderSpawner;

#[test]
fn wandering_trader_saved_ints_import_and_export_without_range_normalization() {
    let path = "data/minecraft/wandering_trader.dat";
    for clock in [
        TraderSpawner {
            delay: 0,
            chance: 0,
        },
        TraderSpawner {
            delay: -1,
            chance: 100,
        },
        TraderSpawner {
            delay: 24_001,
            chance: -25,
        },
        TraderSpawner {
            delay: i32::MIN,
            chance: i32::MAX,
        },
        TraderSpawner {
            delay: i32::MAX,
            chance: i32::MIN,
        },
    ] {
        let mut tags = MetadataTags::new();
        let source = CheckpointManifest {
            world_spawn: Some([0, 80, 0]),
            wandering_trader: Some(clock),
            ..Default::default()
        };
        checkpoint_tags(&mut tags, 42, &source).unwrap();
        let data = saved_data(&tags, path).unwrap().unwrap();
        assert_eq!(field(data, "spawn_delay"), Some(&Tag::Int(clock.delay)));
        assert_eq!(field(data, "spawn_chance"), Some(&Tag::Int(clock.chance)));
        let mut imported = CheckpointManifest::default();
        import_tags(&tags, &mut imported).unwrap();
        assert_eq!(imported.wandering_trader, Some(clock));
        let encoded = serde_json::to_vec(&imported).unwrap();
        let native: CheckpointManifest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(native.wandering_trader, Some(clock));
        checkpoint_tags(&mut tags, 42, &native).unwrap();
        let data = saved_data(&tags, path).unwrap().unwrap();
        assert_eq!(field(data, "spawn_delay"), Some(&Tag::Int(clock.delay)));
        assert_eq!(field(data, "spawn_chance"), Some(&Tag::Int(clock.chance)));
    }
}

#[test]
fn wandering_trader_only_absent_saved_fields_receive_defaults() {
    let path = "data/minecraft/wandering_trader.dat";
    let mut tags = MetadataTags::new();
    checkpoint_tags(
        &mut tags,
        42,
        &CheckpointManifest {
            world_spawn: Some([0, 80, 0]),
            wandering_trader: Some(TraderSpawner::default()),
            ..Default::default()
        },
    )
    .unwrap();
    saved_data_mut(&mut tags, path).unwrap().clear();
    let mut imported = CheckpointManifest::default();
    import_tags(&tags, &mut imported).unwrap();
    assert_eq!(imported.wandering_trader, Some(TraderSpawner::default()));
    put(
        saved_data_mut(&mut tags, path).unwrap(),
        "spawn_delay",
        Tag::Int(0),
    );
    import_tags(&tags, &mut imported).unwrap();
    assert_eq!(
        imported.wandering_trader,
        Some(TraderSpawner {
            delay: 0,
            chance: 25
        })
    );
    put(
        saved_data_mut(&mut tags, path).unwrap(),
        "spawn_chance",
        Tag::Int(0),
    );
    import_tags(&tags, &mut imported).unwrap();
    assert_eq!(
        imported.wandering_trader,
        Some(TraderSpawner {
            delay: 0,
            chance: 0
        })
    );
    put(
        saved_data_mut(&mut tags, path).unwrap(),
        "spawn_delay",
        Tag::String("invalid".into()),
    );
    assert!(import_tags(&tags, &mut imported).is_err());
}
