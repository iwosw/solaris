use super::*;
use bytes::BytesMut;

#[test]
fn copper_metadata_has_exact_26_1_2_serializer_ids_and_ordinals() {
    let packet = ClientboundSetEntityData {
        entity_id: 28,
        values: vec![
            EntityDataValue::WeatheringCopper {
                index: 16,
                value: 3,
            },
            EntityDataValue::CopperGolemState {
                index: 17,
                value: 4,
            },
        ],
    };
    let mut bytes = BytesMut::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(&bytes[..], &[28, 16, 38, 3, 17, 37, 4, 255]);
    assert_eq!(
        ClientboundSetEntityData::decode(&mut bytes.freeze()).unwrap(),
        packet
    );
}

#[test]
fn copper_metadata_rejects_invalid_weather_and_state_ordinals() {
    for bytes in [[28, 16, 38, 4, 255], [28, 17, 37, 5, 255]] {
        assert!(ClientboundSetEntityData::decode(&mut &bytes[..]).is_err());
    }
}
