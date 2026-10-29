use super::*;
use bytes::{BufMut, Bytes};
use mc_nbt::{ListTag, Tag};

fn compound(entries: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

fn sample_dialog() -> Tag {
    compound(vec![
        ("type", Tag::String("minecraft:multi_action".into())),
        ("title", Tag::String("Settlement".into())),
        (
            "inputs",
            Tag::List(ListTag {
                element_type: 10,
                elements: vec![compound(vec![
                    ("type", Tag::String("minecraft:number_range".into())),
                    ("key", Tag::String("amount".into())),
                ])],
            }),
        ),
    ])
}

#[test]
fn ids_match_local_packets_report() {
    assert_eq!(ClientboundClearDialog::ID, 142);
    assert_eq!(ClientboundShowDialog::ID, 143);
    assert_eq!(ServerboundCustomClickAction::ID, 68);
}

#[test]
fn show_dialog_is_inline_holder_then_network_nbt() {
    let packet = ClientboundShowDialog {
        dialog: sample_dialog(),
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    // Holder direct id 0, then TAG_Compound without a root name.
    assert_eq!(&bytes[..2], &[0, 10]);
    let mut input = bytes.as_slice();
    assert_eq!(ClientboundShowDialog::decode(&mut input).unwrap(), packet);
    assert!(input.is_empty());
}

#[test]
fn show_dialog_rejects_registry_reference() {
    let mut bytes = Vec::new();
    bytes.put_u8(1);
    bytes.put_u8(10);
    bytes.put_u8(0);
    assert!(ClientboundShowDialog::decode(&mut bytes.as_slice()).is_err());
}

#[test]
fn clear_dialog_has_empty_body() {
    let mut bytes = Vec::new();
    ClientboundClearDialog.encode(&mut bytes).unwrap();
    assert!(bytes.is_empty());
    assert_eq!(
        ClientboundClearDialog::decode(&mut Bytes::new()).unwrap(),
        ClientboundClearDialog
    );
}

#[test]
fn custom_click_without_payload_is_id_then_length_one_tag_end() {
    let packet = ServerboundCustomClickAction {
        id: Identifier::parse("solaris:ok").unwrap(),
        payload: None,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    // string "solaris:ok" (len 10), VarInt payload length 1, TAG_End.
    let mut expected = vec![10];
    expected.extend_from_slice(b"solaris:ok");
    expected.extend_from_slice(&[1, 0]);
    assert_eq!(bytes, expected);
    assert_eq!(
        ServerboundCustomClickAction::decode(&mut bytes.as_slice()).unwrap(),
        packet
    );
}

#[test]
fn custom_click_with_inputs_round_trips_vanilla_value_types() {
    // Vanilla input values: text -> String, boolean -> Byte, number_range -> Float.
    let packet = ServerboundCustomClickAction {
        id: Identifier::parse("solaris:settlement/found").unwrap(),
        payload: Some(compound(vec![
            ("name", Tag::String("Ёлки".into())),
            ("walls", Tag::Byte(1)),
            ("radius", Tag::Float(12.5)),
        ])),
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut input = bytes.as_slice();
    assert_eq!(
        ServerboundCustomClickAction::decode(&mut input).unwrap(),
        packet
    );
    assert!(input.is_empty());
}

#[test]
fn custom_click_payload_may_be_a_non_compound_tag() {
    let packet = ServerboundCustomClickAction {
        id: Identifier::parse("solaris:x").unwrap(),
        payload: Some(Tag::String("yes".into())),
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(
        ServerboundCustomClickAction::decode(&mut bytes.as_slice()).unwrap(),
        packet
    );
}

fn raw_click(id: &str, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.write_string(id, 32767).unwrap();
    bytes.write_varint(payload.len() as i32);
    bytes.put_slice(payload);
    bytes
}

#[test]
fn custom_click_rejects_malformed_and_over_budget_payloads() {
    // Declared length past the buffer.
    let mut truncated = raw_click("a:b", &[0]);
    truncated[4] = 9;
    assert!(ServerboundCustomClickAction::decode(&mut truncated.as_slice()).is_err());
    // Length prefix above the 65536 ceiling.
    let mut huge = Vec::new();
    huge.write_string("a:b", 32767).unwrap();
    huge.write_varint(65537);
    assert!(ServerboundCustomClickAction::decode(&mut huge.as_slice()).is_err());
    // Invalid identifier.
    assert!(ServerboundCustomClickAction::decode(&mut raw_click("A B", &[0]).as_slice()).is_err());
    // 17 nested compounds exceed NbtAccounter depth 16.
    let mut nested = Tag::Compound(Vec::new());
    for _ in 0..16 {
        nested = Tag::Compound(vec![("c".into(), nested)]);
    }
    let mut payload = Vec::new();
    mc_nbt::write_network_value(&mut payload, &nested).unwrap();
    assert!(
        ServerboundCustomClickAction::decode(&mut raw_click("a:b", &payload).as_slice()).is_err()
    );
    // 16 levels is accepted.
    let mut ok = Tag::Compound(Vec::new());
    for _ in 0..15 {
        ok = Tag::Compound(vec![("c".into(), ok)]);
    }
    let mut payload = Vec::new();
    mc_nbt::write_network_value(&mut payload, &ok).unwrap();
    assert!(
        ServerboundCustomClickAction::decode(&mut raw_click("a:b", &payload).as_slice()).is_ok()
    );
}
