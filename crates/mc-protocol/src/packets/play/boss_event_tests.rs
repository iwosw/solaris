use super::*;
#[test]
fn boss_uuid_operation_and_progress_wire_are_exact() {
    assert_eq!(ClientboundBossEvent::ID, 9);
    let packet = ClientboundBossEvent {
        uuid: Uuid::from_u128(0x0102030405060708090a0b0c0d0e0f10),
        operation: BossOperation::Progress(0.5),
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(&bytes[..16], packet.uuid.as_bytes());
    assert_eq!(bytes[16], 2);
    assert_eq!(&bytes[17..], &[0x3f, 0, 0, 0]);
    assert_eq!(
        ClientboundBossEvent::decode(&mut bytes.as_slice()).unwrap(),
        packet
    );
}
#[test]
fn all_six_boss_operations_round_trip_without_trailing_fields() {
    for operation in [
        BossOperation::Add {
            name: Tag::Compound(vec![(
                "translate".into(),
                Tag::String("entity.minecraft.wither".into()),
            )]),
            progress: 0.75,
            color: 5,
            overlay: 0,
            flags: 1,
        },
        BossOperation::Remove,
        BossOperation::Progress(0.5),
        BossOperation::Name(Tag::String("Wither".into())),
        BossOperation::Style {
            color: 5,
            overlay: 0,
        },
        BossOperation::Properties(7),
    ] {
        let packet = ClientboundBossEvent {
            uuid: Uuid::nil(),
            operation,
        };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        let mut input = bytes.as_slice();
        assert_eq!(ClientboundBossEvent::decode(&mut input).unwrap(), packet);
        assert!(input.is_empty());
    }
}
