use mc_nbt::Tag;

pub(super) fn fields(tag: &Tag) -> Option<&[(String, Tag)]> {
    match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    }
}
pub(super) fn fields_mut(tag: &mut Tag) -> Option<&mut Vec<(String, Tag)>> {
    match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    }
}
pub(super) fn get<'a>(tag: &'a Tag, key: &str) -> Option<&'a Tag> {
    fields(tag)?
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}
pub(super) fn get_mut<'a>(tag: &'a mut Tag, key: &str) -> Option<&'a mut Tag> {
    fields_mut(tag)?
        .iter_mut()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}
pub(super) fn take(tag: &mut Tag, key: &str) -> Option<Tag> {
    let fields = fields_mut(tag)?;
    let index = fields.iter().position(|(name, _)| name == key)?;
    Some(fields.remove(index).1)
}
pub(super) fn set(tag: &mut Tag, key: &str, value: Tag) {
    if let Some(existing) = get_mut(tag, key) {
        *existing = value;
    } else if let Some(fields) = fields_mut(tag) {
        fields.push((key.into(), value));
    }
}
pub(super) fn rename(tag: &mut Tag, old: &str, new: &str) {
    if let Some(value) = take(tag, old) {
        set(tag, new, value);
    }
}
pub(super) fn remove(tag: &mut Tag, key: &str) {
    if let Some(fields) = fields_mut(tag) {
        fields.retain(|(name, _)| name != key);
    }
}
pub(super) fn string(tag: &Tag) -> Option<&str> {
    match tag {
        Tag::String(value) => Some(value),
        _ => None,
    }
}
pub(super) fn int(tag: &Tag) -> Option<i32> {
    match tag {
        Tag::Byte(value) => Some(i32::from(*value)),
        Tag::Short(value) => Some(i32::from(*value)),
        Tag::Int(value) => Some(*value),
        _ => None,
    }
}
pub(super) fn number(tag: &Tag) -> Option<f64> {
    match tag {
        Tag::Byte(value) => Some(f64::from(*value)),
        Tag::Short(value) => Some(f64::from(*value)),
        Tag::Int(value) => Some(f64::from(*value)),
        Tag::Long(value) => Some(*value as f64),
        Tag::Float(value) => Some(f64::from(*value)),
        Tag::Double(value) => Some(*value),
        _ => None,
    }
}
pub(super) fn list(tag: &Tag) -> Option<&[Tag]> {
    match tag {
        Tag::List(list) => Some(&list.elements),
        _ => None,
    }
}
pub(super) fn list_mut(tag: &mut Tag) -> Option<&mut Vec<Tag>> {
    match tag {
        Tag::List(list) => Some(&mut list.elements),
        _ => None,
    }
}
pub(super) fn compound(fields: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        fields
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
}
