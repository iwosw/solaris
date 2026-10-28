use super::*;
use mc_data::data_component_types_26_1_2 as ids;
use mc_nbt::{ListTag, tag_type};

fn item(name: &str, count: i32) -> Tag {
    Tag::Compound(vec![
        ("id".into(), Tag::String(name.into())),
        ("count".into(), Tag::Int(count)),
        (
            "components".into(),
            Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(7))]),
        ),
    ])
}

fn list(values: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: values.first().map_or(tag_type::END, Tag::type_id),
        elements: values,
    })
}

fn arrow() -> i32 {
    copied_components::items()
        .id_of(&Identifier::parse("minecraft:arrow").unwrap())
        .unwrap() as i32
}

fn template_bytes(count: i32) -> Vec<u8> {
    // Unlike the outer stack, the template's item comes first.
    let mut bytes = Vec::new();
    for value in [arrow(), count, 1, 0, ids::DAMAGE, 7] {
        bytes.write_varint(value);
    }
    bytes
}

fn fixture(name: &str, id: i32, value: Tag, payload: Vec<u8>) {
    let items = copied_components::items();
    let original = mc_data::item_nbt::stack_from_component_patch(
        items,
        &Identifier::parse("minecraft:stone").unwrap(),
        &[(name.into(), value)],
    )
    .unwrap();
    let durable = mc_data::item_nbt::stack_to_fields(items, &original).unwrap();
    let original = mc_data::item_nbt::stack_from_fields(items, &durable)
        .unwrap()
        .unwrap();
    let mut expected = Vec::new();
    for value in [1, original.item_id as i32, 1, 0, id] {
        expected.write_varint(value);
    }
    expected.extend(payload);
    let mut encoded = Vec::new();
    original.encode(&mut encoded).unwrap();
    assert_eq!(encoded, expected, "{name}: exact vanilla template framing");
    let mut cursor = expected.as_slice();
    let decoded = ItemStack::decode(&mut cursor).unwrap();
    assert!(cursor.is_empty());
    let mut reencoded = Vec::new();
    decoded.encode(&mut reencoded).unwrap();
    assert_eq!(reencoded, expected);
    for end in 0..expected.len() {
        assert!(
            ItemStack::decode(&mut &expected[..end]).is_err(),
            "{name}: truncated at {end}"
        );
    }
}

#[test]
fn template_wire_charged_projectiles_is_list_of_item_count_patch() {
    let mut payload = vec![1];
    payload.extend(template_bytes(1));
    fixture(
        "minecraft:charged_projectiles",
        ids::CHARGED_PROJECTILES,
        list(vec![item("minecraft:arrow", 1)]),
        payload,
    );
}

#[test]
fn template_wire_bundle_contents_is_list_of_item_count_patch() {
    let mut payload = vec![2];
    payload.extend(template_bytes(3));
    payload.extend(template_bytes(2));
    fixture(
        "minecraft:bundle_contents",
        ids::BUNDLE_CONTENTS,
        list(vec![item("minecraft:arrow", 3), item("minecraft:arrow", 2)]),
        payload,
    );
}

#[test]
fn template_wire_use_remainder_is_one_item_count_patch() {
    fixture(
        "minecraft:use_remainder",
        ids::USE_REMAINDER,
        item("minecraft:arrow", 1),
        template_bytes(1),
    );
}

#[test]
fn template_wire_container_is_list_of_boolean_optional_templates() {
    let mut payload = vec![3, 0, 0, 1];
    payload.extend(template_bytes(2));
    fixture(
        "minecraft:container",
        ids::CONTAINER,
        list(vec![Tag::Compound(vec![
            ("slot".into(), Tag::Int(2)),
            ("item".into(), item("minecraft:arrow", 2)),
        ])]),
        payload,
    );
}

#[test]
fn template_wire_remainder_and_bundle_accept_item_name_shorthand() {
    let mut bare = Vec::new();
    for value in [arrow(), 1, 0, 0] {
        bare.write_varint(value);
    }
    fixture(
        "minecraft:use_remainder",
        ids::USE_REMAINDER,
        Tag::String("minecraft:arrow".into()),
        bare.clone(),
    );
    let mut payload = vec![1];
    payload.extend(bare);
    fixture(
        "minecraft:bundle_contents",
        ids::BUNDLE_CONTENTS,
        list(vec![Tag::String("minecraft:arrow".into())]),
        payload,
    );
}

#[test]
fn template_wire_rejects_empty_air_and_invalid_counts_without_clamping() {
    for stack in [
        ItemStack::EMPTY,
        ItemStack::new(0, 1),
        ItemStack::new(arrow() as u32, -1),
        ItemStack::new(arrow() as u32, 100),
    ] {
        assert!(item_stack_template::encode(&mut Vec::new(), &stack, 0).is_err());
        let mut bytes = Vec::new();
        bytes.write_varint(stack.item_id as i32);
        bytes.write_varint(stack.count);
        bytes.extend([0, 0]);
        assert!(item_stack_template::decode(&mut bytes.as_slice(), 0).is_err());
    }
}

#[test]
fn template_wire_preserves_depth_refusal_before_child_payload() {
    for id in [ids::USE_REMAINDER, ids::BUNDLE_CONTENTS, ids::CONTAINER] {
        let mut bytes = if id == ids::CONTAINER {
            vec![1, 1]
        } else if id == ids::BUNDLE_CONTENTS {
            vec![1]
        } else {
            Vec::new()
        };
        bytes.extend(template_bytes(1));
        assert!(matches!(
            copied_components::decode(
                &mut bytes.as_slice(),
                id,
                copied_components::MAX_ITEM_COMPONENT_DEPTH
            ),
            Err(CodecError::NotSupported("nested item component depth"))
        ));
    }
}

#[test]
fn template_wire_command_admission_rejects_empty_counts_air_and_excess_depth() {
    let items = copied_components::items();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    for name in ["minecraft:use_remainder", "minecraft:bundle_contents"] {
        for invalid in [
            item("minecraft:arrow", 0),
            item("minecraft:arrow", -1),
            item("minecraft:arrow", 100),
            item("minecraft:air", 1),
            Tag::String("minecraft:air".into()),
        ] {
            let value = if name == "minecraft:bundle_contents" {
                list(vec![invalid])
            } else {
                invalid
            };
            assert!(
                mc_data::item_nbt::stack_from_component_patch(
                    items,
                    &stone,
                    &[(name.into(), value)]
                )
                .is_err()
            );
        }
    }
    let mut nested = Tag::String("minecraft:arrow".into());
    for _ in 0..=8 {
        nested = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:arrow".into())),
            (
                "components".into(),
                Tag::Compound(vec![("minecraft:use_remainder".into(), nested)]),
            ),
        ]);
    }
    assert!(
        mc_data::item_nbt::stack_from_component_patch(
            items,
            &stone,
            &[("minecraft:use_remainder".into(), nested)]
        )
        .is_err()
    );
}
