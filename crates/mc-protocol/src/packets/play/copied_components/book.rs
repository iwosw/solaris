//! Book stream layout from local 26.1.2 WritableBookContent,
//! WrittenBookContent and Filterable javap evidence.
use super::*;

fn parts(value: &Tag) -> (&Tag, Option<&Tag>) {
    if let Tag::Compound(fields) = value {
        if let Some(raw) = field(fields, "raw") {
            return (raw, field(fields, "filtered"));
        }
        // The NBT heterogeneous-list envelope is not a text component.
        if fields.len() == 1 && fields[0].0.is_empty() {
            return parts(&fields[0].1);
        }
    }
    (value, None)
}
fn write_filtered<B: BufMut>(
    buf: &mut B,
    value: &Tag,
    limit: Option<usize>,
) -> Result<(), CodecError> {
    let (raw, filtered) = parts(value);
    let write = |buf: &mut B, value: &Tag| match limit {
        Some(limit) => buf.write_string(string(value)?, limit),
        None => write_text(buf, value),
    };
    write(buf, raw)?;
    buf.write_bool(filtered.is_some());
    if let Some(value) = filtered {
        write(buf, value)?;
    }
    Ok(())
}
fn read_filtered<B: Buf>(buf: &mut B, limit: Option<usize>) -> Result<Tag, CodecError> {
    let read = |buf: &mut B| match limit {
        Some(limit) => buf.read_string(limit).map(Tag::String),
        None => read_text(buf),
    };
    let raw = read(buf)?;
    Ok(if buf.read_bool()? {
        Tag::Compound(vec![("raw".into(), raw), ("filtered".into(), read(buf)?)])
    } else {
        raw
    })
}
pub(super) fn encode<B: BufMut>(buf: &mut B, value: &Tag, written: bool) -> Result<(), CodecError> {
    let fields = compound(value)?;
    if written {
        write_filtered(buf, required(fields, "title")?, Some(32))?;
        buf.write_string(string(required(fields, "author")?)?, 32767)?;
        let generation = field(fields, "generation")
            .map(integer)
            .transpose()?
            .unwrap_or(0);
        if !(0..=3).contains(&generation) {
            return Err(invalid());
        }
        buf.write_varint(generation);
    }
    let pages = field(fields, "pages").map(list).transpose()?.unwrap_or(&[]);
    super::super::write_bounded_count(buf, pages.len(), 100)?;
    for page in pages {
        write_filtered(buf, page, (!written).then_some(1024))?;
    }
    if written {
        buf.write_bool(
            field(fields, "resolved")
                .map(integer)
                .transpose()?
                .unwrap_or(0)
                != 0,
        );
    }
    Ok(())
}
pub(super) fn decode<B: Buf>(buf: &mut B, written: bool) -> Result<Tag, CodecError> {
    let mut fields = Vec::new();
    if written {
        fields.push(("title".into(), read_filtered(buf, Some(32))?));
        fields.push(("author".into(), Tag::String(buf.read_string(32767)?)));
        let generation = buf.read_varint()?;
        if !(0..=3).contains(&generation) {
            return Err(invalid());
        }
        fields.push(("generation".into(), Tag::Int(generation)));
    }
    let pages = read_bounded_vec(buf, 100, 2, |buf| {
        read_filtered(buf, (!written).then_some(1024))
    })?;
    fields.push(("pages".into(), make_list(pages)));
    if written {
        fields.push(("resolved".into(), Tag::Byte(i8::from(buf.read_bool()?))));
    }
    Ok(Tag::Compound(fields))
}

#[cfg(test)]
#[path = "book_tests.rs"]
mod tests;
