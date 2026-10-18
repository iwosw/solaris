//! ResolvableProfile: either full GameProfile or Partial, then four optional
//! PlayerSkin.Patch fields. PlayerModelType is a boolean (slim), not a VarInt.
use super::{compound, field, invalid, list, make_list, required, string};
use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;
use bytes::{Buf, BufMut};
use mc_data::Identifier;
use mc_nbt::Tag;
use uuid::Uuid;

pub(super) fn encode<B: BufMut>(buf: &mut B, value: &Tag) -> Result<(), CodecError> {
    if let Tag::String(name) = value {
        buf.write_bool(false);
        buf.write_bool(true);
        buf.write_string(name, 16)?;
        buf.write_bool(false);
        buf.write_varint(0);
        for _ in 0..4 {
            buf.write_bool(false);
        }
        return Ok(());
    }
    let fields = compound(value)?;
    let name = field(fields, "name").map(string).transpose()?;
    let id = field(fields, "id").map(uuid).transpose()?;
    let full = name.is_some() && id.is_some();
    buf.write_bool(full);
    if full {
        buf.write_uuid(id.unwrap());
        buf.write_string(name.unwrap(), 16)?;
    } else {
        buf.write_bool(name.is_some());
        if let Some(name) = name {
            buf.write_string(name, 16)?;
        }
        buf.write_bool(id.is_some());
        if let Some(id) = id {
            buf.write_uuid(id);
        }
    }
    write_properties(buf, field(fields, "properties"))?;
    for key in ["texture", "cape", "elytra"] {
        let texture = field(fields, key).map(string).transpose()?;
        buf.write_bool(texture.is_some());
        if let Some(texture) = texture {
            buf.write_identifier(&Identifier::parse(texture).map_err(|_| invalid())?)?;
        }
    }
    let model = field(fields, "model").map(string).transpose()?;
    buf.write_bool(model.is_some());
    if let Some(model) = model {
        buf.write_bool(match model {
            "slim" => true,
            "wide" => false,
            _ => return Err(invalid()),
        });
    }
    Ok(())
}

pub(super) fn decode<B: Buf>(buf: &mut B) -> Result<Tag, CodecError> {
    let full = buf.read_bool()?;
    let (name, id) = if full {
        let id = buf.read_uuid()?;
        (Some(buf.read_string(16)?), Some(id))
    } else {
        let name = if buf.read_bool()? {
            Some(buf.read_string(16)?)
        } else {
            None
        };
        let id = if buf.read_bool()? {
            Some(buf.read_uuid()?)
        } else {
            None
        };
        (name, id)
    };
    let mut fields = Vec::new();
    if let Some(id) = id {
        fields.push(("id".into(), uuid_tag(id)));
    }
    if let Some(name) = name {
        fields.push(("name".into(), Tag::String(name)));
    }
    let count = crate::codec::read_bounded_count(buf, 16, 3)?;
    let mut properties = Vec::with_capacity(count);
    for _ in 0..count {
        let name = buf.read_string(64)?;
        let value = buf.read_string(32767)?;
        let mut property = vec![
            ("name".into(), Tag::String(name)),
            ("value".into(), Tag::String(value)),
        ];
        if buf.read_bool()? {
            property.push(("signature".into(), Tag::String(buf.read_string(1024)?)));
        }
        properties.push(Tag::Compound(property));
    }
    if !properties.is_empty() {
        fields.push(("properties".into(), make_list(properties)));
    }
    for key in ["texture", "cape", "elytra"] {
        if buf.read_bool()? {
            fields.push((
                key.into(),
                Tag::String(buf.read_identifier()?.as_str().into()),
            ));
        }
    }
    if buf.read_bool()? {
        fields.push((
            "model".into(),
            Tag::String(if buf.read_bool()? { "slim" } else { "wide" }.into()),
        ));
    }
    Ok(Tag::Compound(fields))
}

fn write_properties<B: BufMut>(buf: &mut B, value: Option<&Tag>) -> Result<(), CodecError> {
    let count = match value {
        None => 0,
        Some(Tag::List(properties)) => properties.elements.len(),
        Some(Tag::Compound(groups)) => {
            let mut count = 0_usize;
            for (_, entries) in groups {
                count = count
                    .checked_add(list(entries)?.len())
                    .ok_or_else(invalid)?;
            }
            count
        }
        Some(_) => return Err(invalid()),
    };
    super::super::write_bounded_count(buf, count, 16)?;
    match value {
        Some(Tag::List(properties)) => {
            for property in &properties.elements {
                let fields = compound(property)?;
                write_property(buf, string(required(fields, "name")?)?, fields)?;
            }
        }
        Some(Tag::Compound(groups)) => {
            for (name, properties) in groups {
                for property in list(properties)? {
                    write_property(buf, name, compound(property)?)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn write_property<B: BufMut>(
    buf: &mut B,
    name: &str,
    fields: &[(String, Tag)],
) -> Result<(), CodecError> {
    buf.write_string(name, 64)?;
    buf.write_string(string(required(fields, "value")?)?, 32767)?;
    let signature = field(fields, "signature").map(string).transpose()?;
    buf.write_bool(signature.is_some());
    if let Some(signature) = signature {
        buf.write_string(signature, 1024)?;
    }
    Ok(())
}

fn uuid(value: &Tag) -> Result<Uuid, CodecError> {
    match value {
        Tag::IntArray(parts) if parts.len() == 4 => Ok(Uuid::from_u128(
            (u128::from(parts[0] as u32) << 96)
                | (u128::from(parts[1] as u32) << 64)
                | (u128::from(parts[2] as u32) << 32)
                | u128::from(parts[3] as u32),
        )),
        Tag::String(value) => Uuid::parse_str(value).map_err(|_| invalid()),
        _ => Err(invalid()),
    }
}

fn uuid_tag(id: Uuid) -> Tag {
    let value = id.as_u128();
    Tag::IntArray(vec![
        (value >> 96) as i32,
        (value >> 64) as i32,
        (value >> 32) as i32,
        value as i32,
    ])
}
