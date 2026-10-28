use super::{text, translated};
use mc_nbt::{ListTag, Tag, tag_type};

#[test]
fn literal_text_has_one_string_field() {
    assert_eq!(
        text("name 😀"),
        Tag::Compound(vec![("text".into(), Tag::String("name 😀".into()))])
    );
}

#[test]
fn translation_without_arguments_omits_with() {
    assert_eq!(
        translated("commands.tick.status.frozen", Vec::new()),
        Tag::Compound(vec![(
            "translate".into(),
            Tag::String("commands.tick.status.frozen".into()),
        )])
    );
}

#[test]
fn translation_keeps_field_argument_order_and_compound_list_type() {
    let first = Tag::Compound(vec![("text".into(), Tag::String("first".into()))]);
    let second = Tag::Compound(vec![("translate".into(), Tag::String("second".into()))]);
    assert_eq!(
        translated(
            String::from("commands.example"),
            vec![first.clone(), second.clone()],
        ),
        Tag::Compound(vec![
            ("translate".into(), Tag::String("commands.example".into()),),
            (
                "with".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![first, second],
                }),
            ),
        ])
    );
}
