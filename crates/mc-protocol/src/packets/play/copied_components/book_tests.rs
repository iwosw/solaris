use super::*;
use bytes::BytesMut;

#[test]
fn writable_book_wire_matches_filterable_string_layout() {
    let book = Tag::Compound(vec![(
        "pages".into(),
        make_list(vec![Tag::String("hello".into())]),
    )]);
    let mut bytes = BytesMut::new();
    encode(&mut bytes, &book, false).unwrap();
    assert_eq!(bytes.as_ref(), b"\x01\x05hello\x00");
    assert_eq!(decode(&mut bytes.freeze(), false).unwrap(), book);
}
#[test]
fn written_book_roundtrip_preserves_title_author_generation_filtered_pages() {
    let book = Tag::Compound(vec![
        ("title".into(), Tag::String("a".into())),
        ("author".into(), Tag::String("b".into())),
        ("generation".into(), Tag::Int(2)),
        (
            "pages".into(),
            make_list(vec![Tag::Compound(vec![
                ("raw".into(), Tag::String("raw".into())),
                ("filtered".into(), Tag::String("filtered".into())),
            ])]),
        ),
        ("resolved".into(), Tag::Byte(1)),
    ]);
    let mut bytes = BytesMut::new();
    encode(&mut bytes, &book, true).unwrap();
    assert_eq!(&bytes[..6], b"\x01a\x00\x01b\x02");
    assert_eq!(decode(&mut bytes.freeze(), true).unwrap(), book);
}
#[test]
fn book_pages_and_generations_are_bounded() {
    let too_many = Tag::Compound(vec![(
        "pages".into(),
        make_list(vec![Tag::String(String::new()); 101]),
    )]);
    assert!(encode(&mut BytesMut::new(), &too_many, false).is_err());
    let too_long = Tag::Compound(vec![(
        "pages".into(),
        make_list(vec![Tag::String("x".repeat(1025))]),
    )]);
    assert!(encode(&mut BytesMut::new(), &too_long, false).is_err());
    assert!(decode(&mut &b"\x65"[..], false).is_err());
    assert!(decode(&mut &b"\x01a\x00\x01b\x04"[..], true).is_err());
}
