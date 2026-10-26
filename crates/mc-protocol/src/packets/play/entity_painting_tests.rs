use super::*;

#[test]
fn painting_variant_holder_wire_roundtrip() {
    // Local 26.1.2 EntityDataSerializers + HolderCodec javap: serializer 34,
    // registry references encode id + 1; zero introduces an inline holder.
    let packet = ClientboundSetEntityData {
        entity_id: 17,
        values: vec![EntityDataValue::PaintingVariant { index: 9, value: 4 }],
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(bytes, [17, 9, 34, 5, 255]);
    assert_eq!(ClientboundSetEntityData::decode(&mut bytes.as_slice()).unwrap(), packet);
    assert!(ClientboundSetEntityData::decode(&mut [17, 9, 34, 0, 255].as_slice()).is_err());
}
