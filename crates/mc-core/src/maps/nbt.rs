//! 26.1.2 MapItemSavedData.CODEC; legacy paths are normalized by conversion.
use super::{Banner, SavedMap};
use mc_nbt::{Tag, ListTag, tag_type};
use crate::dimensions::Dimension;

fn field<'a>(tag: &'a Tag, key: &str) -> Result<Option<&'a Tag>, String> {
    let Tag::Compound(fields) = tag else { return Err("map expected compound".into()); };
    let mut found = fields.iter().filter(|(name, _)| name == key);
    let value = found.next().map(|(_, value)| value);
    if found.next().is_some() { return Err(format!("duplicate map field {key}")); }
    Ok(value)
}
fn int(tag: &Tag, key: &str, default: Option<i32>) -> Result<i32, String> {
    match field(tag, key)? {
        Some(Tag::Int(n)) => Ok(*n), Some(Tag::Byte(n)) => Ok(i32::from(*n)),
        None => default.ok_or_else(|| format!("missing map field {key}")),
        _ => Err(format!("invalid map field {key}")),
    }
}
pub fn from_tag(root: &Tag) -> Result<SavedMap, String> {
    let data = field(root, "data")?.ok_or("missing map data")?;
    let dimension = match field(data, "dimension")? {
        Some(Tag::String(name)) => match name.as_str() {
            "minecraft:overworld" => Dimension::Overworld,
            "minecraft:the_nether" => Dimension::Nether,
            "minecraft:the_end" => Dimension::End,
            _ => return Err("unsupported map dimension".into()),
        },
        _ => return Err("missing map dimension".into()),
    };
    let scale = u8::try_from(int(data, "scale", Some(0))?).map_err(|_| "invalid scale")?;
    let center = [int(data, "xCenter", None)?, int(data, "zCenter", None)?];
    let mut map = SavedMap::new(center, scale, dimension)?;
    map.center = center;
    map.locked = int(data, "locked", Some(0))? != 0;
    map.tracking = int(data, "trackingPosition", Some(1))? != 0;
    map.unlimited_tracking = int(data, "unlimitedTracking", Some(0))? != 0;
    map.colors = match field(data, "colors")? {
        Some(Tag::ByteArray(colors)) => colors.iter().map(|n| *n as u8).collect(),
        _ => return Err("missing map colors".into()),
    };
    if let Some(Tag::List(list)) = field(data, "banners")? {
        for tag in &list.elements {
            let pos = match field(tag, "pos")? {
                Some(Tag::IntArray(pos)) if pos.len() == 3 => [pos[0], pos[1], pos[2]],
                _ => return Err("invalid map banner position".into()),
            };
            let color = match field(tag, "color")? {
                Some(Tag::String(color)) => color.clone(), None => "white".into(),
                _ => return Err("invalid map banner color".into()),
            };
            if super::decoration_id(&format!("banner_{color}")).is_none() { return Err("invalid banner color".into()); }
            let name = match field(tag, "name")? { Some(Tag::String(name)) => Some(name.clone()), _ => None };
            map.banners.push(Banner { pos, color, name });
        }
    }
    mc_nbt::write_named(&mut map.original_nbt, "", root).map_err(|e| e.to_string())?;
    map.validate()?;
    Ok(map)
}
fn put(fields: &mut Vec<(String, Tag)>, key: &str, value: Tag) {
    fields.retain(|(name, _)| name != key);
    fields.push((key.into(), value));
}
pub fn to_tag(map: &SavedMap) -> Result<Tag, String> {
    map.validate()?;
    let mut root = if map.original_nbt.is_empty() { Tag::Compound(Vec::new()) }
        else { mc_nbt::read_named(&mut map.original_nbt.as_slice()).map_err(|e| e.to_string())?.1 };
    let Tag::Compound(root_fields) = &mut root else { return Err("invalid map envelope".into()); };
    let mut data = root_fields.iter().find(|(key, _)| key == "data").map(|(_, data)| data.clone())
        .unwrap_or_else(|| Tag::Compound(Vec::new()));
    let Tag::Compound(fields) = &mut data else { return Err("invalid map data".into()); };
    put(fields, "dimension", Tag::String(map.dimension.name().into()));
    put(fields, "xCenter", Tag::Int(map.center[0]));
    put(fields, "zCenter", Tag::Int(map.center[1]));
    put(fields, "scale", Tag::Byte(map.scale as i8));
    put(fields, "locked", Tag::Byte(i8::from(map.locked)));
    put(fields, "trackingPosition", Tag::Byte(i8::from(map.tracking)));
    put(fields, "unlimitedTracking", Tag::Byte(i8::from(map.unlimited_tracking)));
    put(fields, "colors", Tag::ByteArray(map.colors.iter().map(|c| *c as i8).collect()));
    // Preserve imported component styles/unknown fields per retained banner;
    // do not resurrect removed banners from the original envelope.
    let original_banners = fields.iter().find(|(name, _)| name == "banners").map(|(_, v)| v.clone());
    let banners = map.banners.iter().map(|banner| {
        let mut fields = match &original_banners {
            Some(Tag::List(list)) => list.elements.iter().find_map(|tag| {
                if field(tag,"pos").ok().flatten() == Some(&Tag::IntArray(banner.pos.to_vec()))
                    && let Tag::Compound(fields) = tag {Some(fields.clone())} else {None}
            }).unwrap_or_default(),
            _ => Vec::new(),
        };
        put(&mut fields,"pos",Tag::IntArray(banner.pos.to_vec()));
        put(&mut fields,"color",Tag::String(banner.color.clone()));
        if let Some(name) = &banner.name { put(&mut fields,"name",Tag::String(name.clone())); }
        Tag::Compound(fields)
    }).collect();
    put(fields, "banners",Tag::List(ListTag { element_type:tag_type::COMPOUND,elements:banners }));
    put(root_fields, "data", data);
    Ok(root)
}

pub fn sequence_from_tag(root: &Tag) -> Result<i64, String> {
    let data = field(root,"data")?.unwrap_or(root);
    let last = match field(data,"map")? {
        Some(Tag::Int(n)) => i64::from(*n), Some(Tag::Short(n)) => i64::from(*n),
        Some(Tag::Byte(n)) => i64::from(*n), None => -1,
        _ => return Err("invalid map sequence".into()),
    };
    if last < -1 { return Err("invalid map sequence".into()); }
    Ok(last + 1)
}
pub fn sequence_to_tag(next: i64) -> Result<Tag, String> {
    if !(0..=i64::from(i32::MAX)+1).contains(&next) { return Err("invalid map sequence".into()); }
    Ok(Tag::Compound(vec![("data".into(), Tag::Compound(vec![("map".into(),Tag::Int((next-1) as i32))]))]))
}
