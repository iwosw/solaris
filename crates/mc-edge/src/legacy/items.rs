//! Item stacks on the wire: item ids and data-component type ids remapped,
//! component payloads copied. Only components with a known payload shape can
//! be walked; unknown shapes fail closed instead of dropping an inventory
//! update. Components 26.1.2 lacks are omitted after consuming their payload.

use anyhow::{Context, Result, bail, ensure};
use mc_protocol::codec::{ReadMc, WriteMc};

use super::tables::{IdMap, Tables};

const MAX_DEPTH: usize = 8;
const MAX_LIST: i32 = 256;

/// Translation direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum To {
    Legacy,
    Canonical,
}

/// Payload shape of a data component (same in 26.1.2 and 26.3 for these).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload {
    Empty,
    VarInt,
    Bool,
    I32,
    Nbt,
    NbtList,
    Identifier,
    Enchantments,
    TooltipDisplay,
    PotionContents,
    BlockState,
    Template,
    TemplateList,
    Container,
    PotDecorations,
    DeathProtection,
    StewEffects,
    WritableBook,
    WrittenBook,
    Trim,
    BannerPatterns,
    Fireworks,
    Profile,
    Bees,
}

/// Known payload shapes by component name (namespace stripped).
pub fn payload(name: &str) -> Option<Payload> {
    use Payload::*;
    Some(match name.strip_prefix("minecraft:").unwrap_or(name) {
        "unbreakable" | "creative_slot_lock" | "intangible_projectile" | "glider" => Empty,
        "max_stack_size"
        | "max_damage"
        | "damage"
        | "rarity"
        | "repair_cost"
        | "map_id"
        | "map_post_processing"
        | "ominous_bottle_amplifier"
        | "base_color" => VarInt,
        "enchantment_glint_override" => Bool,
        "dyed_color" | "map_color" => I32,
        "custom_data" | "custom_name" | "item_name" | "lock" | "container_loot" => Nbt,
        "lore" => NbtList,
        "item_model" | "tooltip_style" | "note_block_sound" => Identifier,
        "enchantments" | "stored_enchantments" => Enchantments,
        "tooltip_display" => TooltipDisplay,
        "potion_contents" => PotionContents,
        "block_state" => BlockState,
        "use_remainder" => Template,
        "bundle_contents" | "charged_projectiles" => TemplateList,
        "container" => Container,
        "pot_decorations" => PotDecorations,
        "death_protection" => DeathProtection,
        "suspicious_stew_effects" => StewEffects,
        "writable_book_content" => WritableBook,
        "written_book_content" => WrittenBook,
        "trim" => Trim,
        "banner_patterns" => BannerPatterns,
        "fireworks" => Fireworks,
        "profile" => Profile,
        "bees" => Bees,
        _ => return None,
    })
}

/// Component payload shapes indexed by wire id, for both sides.
#[derive(Debug, Default)]
pub struct Components {
    canonical: Vec<Option<Payload>>,
    legacy: Vec<Option<Payload>>,
}

impl Components {
    pub fn new(canonical: &[String], legacy: &[String]) -> Self {
        let shapes = |names: &[String]| names.iter().map(|name| payload(name)).collect();
        Self {
            canonical: shapes(canonical),
            legacy: shapes(legacy),
        }
    }

    fn shape(&self, to: To, id: i32) -> Option<Payload> {
        let source = match to {
            To::Legacy => &self.canonical,
            To::Canonical => &self.legacy,
        };
        usize::try_from(id)
            .ok()
            .and_then(|id| source.get(id).copied().flatten())
    }
}

fn map(ids: &IdMap, to: To, id: i32) -> Option<i32> {
    match to {
        To::Legacy => ids.get(id),
        To::Canonical => ids.back(id),
    }
}

/// Copy one `ItemStack` (optional-stack codec: count 0 = empty).
pub fn stack(t: &Tables, to: To, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    stack_at(t, to, r, w, 0)
}

fn stack_at(t: &Tables, to: To, r: &mut &[u8], w: &mut Vec<u8>, depth: usize) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "item stack nesting too deep");
    let count = r.read_varint()?;
    w.write_varint(count);
    if count <= 0 {
        return Ok(());
    }
    let item = r.read_varint()?;
    w.write_varint(match to {
        To::Legacy => t.items.get(item).unwrap_or(1),
        To::Canonical => t.items.back(item).context("item id has no 26.3 entry")?,
    });
    patch(t, to, r, w, depth)
}

/// Non-optional ItemStackTemplate: item id, count, then a component patch.
pub(super) fn template(t: &Tables, to: To, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    template_at(t, to, r, w, 0)
}

fn template_at(t: &Tables, to: To, r: &mut &[u8], w: &mut Vec<u8>, depth: usize) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "item template nesting too deep");
    let item = r.read_varint()?;
    w.write_varint(map(&t.items, to, item).context("unmapped template item")?);
    w.write_varint(r.read_varint()?);
    patch(t, to, r, w, depth)
}

fn patch(t: &Tables, to: To, r: &mut &[u8], w: &mut Vec<u8>, depth: usize) -> Result<()> {
    let added = r.read_varint()?;
    let removed = r.read_varint()?;
    ensure!(
        (0..=MAX_LIST).contains(&added) && (0..=MAX_LIST).contains(&removed),
        "component count"
    );
    let mut body = Vec::new();
    let mut kept_added = 0;
    for _ in 0..added {
        let id = r.read_varint()?;
        let shape = t
            .component_shapes
            .shape(to, id)
            .with_context(|| format!("unsupported item component {id}"))?;
        let target = map(&t.components, to, id);
        let mut payload_out = Vec::new();
        copy_payload(t, to, shape, r, &mut payload_out, depth)?;
        if let Some(target) = target {
            body.write_varint(target);
            body.extend_from_slice(&payload_out);
            kept_added += 1;
        }
    }
    let mut kept_removed = 0;
    let mut removed_ids = Vec::new();
    for _ in 0..removed {
        if let Some(target) = map(&t.components, to, r.read_varint()?) {
            removed_ids.write_varint(target);
            kept_removed += 1;
        }
    }
    w.write_varint(kept_added);
    w.write_varint(kept_removed);
    w.extend_from_slice(&body);
    w.extend_from_slice(&removed_ids);
    Ok(())
}

fn copy_payload(
    t: &Tables,
    to: To,
    shape: Payload,
    r: &mut &[u8],
    w: &mut Vec<u8>,
    depth: usize,
) -> Result<()> {
    match shape {
        Payload::Empty => {}
        Payload::VarInt => w.write_varint(r.read_varint()?),
        Payload::Bool => w.write_bool(r.read_bool()?),
        Payload::I32 => w.write_i32(r.read_i32()?),
        Payload::Nbt => copy_nbt(r, w)?,
        Payload::NbtList => {
            let n = list_len(r, w)?;
            for _ in 0..n {
                copy_nbt(r, w)?;
            }
        }
        Payload::Identifier => w.write_identifier(&r.read_identifier()?)?,
        Payload::Enchantments => {
            let n = list_len(r, w)?;
            for _ in 0..n {
                registry(t, to, "enchantment", r, w, false)?;
                w.write_varint(r.read_varint()?);
            }
        }
        Payload::TooltipDisplay => {
            w.write_bool(r.read_bool()?);
            let n = r.read_varint()?;
            ensure!((0..=MAX_LIST).contains(&n), "tooltip display length");
            let ids: Vec<i32> = (0..n).map(|_| r.read_varint()).collect::<Result<_, _>>()?;
            let ids: Vec<i32> = ids
                .into_iter()
                .filter_map(|id| map(&t.components, to, id))
                .collect();
            w.write_varint(ids.len() as i32);
            for id in ids {
                w.write_varint(id);
            }
        }
        Payload::PotionContents => {
            if r.read_bool()? {
                w.write_bool(true);
                registry(t, to, "potion", r, w, false)?;
            } else {
                w.write_bool(false);
            }
            if r.read_bool()? {
                w.write_bool(true);
                w.write_i32(r.read_i32()?);
            } else {
                w.write_bool(false);
            }
            let n = list_len(r, w)?;
            for _ in 0..n {
                registry(t, to, "mob_effect", r, w, false)?;
                copy_effect_details(r, w, 0)?;
            }
            if r.read_bool()? {
                w.write_bool(true);
                w.write_string(&r.read_string(32767)?, 32767)?;
            } else {
                w.write_bool(false);
            }
        }
        Payload::BlockState => {
            let n = list_len(r, w)?;
            for _ in 0..n {
                w.write_string(&r.read_string(32767)?, 32767)?;
                w.write_string(&r.read_string(32767)?, 32767)?;
            }
        }
        Payload::Template => template_at(t, to, r, w, depth + 1)?,
        Payload::TemplateList => {
            let n = list_len(r, w)?;
            for _ in 0..n {
                template_at(t, to, r, w, depth + 1)?;
            }
        }
        other => extra(t, to, other, r, w, depth)?,
    }
    Ok(())
}

fn copy_effect_details(r: &mut &[u8], w: &mut Vec<u8>, depth: usize) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "hidden effect nesting too deep");
    w.write_varint(r.read_varint()?);
    w.write_varint(r.read_varint()?);
    for _ in 0..3 {
        w.write_bool(r.read_bool()?);
    }
    if r.read_bool()? {
        w.write_bool(true);
        copy_effect_details(r, w, depth + 1)?;
    } else {
        w.write_bool(false);
    }
    Ok(())
}

fn list_len(r: &mut &[u8], w: &mut Vec<u8>) -> Result<i32> {
    let n = r.read_varint()?;
    ensure!((0..=MAX_LIST).contains(&n), "list length {n}");
    w.write_varint(n);
    Ok(n)
}

/// Copy one network-NBT value verbatim.
pub fn copy_nbt(r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    let before = *r;
    mc_nbt::read_network_value(r)?;
    let used = before.len() - r.len();
    w.extend_from_slice(&before[..used]);
    Ok(())
}

/// Copy a `HashedStack` (container clicks): optional item id + count +
/// hashed component patch, ids translated legacy -> canonical.
pub fn hashed_stack(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    let present = r.read_bool()?;
    w.write_bool(present);
    if !present {
        return Ok(());
    }
    let item = r.read_varint()?;
    w.write_varint(t.items.back(item).context("item id has no 26.3 entry")?);
    w.write_varint(r.read_varint()?);
    let added = r.read_varint()?;
    ensure!((0..=MAX_LIST).contains(&added), "hashed component count");
    let mut entries = Vec::new();
    for _ in 0..added {
        let id = r.read_varint()?;
        let hash = r.read_i32()?;
        if let Some(id) = t.components.back(id) {
            entries.push((id, hash));
        }
    }
    w.write_varint(entries.len() as i32);
    for (id, hash) in entries {
        w.write_varint(id);
        w.write_i32(hash);
    }
    let removed = r.read_varint()?;
    ensure!((0..=MAX_LIST).contains(&removed), "hashed removal count");
    let ids: Vec<i32> = (0..removed)
        .map(|_| r.read_varint())
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|id| t.components.back(id))
        .collect();
    w.write_varint(ids.len() as i32);
    for id in ids {
        w.write_varint(id);
    }
    Ok(())
}

/// Bail helper for walkers that hit a shape they cannot skip.
pub fn unsupported<T>(what: &str) -> Result<T> {
    bail!("unsupported {what}")
}

mod payloads;
use payloads::{extra, registry};

#[cfg(test)]
#[path = "items_tests.rs"]
mod tests;
