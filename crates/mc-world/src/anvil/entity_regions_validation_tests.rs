use super::*;

fn record() -> Tag {
    chunk_to_tag(
        &mc_data::items::solaris_required_items(),
        &EntityChunk {
            x: 1,
            z: -2,
            entities: vec![VanillaEntity {
                id: "minecraft:cow".into(),
                uuid: uuid::Uuid::from_u128(7),
                pos: [16.5, 64.0, -31.5],
                health: 5.0,
                ..Default::default()
            }],
        },
    )
    .unwrap()
}

fn fields(tag: &mut Tag) -> &mut Vec<(String, Tag)> {
    let Tag::Compound(root) = tag else {
        unreachable!()
    };
    let Tag::List(entities) = &mut root
        .iter_mut()
        .find(|(name, _)| name == "Entities")
        .unwrap()
        .1
    else {
        unreachable!()
    };
    let Tag::Compound(fields) = &mut entities.elements[0] else {
        unreachable!()
    };
    fields
}

#[test]
fn malformed_import_health_and_wire_ids_fail_with_complete_record_context() {
    let path = Path::new("world/DIM1/entities/r.0.-1.mca");
    for (key, value) in [
        ("Health", Tag::Float(f32::NAN)),
        ("Health", Tag::Double(1e300)),
        ("Health", Tag::String("5".into())),
        ("solaris:WireId", Tag::Int(-1)),
        ("solaris:WireId", Tag::Long(5)),
    ] {
        let mut tag = record();
        let entries = fields(&mut tag);
        entries.retain(|(name, _)| name != key);
        entries.push((key.into(), value));
        let error = entity_chunk_from_nbt(&mc_data::items::solaris_required_items(), &tag, path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("world/DIM1/entities/r.0.-1.mca"));
        assert!(error.contains("chunk (1, -2) entity-index 0"));
        assert!(error.contains(&uuid::Uuid::from_u128(7).to_string()));
        assert!(error.contains("type-id "));
        assert!(error.contains("(minecraft:cow)"));
        assert!(error.contains(key));
    }
}

#[test]
fn missing_required_native_identity_or_position_is_never_defaulted() {
    for key in ["id", "UUID", "Pos"] {
        let mut tag = record();
        fields(&mut tag).retain(|(name, _)| name != key);
        let error = entity_chunk_from_nbt(
            &mc_data::items::solaris_required_items(),
            &tag,
            Path::new("world/entities/r.0.-1.mca"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(&format!("{key}: missing required saved-entity field")));
        assert!(error.contains("entity-index 0"));
    }
}
