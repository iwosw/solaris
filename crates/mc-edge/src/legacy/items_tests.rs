use super::*;

fn tables(names: &[&str]) -> Tables {
    let names: Vec<String> = names.iter().map(|name| (*name).into()).collect();
    Tables {
        items: IdMap::by_name(
            [("stick", 10), ("brick", 11)],
            [("stick", 3), ("brick", 4)],
            |_| None,
        ),
        legacy_brick: 4,
        components: IdMap::by_name(
            names
                .iter()
                .enumerate()
                .map(|(id, n)| (n.as_str(), id as u32)),
            names
                .iter()
                .enumerate()
                .map(|(id, n)| (n.as_str(), id as u32)),
            |_| None,
        ),
        component_shapes: Components::new(&names, &names),
        ..Tables::default()
    }
}

#[test]
fn nested_templates_are_item_first_not_optional_slots() {
    let t = tables(&["use_remainder", "bundle_contents", "container"]);
    for (component, payload, expected) in [
        (0, vec![10, 2, 0, 0], vec![3, 2, 0, 0]),
        (1, vec![1, 10, 2, 0, 0], vec![1, 3, 2, 0, 0]),
        (2, vec![2, 0, 1, 10, 2, 0, 0], vec![2, 0, 1, 3, 2, 0, 0]),
    ] {
        let mut body = vec![1, 10, 1, 0, component];
        body.extend(payload);
        let mut r = body.as_slice();
        let mut out = Vec::new();
        stack(&t, To::Legacy, &mut r, &mut out).unwrap();
        let mut wanted = vec![1, 3, 1, 0, component];
        wanted.extend(expected);
        assert_eq!(out, wanted);
        assert!(r.is_empty());
        let mut roundtrip = Vec::new();
        stack(&t, To::Canonical, &mut out.as_slice(), &mut roundtrip).unwrap();
        assert_eq!(roundtrip, body);
    }
}

#[test]
fn pot_decorations_convert_optional_templates_to_four_item_ids() {
    let t = tables(&["pot_decorations"]);
    let body = [1, 10, 1, 0, 0, 1, 10, 1, 0, 0, 0, 1, 11, 1, 0, 0, 0];
    let mut r = body.as_slice();
    let mut out = Vec::new();
    stack(&t, To::Legacy, &mut r, &mut out).unwrap();
    assert!(r.is_empty());
    assert_eq!(out, [1, 3, 1, 0, 0, 4, 3, 4, 4, 4]);
    let mut canonical = Vec::new();
    stack(&t, To::Canonical, &mut out.as_slice(), &mut canonical).unwrap();
    assert_eq!(&canonical[..5], &[1, 10, 1, 0, 0]);
    assert_eq!(
        &canonical[5..],
        &[
            1, 10, 1, 0, 0, 1, 11, 1, 0, 0, 1, 11, 1, 0, 0, 1, 11, 1, 0, 0
        ]
    );
}

#[test]
fn every_emitted_component_has_a_walker() {
    for (name, _) in mc_data::data_component_types_26_1_2::ITEM_COMPONENT_CODECS {
        assert!(payload(name).is_some(), "missing walker for {name}");
    }
}

#[test]
fn native_only_components_are_consumed_before_legacy_omission() {
    // These 14 names are absent from the local 26.1.2 component registry.
    // Payload bytes follow the 26.3 component-codec fixtures.
    let mut sign = Vec::new();
    for line in ["A", "B", "C", "D"] {
        mc_nbt::write_network_value(&mut sign, &mc_nbt::Tag::String(line.into())).unwrap();
    }
    sign.extend([0, 11, 1]); // no filtered lines, blue, glowing
    let mut fuel = vec![1];
    fuel.extend(1600_i32.to_be_bytes());
    fuel.push(1);
    fuel.extend(1.5_f32.to_be_bytes());
    let mut visibility = vec![0];
    visibility
        .write_string("minecraft:skeletons", 32767)
        .unwrap();
    visibility.extend(0.5_f32.to_be_bytes());
    for (name, payload) in [
        ("attack_animation", vec![2, 130, 1]),
        ("interact_animation", vec![1, 6]),
        ("block_transformer", vec![0]),
        ("villager_food", vec![130, 1]),
        ("sulfur_cube_content", vec![11, 2, 1, 0, 120]),
        ("compostable", vec![1, 0, 0, 0, 2]),
        ("cooking_fuel", fuel.clone()),
        ("brewing_fuel", fuel),
        ("mob_visibility", visibility),
        ("provides_pottery_pattern", vec![0]),
        ("sign_text_front", sign.clone()),
        ("sign_text_back", sign),
        ("waxed", vec![]),
        ("cushion/color", vec![14]),
    ] {
        let mut t = tables(&[name, "max_stack_size"]);
        t.components = IdMap::by_name(
            [(name, 0), ("max_stack_size", 1)],
            [("max_stack_size", 0)],
            |_| None,
        );
        // Follow the omitted component with a retained component and another
        // stack, catching payload under/over-consumption including nested patches.
        let mut body = vec![1, 10, 2, 0, 0];
        body.extend(payload);
        body.extend([1, 32, 2, 11, 0, 0]);
        let mut cursor = body.as_slice();
        let mut legacy = Vec::new();
        stack(&t, To::Legacy, &mut cursor, &mut legacy)
            .unwrap_or_else(|error| panic!("{name}: {error:#}"));
        stack(&t, To::Legacy, &mut cursor, &mut legacy).unwrap();
        assert!(cursor.is_empty(), "{name}");
        assert_eq!(legacy, [1, 3, 1, 0, 0, 32, 2, 4, 0, 0], "{name}");
    }
    let t = tables(&["attack_animation"]);
    let mut truncated = [1, 10, 1, 0, 0, 2].as_slice();
    assert!(stack(&t, To::Legacy, &mut truncated, &mut Vec::new()).is_err());
}
