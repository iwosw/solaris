use super::*;
use crate::Identifier;
fn registry() -> ItemRegistry {
    crate::items::solaris_required_items()
}
#[test]
fn crafter_exact_nbt_roundtrip_retains_components_and_disabled_indices() {
    let items = registry();
    let mut slots = vec![ItemStack::EMPTY; 9];
    let mut item = ItemStack::new(
        items
            .id_of(&Identifier::parse("minecraft:oak_log").unwrap())
            .unwrap(),
        3,
    );
    item.custom_name = Some("named".into());
    slots[4] = item;
    let settings = CrafterSettings {
        disabled_slots: 0x1ef,
        crafting_ticks_remaining: 6,
        triggered: true,
    };
    let mut fields = vec![("id".into(), Tag::String("minecraft:crafter".into()))];
    encode_items(&mut fields, &slots, &items);
    encode_crafter_fields(&mut fields, settings);
    assert_eq!(
        field(&fields, "disabled_slots"),
        Some(&Tag::IntArray(vec![0, 1, 2, 3, 5, 6, 7, 8]))
    );
    assert_eq!(field(&fields, "triggered"), Some(&Tag::Int(1)));
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).unwrap();
    let Tag::Compound(decoded) = mc_nbt::read_network(&mut &bytes[..]).unwrap() else {
        panic!()
    };
    assert_eq!(decode_crafter(&decoded, &items).unwrap(), (settings, slots));
}
#[test]
fn shelf_exact_three_slots_and_alignment_roundtrip() {
    let items = registry();
    let mut slots = vec![ItemStack::EMPTY; 3];
    slots[2] = ItemStack::new(
        items
            .id_of(&Identifier::parse("minecraft:diamond").unwrap())
            .unwrap(),
        64,
    );
    let mut fields = vec![
        ("id".into(), Tag::String("minecraft:shelf".into())),
        ("align_items_to_bottom".into(), Tag::Byte(1)),
    ];
    encode_items(&mut fields, &slots, &items);
    assert_eq!(decode_shelf(&fields, &items).unwrap(), (true, slots));
}
#[test]
fn corrupt_storage_records_fail_closed_with_exact_diagnostics() {
    let items = registry();
    for (fields, wanted) in [
        (
            vec![("disabled_slots".into(), Tag::IntArray(vec![9]))],
            "disabled_slots: expected slot in 0..=8",
        ),
        (
            vec![("disabled_slots".into(), Tag::IntArray(vec![2, 2]))],
            "disabled_slots: duplicate slot 2",
        ),
        (
            vec![("triggered".into(), Tag::Byte(1))],
            "triggered: expected int in 0..=1",
        ),
        (
            vec![("crafting_ticks_remaining".into(), Tag::Int(7))],
            "crafting_ticks_remaining: expected int in 0..=6",
        ),
        (
            vec![
                ("triggered".into(), Tag::Int(0)),
                ("triggered".into(), Tag::Int(1)),
            ],
            "triggered: duplicate field",
        ),
    ] {
        assert_eq!(decode_crafter(&fields, &items).unwrap_err(), wanted);
    }
    let mut fields = Vec::new();
    let item = ItemStack::new(
        items
            .id_of(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap(),
        1,
    );
    encode_items(&mut fields, &[item], &items);
    let Some(Tag::List(list)) = fields
        .iter_mut()
        .find(|(k, _)| k == "Items")
        .map(|(_, v)| v)
    else {
        panic!()
    };
    list.elements.push(list.elements[0].clone());
    assert_eq!(
        decode_crafter(&fields, &items).unwrap_err(),
        "Items[].Slot: duplicate slot 0"
    );
    assert_eq!(
        decode_shelf(&[("align_items_to_bottom".into(), Tag::Int(1))], &items).unwrap_err(),
        "align_items_to_bottom: expected boolean byte"
    );
}
