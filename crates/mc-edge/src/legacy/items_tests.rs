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
