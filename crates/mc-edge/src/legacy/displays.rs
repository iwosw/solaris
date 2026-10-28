//! Registry-bearing command, statistic, recipe and advancement presentations.
//! Layout authority is the canonical packet codecs; advancement x/y moved
//! outside DisplayInfo in 26.3 and must be inserted back for a 26.1.2 client.
use super::{
    items::{self, To, copy_nbt},
    tables::Tables,
};
use anyhow::{Context, Result, bail, ensure};
use mc_protocol::codec::{ReadMc, WriteMc};

fn count(r: &mut &[u8], w: &mut Vec<u8>) -> Result<i32> {
    let n = r.read_varint()?;
    ensure!((0..=65536).contains(&n), "display list length {n}");
    w.write_varint(n);
    Ok(n)
}
fn string(r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_string(&r.read_string(32767)?, 32767)?;
    Ok(())
}
fn item(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_varint(
        t.items
            .get(r.read_varint()?)
            .context("unmapped recipe item")?,
    );
    Ok(())
}
fn ingredient(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    let n = r.read_varint()?;
    ensure!((0..=65537).contains(&n), "ingredient size");
    w.write_varint(n);
    if n == 0 {
        string(r, w)?;
    } else {
        for _ in 1..n {
            item(t, r, w)?;
        }
    }
    Ok(())
}
fn slot(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>, depth: usize) -> Result<()> {
    ensure!(depth <= 16, "slot display depth");
    let kind = r.read_varint()?;
    w.write_varint(kind); // slot_display registration order is unchanged
    match kind {
        0 | 1 => {}
        4 => item(t, r, w)?,
        5 => items::template(t, To::Legacy, r, w)?,
        6 => string(r, w)?,
        10 => {
            for _ in 0..count(r, w)? {
                slot(t, r, w, depth + 1)?;
            }
        }
        _ => bail!("slot display {kind} is not emitted by Solaris"),
    }
    Ok(())
}
fn recipe(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    let kind = r.read_varint()?;
    w.write_varint(kind); // recipe_display registration order is unchanged
    match kind {
        0 | 1 => {
            if kind == 1 {
                w.write_varint(r.read_varint()?);
                w.write_varint(r.read_varint()?);
            }
            for _ in 0..count(r, w)? {
                slot(t, r, w, 0)?;
            }
            slot(t, r, w, 0)?;
            slot(t, r, w, 0)?;
        }
        2 => {
            for _ in 0..4 {
                slot(t, r, w, 0)?;
            }
            w.write_varint(r.read_varint()?);
            w.write_f32(r.read_f32()?);
        }
        _ => bail!("recipe display {kind} is not emitted by Solaris"),
    }
    Ok(())
}
fn commands(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    for _ in 0..count(r, w)? {
        let flags = r.read_u8()?;
        w.write_u8(flags);
        for _ in 0..count(r, w)? {
            w.write_varint(r.read_varint()?);
        }
        if flags & 8 != 0 {
            w.write_varint(r.read_varint()?);
        }
        match flags & 3 {
            0 => {}
            1 => string(r, w)?,
            2 => {
                string(r, w)?;
                let parser = r.read_varint()?;
                w.write_varint(
                    t.argument_types
                        .get(parser)
                        .context("unmapped command parser")?,
                );
                let name = t
                    .argument_names
                    .get(parser as usize)
                    .context("unknown command parser")?
                    .as_str();
                match name {
                    "brigadier:float" | "brigadier:integer" | "brigadier:double"
                    | "brigadier:long" => {
                        let flags = r.read_u8()?;
                        w.write_u8(flags);
                        for bit in [1, 2] {
                            if flags & bit != 0 {
                                if matches!(name, "brigadier:double" | "brigadier:long") {
                                    w.write_u64(r.read_u64()?);
                                } else {
                                    w.write_u32(r.read_u32()?);
                                }
                            }
                        }
                    }
                    "brigadier:string" => w.write_varint(r.read_varint()?),
                    "minecraft:entity" | "minecraft:score_holder" => w.write_u8(r.read_u8()?),
                    "minecraft:time" => w.write_i32(r.read_i32()?),
                    "minecraft:resource_or_tag"
                    | "minecraft:resource_or_tag_key"
                    | "minecraft:resource"
                    | "minecraft:resource_key" => string(r, w)?,
                    _ => {} // all other Solaris argument codecs are singletons
                }
                if flags & 16 != 0 {
                    string(r, w)?;
                }
            }
            _ => bail!("invalid command node flags"),
        }
    }
    w.write_varint(r.read_varint()?);
    Ok(())
}
fn statistics(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    let n = r.read_varint()?;
    ensure!((0..=65536).contains(&n), "statistic count");
    let custom = t
        .registry_ids
        .get("custom_stat")
        .context("missing custom stats")?;
    let mut kept = 0;
    let mut entries = Vec::new();
    for _ in 0..n {
        let category = r.read_varint()?;
        let raw = r.read_varint()?;
        let value = r.read_varint()?;
        let ids = match category {
            0 => &t.blocks,
            1..=5 => &t.items,
            6 | 7 => &t.entities,
            8 => custom,
            _ => bail!("unknown statistic category {category}"),
        };
        if let Some(id) = ids.get(raw) {
            entries.write_varint(category);
            entries.write_varint(id);
            entries.write_varint(value);
            kept += 1;
        }
    }
    w.write_varint(kept);
    w.extend(entries);
    Ok(())
}
fn advancements(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_bool(r.read_bool()?);
    for _ in 0..count(r, w)? {
        string(r, w)?;
        let parent = r.read_bool()?;
        w.write_bool(parent);
        if parent {
            string(r, w)?;
        }
        let display = r.read_bool()?;
        w.write_bool(display);
        if display {
            copy_nbt(r, w)?;
            copy_nbt(r, w)?;
            items::template(t, To::Legacy, r, w)?;
            w.write_varint(r.read_varint()?);
            let flags = r.read_i32()?;
            w.write_i32(flags);
            if flags & 1 != 0 {
                string(r, w)?;
            }
        }
        let mut requirements = Vec::new();
        for _ in 0..count(r, &mut requirements)? {
            for _ in 0..count(r, &mut requirements)? {
                string(r, &mut requirements)?;
            }
        }
        requirements.write_bool(r.read_bool()?);
        let x = r.read_f32()?;
        let y = r.read_f32()?;
        if display {
            w.write_f32(x);
            w.write_f32(y);
        }
        w.extend(requirements);
    }
    // Removed identifiers, progress and showAdvancements have no registry ids.
    w.extend_from_slice(r);
    *r = &[];
    Ok(())
}

pub(super) fn clientbound(t: &Tables, name: &str, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    match name {
        "commands" => commands(t, r, w)?,
        "award_stats" => statistics(t, r, w)?,
        "update_recipes" => {
            for _ in 0..count(r, w)? {
                string(r, w)?;
                for _ in 0..count(r, w)? {
                    item(t, r, w)?;
                }
            }
            for _ in 0..count(r, w)? {
                ingredient(t, r, w)?;
                slot(t, r, w, 0)?;
            }
        }
        "recipe_book_add" => {
            for _ in 0..count(r, w)? {
                w.write_varint(r.read_varint()?);
                recipe(t, r, w)?;
                w.write_varint(r.read_varint()?);
                w.write_varint(r.read_varint()?);
                let requirements = r.read_bool()?;
                w.write_bool(requirements);
                if requirements {
                    for _ in 0..count(r, w)? {
                        ingredient(t, r, w)?;
                    }
                }
                w.write_u8(r.read_u8()?);
            }
            w.write_bool(r.read_bool()?);
        }
        "place_ghost_recipe" => {
            w.write_varint(r.read_varint()?);
            recipe(t, r, w)?;
        }
        "update_advancements" => advancements(t, r, w)?,
        _ => bail!("unknown registry-bearing display {name}"),
    }
    Ok(())
}

#[cfg(test)]
#[path = "displays_tests.rs"]
mod tests;
