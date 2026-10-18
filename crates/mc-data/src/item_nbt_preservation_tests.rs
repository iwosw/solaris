use super::*;
use mc_nbt::{ListTag, tag_type};

fn rich_stack_fields() -> Vec<(String, Tag)> {
    vec![
        ("id".into(), Tag::String("minecraft:diamond_sword".into())),
        ("count".into(), Tag::Int(7)),
        (
            "components".into(),
            Tag::Compound(vec![
                (
                    "minecraft:custom_name".into(),
                    Tag::Compound(vec![
                        ("text".into(), Tag::String("Village heirloom".into())),
                        ("color".into(), Tag::String("gold".into())),
                        ("bold".into(), Tag::Byte(1)),
                        (
                            "extra".into(),
                            Tag::List(ListTag {
                                element_type: tag_type::COMPOUND,
                                elements: vec![Tag::Compound(vec![
                                    ("text".into(), Tag::String("!".into())),
                                    ("italic".into(), Tag::Byte(0)),
                                ])],
                            }),
                        ),
                    ]),
                ),
                (
                    "minecraft:lore".into(),
                    Tag::List(ListTag {
                        element_type: tag_type::COMPOUND,
                        elements: vec![Tag::Compound(vec![
                            (
                                "translate".into(),
                                Tag::String("item.minecraft.diamond_sword".into()),
                            ),
                            ("color".into(), Tag::String("gray".into())),
                        ])],
                    }),
                ),
                ("minecraft:enchantment_glint_override".into(), Tag::Byte(1)),
                ("minecraft:damage".into(), Tag::Int(9)),
            ]),
        ),
    ]
}

fn assert_components_eq(left: &[(String, Tag)], right: &[(String, Tag)]) {
    let Some(Tag::Compound(left)) = field(left, "components") else {
        panic!("left components absent");
    };
    let Some(Tag::Compound(right)) = field(right, "components") else {
        panic!("right components absent");
    };
    assert_eq!(left.len(), right.len());
    for (name, value) in left {
        assert_eq!(Some(value), field(right, name), "component {name}");
    }
}

#[test]
fn rich_vanilla_components_survive_count_change_and_durable_serde() {
    let items = crate::items::solaris_required_items();
    let original = rich_stack_fields();
    let mut stack = stack_from_fields(&items, &original).unwrap().unwrap();
    assert_eq!(stack.custom_name.as_deref(), Some("Village heirloom"));
    stack.count = 3;
    let image = serde_json::to_vec(&stack).unwrap();
    let restored: ItemStack = serde_json::from_slice(&image).unwrap();
    let encoded = stack_to_fields(&items, &restored).unwrap();
    assert_eq!(int_field(&encoded, "count"), Some(3));
    assert_components_eq(&original, &encoded);
}

#[test]
fn typed_edits_override_original_projection_without_dropping_other_components() {
    let items = crate::items::solaris_required_items();
    let original = rich_stack_fields();
    let mut stack = stack_from_fields(&items, &original).unwrap().unwrap();
    stack.damage = Some(12);
    stack.custom_name = Some("Renamed".into());
    let encoded = stack_to_fields(&items, &stack).unwrap();
    let Some(Tag::Compound(components)) = field(&encoded, "components") else {
        panic!("components absent");
    };
    assert_eq!(field(components, "minecraft:damage"), Some(&Tag::Int(12)));
    assert_eq!(custom_name_component(&encoded).as_deref(), Some("Renamed"));
    assert!(field(components, "minecraft:lore").is_some());
    assert!(field(components, "minecraft:enchantment_glint_override").is_some());
}

#[test]
fn nested_charged_item_keeps_rich_components_when_child_count_changes() {
    let items = crate::items::solaris_required_items();
    let child = rich_stack_fields();
    let original = vec![
        ("id".into(), Tag::String("minecraft:crossbow".into())),
        ("count".into(), Tag::Int(1)),
        (
            "components".into(),
            Tag::Compound(vec![(
                "minecraft:charged_projectiles".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![Tag::Compound(child.clone())],
                }),
            )]),
        ),
    ];
    let mut stack = stack_from_fields(&items, &original).unwrap().unwrap();
    stack.extended_mut().charged_projectiles[0].count = 2;
    let encoded = stack_to_fields(&items, &stack).unwrap();
    let Some(Tag::Compound(components)) = field(&encoded, "components") else {
        panic!("components absent");
    };
    let Some(Tag::List(projectiles)) = field(components, "minecraft:charged_projectiles") else {
        panic!("projectiles absent");
    };
    let Tag::Compound(actual) = &projectiles.elements[0] else {
        panic!("charged item absent");
    };
    assert_eq!(int_field(actual, "count"), Some(2));
    assert_components_eq(&child, actual);
}

#[test]
fn every_typed_setter_supersedes_its_preserved_entry_at_the_same_baseline() {
    use crate::item_stack::PreservedItemComponent;
    let items = crate::items::solaris_required_items();
    let model = Identifier::parse("minecraft:diamond_sword").unwrap();
    let sharpness = Identifier::parse("minecraft:sharpness").unwrap();
    let mut custom_data = Vec::new();
    mc_nbt::write_named(
        &mut custom_data,
        "",
        &Tag::Compound(vec![("owner".into(), Tag::Int(1))]),
    )
    .unwrap();
    let template = ItemStack::new(items.id_of(&model).unwrap(), 1)
        .with_damage(9)
        .with_enchantment(sharpness.clone(), 4)
        .with_custom_name("Village heirloom")
        .with_item_model(model.clone())
        .with_max_damage(1561)
        .unwrap()
        .with_custom_data(&custom_data)
        .unwrap();
    let projected = stack_to_fields(&items, &template).unwrap();
    let Some(Tag::Compound(projected_components)) = field(&projected, "components") else {
        panic!("typed components");
    };
    let original = rich_stack_fields();
    let Some(Tag::Compound(original_components)) = field(&original, "components") else {
        panic!("rich components");
    };
    let lore = field(original_components, "minecraft:lore").unwrap();
    for name in [
        "minecraft:damage",
        "minecraft:enchantments",
        "minecraft:custom_name",
        "minecraft:item_model",
        "minecraft:max_damage",
        "minecraft:custom_data",
    ] {
        let baseline = field(projected_components, name).unwrap();
        let retained = if name == "minecraft:custom_name" {
            field(original_components, name).unwrap()
        } else {
            baseline
        };
        let mut stack = template.clone();
        stack.extended_mut().preserved_components = vec![
            PreservedItemComponent {
                name: name.into(),
                value: component_bytes(retained).unwrap(),
                baseline: Some(component_bytes(baseline).unwrap()),
            },
            PreservedItemComponent {
                name: "minecraft:lore".into(),
                value: component_bytes(lore).unwrap(),
                baseline: None,
            },
        ];
        let edited = match name {
            "minecraft:damage" => stack.with_damage(9),
            "minecraft:enchantments" => stack.with_enchantment(sharpness.clone(), 4),
            "minecraft:custom_name" => stack.with_custom_name("Village heirloom"),
            "minecraft:item_model" => stack.with_item_model(model.clone()),
            "minecraft:max_damage" => stack.with_max_damage(1561).unwrap(),
            "minecraft:custom_data" => stack.with_custom_data(&custom_data).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(edited.extended().preserved_components.len(), 1, "{name}");
        assert_eq!(
            edited.extended().preserved_components[0].name,
            "minecraft:lore"
        );
        let durable = serde_json::to_vec(&edited).unwrap();
        let edited: ItemStack = serde_json::from_slice(&durable).unwrap();
        let encoded = stack_to_fields(&items, &edited).unwrap();
        let Some(Tag::Compound(components)) = field(&encoded, "components") else {
            panic!("edited components");
        };
        assert_eq!(field(components, name), Some(baseline), "{name}");
        assert_eq!(field(components, "minecraft:lore"), Some(lore), "{name}");
    }
}

#[test]
fn editing_bare_typed_fields_does_not_allocate_an_opaque_component_carrier() {
    let stack = ItemStack::new(1, 1)
        .with_damage(9)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 4)
        .with_custom_name("Blade")
        .with_item_model(Identifier::parse("minecraft:diamond_sword").unwrap());
    assert!(stack.components.is_none());
}

#[test]
fn removed_component_native_nbt_rejects_corrupt_marker_with_exact_diagnostic() {
    let items = crate::items::solaris_required_items();
    for (name, value) in [
        ("!minecraft:custom_name", Tag::String("bad marker".into())),
        ("!minecraft:not_registered", Tag::Compound(Vec::new())),
    ] {
        let fields = vec![
            ("id".into(), Tag::String("minecraft:stone".into())),
            (
                "components".into(),
                Tag::Compound(vec![(name.into(), value)]),
            ),
        ];
        assert_eq!(
            stack_from_fields(&items, &fields).unwrap_err(),
            ItemNbtError::InvalidComponent(format!("invalid removed component {name}"))
        );
    }
}

#[test]
fn native_removed_and_added_component_collision_is_corrupt_not_last_writer_wins() {
    let items = crate::items::solaris_required_items();
    let fields = vec![
        ("id".into(), Tag::String("minecraft:stone".into())),
        (
            "components".into(),
            Tag::Compound(vec![
                ("minecraft:custom_name".into(), Tag::String("Name".into())),
                ("!minecraft:custom_name".into(), Tag::Compound(Vec::new())),
            ]),
        ),
    ];
    assert_eq!(
        stack_from_fields(&items, &fields).unwrap_err(),
        ItemNbtError::InvalidComponent("duplicate item component minecraft:custom_name".into())
    );
}
