use super::NativeError;
use std::collections::BTreeSet;
fn bad(key: &str) -> NativeError {
    NativeError::Source(format!("native checkpoint/{key}: invalid NBT record"))
}

pub(super) fn nbt(tag: &mc_nbt::Tag) -> Result<(), NativeError> {
    use mc_nbt::Tag;
    match tag {
        Tag::Float(v) if !v.is_finite() => return Err(bad("NBT/nonfinite")),
        Tag::Double(v) if !v.is_finite() => return Err(bad("NBT/nonfinite")),
        Tag::Compound(fields) => {
            let mut seen = BTreeSet::new();
            for (key, value) in fields {
                if !seen.insert(key) {
                    return Err(bad("NBT/duplicate_field"));
                }
                nbt(value)?;
            }
        }
        Tag::List(list) => {
            for value in &list.elements {
                nbt(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
