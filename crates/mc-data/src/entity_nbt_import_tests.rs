use super::*;

#[test]
fn vanilla_unassigned_wire_ids_stay_absent_until_native_import_finalizes_them() {
    let items = crate::items::solaris_required_items();
    let mut entity = VanillaEntity {
        id: "minecraft:cow".into(),
        uuid: uuid::Uuid::from_u128(1),
        ..Default::default()
    };
    let fields = entity_to_fields(&items, &entity).unwrap();
    assert!(field(&fields, "solaris:WireId").is_none());
    assert_eq!(
        entity_from_fields(&items, Default::default(), &fields)
            .unwrap()
            .wire_id,
        0
    );
    entity.wire_id = 123;
    let fields = entity_to_fields(&items, &entity).unwrap();
    assert_eq!(field(&fields, "solaris:WireId"), Some(&Tag::Int(123)));
    assert_eq!(
        entity_from_fields(&items, Default::default(), &fields)
            .unwrap()
            .wire_id,
        123
    );
}

#[test]
fn present_native_wire_id_requires_a_nonnegative_int() {
    let items = crate::items::solaris_required_items();
    for value in [
        Tag::Int(-1),
        Tag::Long(1),
        Tag::String("1".into()),
        Tag::Byte(1),
    ] {
        let error = entity_from_fields(
            &items,
            Default::default(),
            &[("solaris:WireId".into(), value)],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("solaris:WireId: expected nonnegative int"));
    }
    let entity = VanillaEntity {
        wire_id: -1,
        ..Default::default()
    };
    assert!(
        entity_to_fields(&items, &entity)
            .unwrap_err()
            .to_string()
            .contains("solaris:WireId")
    );
}

#[test]
fn accepted_numeric_health_never_silently_defaults_during_import() {
    let items = crate::items::solaris_required_items();
    for value in [
        Tag::Float(5.0),
        Tag::Double(5.0),
        Tag::Byte(5),
        Tag::Short(5),
        Tag::Int(5),
        Tag::Long(5),
    ] {
        let entity =
            entity_from_fields(&items, Default::default(), &[("Health".into(), value)]).unwrap();
        assert_eq!(entity.health, 5.0);
    }
}
