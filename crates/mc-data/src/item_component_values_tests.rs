use super::*;
use crate::ItemStack;
use crate::item_nbt::{stack_from_component_patch, stack_from_fields, stack_to_fields};

#[test]
fn new_26_3_components_survive_command_admission_and_durable_inventory_storage() {
    let items = crate::items::solaris_required_items();
    let text = |value: &str| Tag::String(value.into());
    let fields = |values: Vec<(&str, Tag)>| {
        Tag::Compound(values.into_iter().map(|(k, v)| (k.into(), v)).collect())
    };
    let lines = Tag::List(mc_nbt::ListTag {
        element_type: mc_nbt::tag_type::STRING,
        elements: vec![text("A"); 4],
    });
    for (name, value) in [
        ("attack_animation", fields(vec![])),
        ("interact_animation", fields(vec![("type", text("stab"))])),
        ("block_transformer", text("minecraft:axe")),
        ("villager_food", fields(vec![("nutrition", Tag::Int(12))])),
        ("sulfur_cube_content", text("minecraft:stone")),
        ("compostable", fields(vec![("layers", Tag::Int(1))])),
        (
            "cooking_fuel",
            fields(vec![
                ("burn_time", Tag::Int(200)),
                ("speed_multiplier", Tag::Float(1.0)),
            ]),
        ),
        (
            "brewing_fuel",
            fields(vec![
                ("uses", Tag::Int(20)),
                ("speed_multiplier", Tag::Float(1.0)),
            ]),
        ),
        (
            "mob_visibility",
            fields(vec![
                ("targeting_entity_types", text("#minecraft:skeletons")),
                ("visibility", Tag::Float(0.5)),
            ]),
        ),
        ("provides_pottery_pattern", text("minecraft:angler")),
        ("sign_text_front", fields(vec![("messages", lines.clone())])),
        ("sign_text_back", fields(vec![("messages", lines)])),
        ("waxed", fields(vec![])),
        ("cushion/color", text("red")),
    ] {
        let name = format!("minecraft:{name}");
        let stack = stack_from_component_patch(
            &items,
            &Identifier::parse("minecraft:stone").unwrap(),
            &[(name.clone(), value.clone())],
        )
        .unwrap();
        let image = serde_json::to_vec(&stack).unwrap();
        let restored: ItemStack = serde_json::from_slice(&image).unwrap();
        let nbt = stack_to_fields(&items, &restored).unwrap();
        let restored = stack_from_fields(&items, &nbt).unwrap().unwrap();
        let nbt = stack_to_fields(&items, &restored).unwrap();
        let Some(Tag::Compound(components)) = field(&nbt, "components") else {
            panic!("missing {name}")
        };
        assert_eq!(field(components, &name), Some(&value), "{name}");
    }
}

#[test]
fn new_26_3_component_classes_reject_invalid_command_shapes() {
    let items = crate::items::solaris_required_items();
    for (name, value) in [
        ("waxed", Tag::Byte(1)),
        ("cushion/color", Tag::String("infrared".into())),
        (
            "attack_animation",
            Tag::Compound(vec![("duration".into(), Tag::Int(-1))]),
        ),
        (
            "villager_food",
            Tag::Compound(vec![("nutrition".into(), Tag::Int(0))]),
        ),
        (
            "compostable",
            Tag::Compound(vec![("layers".into(), Tag::Compound(vec![]))]),
        ),
        (
            "sign_text_front",
            Tag::Compound(vec![(
                "messages".into(),
                Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::STRING,
                    elements: vec![Tag::String("one".into())],
                }),
            )]),
        ),
        (
            "pot_decorations",
            Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::STRING,
                elements: vec![Tag::String("minecraft:brick".into())],
            }),
        ),
    ] {
        assert!(
            validate(&items, &[(format!("minecraft:{name}"), value)], 0).is_err(),
            "{name}"
        );
    }
}
