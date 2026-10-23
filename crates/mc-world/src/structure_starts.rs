//! Native and imported start metadata shared by generation and locate.
use crate::{BlockPos, Chunk, ChunkPos, chunk::GeneratedStructureSite};
use mc_data::Identifier;
use mc_nbt::Tag;

/// Missing starts mean no generated structure, not permission to consult worldgen.
pub fn read(
    fields: &[(String, Tag)],
    chunk: ChunkPos,
    skip_referenced: bool,
) -> Result<Vec<GeneratedStructureSite>, String> {
    let Some((_, value)) = fields.iter().find(|(k, _)| k == "structures") else {
        return Ok(Vec::new());
    };
    let Tag::Compound(structures) = value else {
        return Err("invalid structures compound".into());
    };
    let Some((_, value)) = structures.iter().find(|(k, _)| k == "starts") else {
        return Ok(Vec::new());
    };
    let Tag::Compound(starts) = value else {
        return Err("invalid starts compound".into());
    };
    let mut out = Vec::new();
    for (name, value) in starts {
        let Tag::Compound(start) = value else {
            return Err("invalid structure start compound".into());
        };
        let lookup = |key: &str| start.iter().find(|(k, _)| k == key).map(|(_, v)| v);
        if matches!(lookup("id"),Some(Tag::String(id)) if id=="INVALID") {
            continue;
        }
        let structure =
            Identifier::parse(name).map_err(|_| "invalid saved structure identifier")?;
        if !matches!(lookup("id"),Some(Tag::String(id)) if id==name) {
            return Err("structure start identifier mismatch".into());
        }
        let integer = |key: &str| match lookup(key) {
            Some(Tag::Int(v)) => Ok(*v),
            _ => Err(format!("missing structure {key}")),
        };
        let start_chunk = (integer("ChunkX")?, integer("ChunkZ")?);
        if start_chunk != (chunk.x, chunk.z) {
            return Err("structure start chunk mismatch".into());
        }
        let references = match lookup("references") {
            None => 0,
            Some(Tag::Int(value)) if *value >= 0 => *value,
            _ => return Err("invalid structure reference count".into()),
        };
        let bounds = lookup("BB").or_else(|| match lookup("Children") {
            Some(Tag::List(children)) => children.elements.iter().find_map(|child| match child {
                Tag::Compound(fields) => fields.iter().find(|(k, _)| k == "BB").map(|(_, v)| v),
                _ => None,
            }),
            _ => None,
        });
        let Some(bounds) = bounds else {
            return Err("missing structure bounding box".into());
        };
        let Tag::IntArray(bb) = bounds else {
            return Err("invalid structure bounding box type".into());
        };
        if bb.len() != 6 || bb[0] > bb[3] || bb[1] > bb[4] || bb[2] > bb[5] {
            return Err("invalid structure bounding box".into());
        }
        let position = match lookup("SolarisLocatePos") {
            None => BlockPos {
                x: bb[0],
                y: bb[1],
                z: bb[2],
            },
            Some(Tag::IntArray(pos)) if pos.len() == 3 => BlockPos {
                x: pos[0],
                y: pos[1],
                z: pos[2],
            },
            Some(_) => return Err("invalid native structure locate anchor".into()),
        };
        if skip_referenced && references > 0 {
            continue;
        }
        out.push(GeneratedStructureSite {
            structure,
            start_chunk,
            position,
        });
    }
    Ok(out)
}

/// Add a native start without discarding imported References or other starts.
pub fn insert(chunk: &mut Chunk, site: &GeneratedStructureSite, min: BlockPos, max: BlockPos) {
    if site.start_chunk != (chunk.pos.x, chunk.pos.z) {
        return;
    }
    let index = chunk
        .extras
        .iter()
        .position(|(k, _)| k == "structures")
        .unwrap_or_else(|| {
            chunk
                .extras
                .push(("structures".into(), Tag::Compound(Vec::new())));
            chunk.extras.len() - 1
        });
    let Tag::Compound(structures) = &mut chunk.extras[index].1 else {
        panic!("generated chunk structures must be compound");
    };
    let index = structures
        .iter()
        .position(|(k, _)| k == "starts")
        .unwrap_or_else(|| {
            structures.push(("starts".into(), Tag::Compound(Vec::new())));
            structures.len() - 1
        });
    let Tag::Compound(starts) = &mut structures[index].1 else {
        panic!("generated structure starts must be compound");
    };
    starts.retain(|(key, _)| key != site.structure.as_str());
    starts.push((
        site.structure.to_string(),
        Tag::Compound(vec![
            ("id".into(), Tag::String(site.structure.to_string())),
            ("ChunkX".into(), Tag::Int(site.start_chunk.0)),
            ("ChunkZ".into(), Tag::Int(site.start_chunk.1)),
            ("references".into(), Tag::Int(0)),
            (
                "SolarisLocatePos".into(),
                Tag::IntArray(vec![site.position.x, site.position.y, site.position.z]),
            ),
            (
                "BB".into(),
                Tag::IntArray(vec![min.x, min.y, min.z, max.x, max.y, max.z]),
            ),
        ]),
    ));
}
