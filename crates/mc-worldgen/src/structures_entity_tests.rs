use super::*;

#[test]
fn authored_monster_persistence_is_preserved_and_malformed_flags_rejected() {
    for flag in [
        None,
        Some(Tag::Byte(0)),
        Some(Tag::Byte(1)),
        Some(Tag::Int(1)),
    ] {
        let mut fields = vec![("id".into(), Tag::String("minecraft:piglin_brute".into()))];
        if let Some(value) = flag.clone() {
            fields.push(("PersistenceRequired".into(), value));
        }
        let root = vec![(
            "entities".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![("nbt".into(), Tag::Compound(fields))])],
            }),
        )];
        let parsed = parse_entities("test:bastion", &root);
        if matches!(flag, Some(Tag::Int(_))) {
            assert!(parsed.is_err());
        } else {
            let entities = parsed.unwrap();
            assert_eq!(entities.len(), 1);
            assert_eq!(entities[0].entity_type, "minecraft:piglin_brute");
            assert_eq!(entities[0].persistent, matches!(flag, Some(Tag::Byte(1))));
            assert!(entities[0].villager.is_none());
        }
    }
}
