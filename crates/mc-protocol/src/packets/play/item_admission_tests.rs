use super::*;
use mc_data::item_stack::ExtendedItemComponents;
use mc_nbt::{ListTag, tag_type};

fn varints(values: &[i32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for &value in values {
        bytes.write_varint(value);
    }
    bytes
}

#[test]
fn item_admission_rejects_negative_and_unknown_outer_template_and_merchant_ids() {
    for raw in [-1, i32::MIN, i32::MAX] {
        let outer = varints(&[1, raw, 0, 0]);
        assert!(matches!(
            ItemStack::decode(&mut outer.as_slice()),
            Err(CodecError::NotSupported(_))
        ));
        let template = varints(&[raw, 1, 0, 0]);
        assert!(matches!(
            item_stack_template::decode(&mut template.as_slice(), 0),
            Err(CodecError::NotSupported(_))
        ));
        let merchant = varints(&[0, 1, raw, 1, 0, 0]);
        assert!(matches!(
            ClientboundMerchantOffers::decode(&mut merchant.as_slice()),
            Err(CodecError::NotSupported(_))
        ));
    }
    let valid = varints(&[1, 896, 0, 0]);
    assert_eq!(
        ItemStack::decode(&mut valid.as_slice()).unwrap(),
        ItemStack::new(896, 1)
    );
}

#[test]
fn item_admission_creative_packet_rejects_unknown_stack_before_boundary() {
    for raw in [-1, i32::MIN, i32::MAX] {
        let mut bytes = Vec::new();
        bytes.write_i16(36);
        bytes.extend(varints(&[1, raw, 0, 0]));
        assert!(matches!(
            ServerboundSetCreativeModeSlot::decode(&mut bytes.as_slice()),
            Err(CodecError::NotSupported(_))
        ));
    }
}

#[test]
fn item_admission_holder_minimum_varint_returns_codec_error_without_overflow() {
    let trim = varints(&[i32::MIN]);
    let mut components = ExtendedItemComponents::default();
    assert!(matches!(
        station_components::decode(&mut trim.as_slice(), 56, &mut components, 0),
        Err(CodecError::NotSupported("unknown station registry holder"))
    ));
    let banner = varints(&[1, i32::MIN, 0]);
    assert!(matches!(
        station_components::decode(&mut banner.as_slice(), 72, &mut components, 0),
        Err(CodecError::NotSupported("unknown station registry holder"))
    ));
    assert!(matches!(
        copied_components::decode(&mut banner.as_slice(), 72, 0),
        Err(CodecError::NotSupported(_))
    ));
}

fn bundle(count: usize) -> ItemStack {
    let value = Tag::List(ListTag {
        element_type: tag_type::STRING,
        elements: vec![Tag::String("minecraft:arrow".into()); count],
    });
    mc_data::item_nbt::stack_from_component_patch(
        copied_components::items(),
        &Identifier::parse("minecraft:stone").unwrap(),
        &[("minecraft:bundle_contents".into(), value)],
    )
    .unwrap()
}

#[test]
fn item_admission_bundle_sixty_four_roundtrips_and_sixty_five_encode_fails() {
    let mut bytes = Vec::new();
    bundle(64).encode(&mut bytes).unwrap();
    let mut cursor = bytes.as_slice();
    let decoded = ItemStack::decode(&mut cursor).unwrap();
    assert!(cursor.is_empty());
    let mut encoded = Vec::new();
    decoded.encode(&mut encoded).unwrap();
    assert_eq!(encoded, bytes);
    assert!(matches!(
        bundle(65).encode(&mut Vec::new()),
        Err(CodecError::StringTooLong { len: 65, max: 64 })
    ));
}

#[test]
fn item_admission_bundle_rejects_oversized_count_before_entries() {
    for count in [65, i32::MAX] {
        let bytes = varints(&[1, 896, 1, 0, 50, count]);
        assert!(matches!(
            ItemStack::decode(&mut bytes.as_slice()),
            Err(CodecError::StringTooLong { max: 64, .. })
        ));
    }
}
