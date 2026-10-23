use super::*;

#[test]
fn custom_mob_export_uses_presentation_and_retains_native_marker() {
    let directory = tempfile::tempdir().unwrap();
    let mut entities = [mc_world::native::NativeEntitySnapshot {
        identity: [7; 16],
        dimension: 0,
        position: ChunkPos { x: 0, z: 0 },
        generated_slot: None,
        journal_lsn: 1,
        payload: Some(Tag::Compound(vec![
            ("id".into(), Tag::String("sky-creeper:sky_creeper".into())),
            (
                "solaris:Presentation".into(),
                Tag::String("minecraft:creeper".into()),
            ),
        ])),
    }];
    let mut stats = ConversionStats::default();
    export_entities(directory.path(), 0, &mut entities, &mut stats).unwrap();
    assert_eq!(stats.entities, 1);
    let payload = mc_world::anvil::read_chunk(directory.path().join("r.0.0.mca"), 0, 0)
        .unwrap()
        .unwrap();
    let (_, Tag::Compound(root)) =
        mc_nbt::read_named(&mut payload.uncompressed_nbt.as_slice()).unwrap()
    else {
        panic!("entity region root");
    };
    let Tag::List(list) = &root.iter().find(|(key, _)| key == "Entities").unwrap().1 else {
        panic!("entity list");
    };
    let Tag::Compound(entity) = &list.elements[0] else {
        panic!("entity record");
    };
    assert!(entity.contains(&("id".into(), Tag::String("minecraft:creeper".into()))));
    assert!(entity.contains(&(
        "solaris:MobBlueprint".into(),
        Tag::String("sky-creeper:sky_creeper".into())
    )));
    assert!(
        !entity.iter().any(|(key, _)| key == "solaris:Presentation"),
        "internal export marker must not survive a vanilla Anvil round trip"
    );
}
