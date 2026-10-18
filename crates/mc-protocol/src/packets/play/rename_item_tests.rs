use super::*;
use bytes::{Bytes, BytesMut};

#[test]
fn rename_accepts_full_native_utf16_limit_and_rejects_next_code_unit() {
    let name = format!("{}a", "😀".repeat(16383));
    let packet = ServerboundRenameItem { name: name.clone() };
    let mut bytes = BytesMut::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(
        ServerboundRenameItem::decode(&mut bytes.freeze()).unwrap(),
        packet
    );
    let too_long = ServerboundRenameItem {
        name: format!("{name}a"),
    };
    assert!(too_long.encode(&mut BytesMut::new()).is_err());
    let mut bytes = BytesMut::new();
    bytes.write_string(&too_long.name, 32768).unwrap();
    assert!(ServerboundRenameItem::decode(&mut bytes.freeze()).is_err());
}

#[test]
fn rename_accepts_empty_clear_name_and_rejects_truncated_or_invalid_utf8() {
    let clear = ServerboundRenameItem {
        name: String::new(),
    };
    assert_eq!(
        ServerboundRenameItem::decode(&mut Bytes::from_static(&[0])).unwrap(),
        clear
    );
    for malformed in [&[2, b'a'][..], &[1, 0xff][..], &[0x80][..]] {
        assert!(ServerboundRenameItem::decode(&mut Bytes::copy_from_slice(malformed)).is_err());
    }
}
