use super::{item_stack_template, read_bounded_vec, write_count};
use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;
use bytes::{Buf, BufMut};
use mc_data::data_component_types_26_1_2 as ids;
use mc_data::item_stack::{
    ArmorTrim, BannerPattern, ExtendedItemComponents, FireworkExplosion, Fireworks,
};
use mc_data::item_stack::{MAX_POTION_CUSTOM_EFFECTS, MAX_POTION_HIDDEN_DEPTH, PotionEffect};
use mc_data::{Identifier, ItemEnchantment};

pub(super) fn count(c: &ExtendedItemComponents, overrides: &[(i32, mc_nbt::Tag)]) -> i32 {
    use super::copied_components::contains;
    i32::from(c.has_potion_contents() && !contains(overrides, ids::POTION_CONTENTS))
        + i32::from(c.trim.is_some() && !contains(overrides, ids::TRIM))
        + i32::from(!c.banner_patterns.is_empty() && !contains(overrides, ids::BANNER_PATTERNS))
        + i32::from(c.repair_cost != 0 && !contains(overrides, ids::REPAIR_COST))
        + i32::from(
            !c.stored_enchantments.is_empty() && !contains(overrides, ids::STORED_ENCHANTMENTS),
        )
        + i32::from(
            !c.charged_projectiles.is_empty() && !contains(overrides, ids::CHARGED_PROJECTILES),
        )
        + i32::from(c.fireworks.is_some() && !contains(overrides, ids::FIREWORKS))
        + i32::from(
            c.ominous_bottle_amplifier.is_some()
                && !contains(overrides, ids::OMINOUS_BOTTLE_AMPLIFIER),
        )
}

fn write_holder<B: BufMut>(buf: &mut B, registry: &str, id: &Identifier) -> Result<(), CodecError> {
    let raw = mc_data::required_registry_entry_id(registry, id)
        .ok_or(CodecError::NotSupported("unknown station registry entry"))?;
    buf.write_varint(raw as i32 + 1);
    Ok(())
}

fn read_holder<B: Buf>(buf: &mut B, registry: &str) -> Result<Identifier, CodecError> {
    let raw = buf.read_varint()?;
    raw.checked_sub(1)
        .and_then(|raw| u32::try_from(raw).ok())
        .and_then(|raw| mc_data::required_registry_entry(registry, raw))
        .ok_or(CodecError::NotSupported("unknown station registry holder"))
}

fn encode_effect_details<B: BufMut>(
    buf: &mut B,
    effect: &PotionEffect,
    depth: usize,
) -> Result<(), CodecError> {
    if depth >= MAX_POTION_HIDDEN_DEPTH {
        return Err(CodecError::NotSupported(
            "potion hidden effect depth exceeded",
        ));
    }
    buf.write_varint(i32::from(effect.amplifier));
    buf.write_varint(effect.duration);
    buf.write_bool(effect.ambient);
    buf.write_bool(effect.show_particles);
    buf.write_bool(effect.show_icon);
    buf.write_bool(effect.hidden.is_some());
    if let Some(hidden) = &effect.hidden {
        if hidden.id != effect.id {
            return Err(CodecError::NotSupported(
                "hidden potion effect identifier differs",
            ));
        }
        encode_effect_details(buf, hidden, depth + 1)?;
    }
    Ok(())
}

fn decode_effect_details<B: Buf>(
    buf: &mut B,
    id: Identifier,
    depth: usize,
) -> Result<PotionEffect, CodecError> {
    if depth >= MAX_POTION_HIDDEN_DEPTH {
        return Err(CodecError::NotSupported(
            "potion hidden effect depth exceeded",
        ));
    }
    let amplifier = u8::try_from(buf.read_varint()?)
        .map_err(|_| CodecError::NotSupported("invalid potion effect amplifier"))?;
    let duration = buf.read_varint()?;
    let ambient = buf.read_bool()?;
    let show_particles = buf.read_bool()?;
    let show_icon = buf.read_bool()?;
    let hidden = if buf.read_bool()? {
        Some(Box::new(decode_effect_details(buf, id.clone(), depth + 1)?))
    } else {
        None
    };
    Ok(PotionEffect {
        id,
        amplifier,
        duration,
        ambient,
        show_particles,
        show_icon,
        hidden,
    })
}

pub(super) fn encode<B: BufMut>(
    buf: &mut B,
    c: &ExtendedItemComponents,
    overrides: &[(i32, mc_nbt::Tag)],
    depth: usize,
) -> Result<(), CodecError> {
    use super::copied_components::contains;
    if c.repair_cost != 0 && !contains(overrides, ids::REPAIR_COST) {
        buf.write_varint(ids::REPAIR_COST);
        buf.write_varint(c.repair_cost);
    }
    if !c.stored_enchantments.is_empty() && !contains(overrides, ids::STORED_ENCHANTMENTS) {
        buf.write_varint(ids::STORED_ENCHANTMENTS);
        write_count(buf, c.stored_enchantments.len())?;
        for e in &c.stored_enchantments {
            let raw = mc_data::required_registry_entry_id("enchantment", &e.id)
                .ok_or(CodecError::NotSupported("unknown stored enchantment"))?;
            buf.write_varint(raw as i32);
            buf.write_varint(e.level);
        }
    }
    if !c.charged_projectiles.is_empty() && !contains(overrides, ids::CHARGED_PROJECTILES) {
        buf.write_varint(ids::CHARGED_PROJECTILES);
        write_count(buf, c.charged_projectiles.len())?;
        for stack in &c.charged_projectiles {
            item_stack_template::encode(buf, stack, depth + 1)?;
        }
    }
    if c.has_potion_contents() && !contains(overrides, ids::POTION_CONTENTS) {
        if c.potion_custom_effects.len() > MAX_POTION_CUSTOM_EFFECTS {
            return Err(CodecError::StringTooLong {
                len: c.potion_custom_effects.len(),
                max: MAX_POTION_CUSTOM_EFFECTS,
            });
        }
        buf.write_varint(ids::POTION_CONTENTS);
        buf.write_bool(c.potion.is_some());
        if let Some(potion) = &c.potion {
            let raw = mc_data::stations::potion_id(potion)
                .ok_or(CodecError::NotSupported("unknown potion"))?;
            buf.write_varint(raw as i32);
        }
        buf.write_bool(c.potion_custom_color.is_some());
        if let Some(color) = c.potion_custom_color {
            buf.write_i32(color);
        }
        write_count(buf, c.potion_custom_effects.len())?;
        for effect in &c.potion_custom_effects {
            let raw = mc_data::mob_effects_26_1_2::MobEffect::from_name(effect.id.as_str())
                .ok_or(CodecError::NotSupported("unknown potion effect"))?;
            buf.write_varint(raw as i32);
            encode_effect_details(buf, effect, 0)?;
        }
        buf.write_bool(c.potion_custom_name.is_some());
        if let Some(name) = &c.potion_custom_name {
            buf.write_string(name, 32767)?;
        }
    }
    if let Some(trim) = &c.trim
        && !contains(overrides, ids::TRIM)
    {
        buf.write_varint(ids::TRIM);
        write_holder(buf, "trim_material", &trim.material)?;
        write_holder(buf, "trim_pattern", &trim.pattern)?;
    }
    if !c.banner_patterns.is_empty() && !contains(overrides, ids::BANNER_PATTERNS) {
        buf.write_varint(ids::BANNER_PATTERNS);
        write_count(buf, c.banner_patterns.len())?;
        for layer in &c.banner_patterns {
            write_holder(buf, "banner_pattern", &layer.pattern)?;
            buf.write_varint(layer.color);
        }
    }
    if let Some(amplifier) = c.ominous_bottle_amplifier
        && !contains(overrides, ids::OMINOUS_BOTTLE_AMPLIFIER)
    {
        if !(0..=4).contains(&amplifier) {
            return Err(CodecError::NotSupported("invalid ominous bottle amplifier"));
        }
        buf.write_varint(ids::OMINOUS_BOTTLE_AMPLIFIER);
        buf.write_varint(amplifier);
    }
    if let Some(fireworks) = &c.fireworks
        && !contains(overrides, ids::FIREWORKS)
    {
        if !(0..=255).contains(&fireworks.flight_duration) || fireworks.explosions.len() > 256 {
            return Err(CodecError::NotSupported("invalid fireworks"));
        }
        buf.write_varint(ids::FIREWORKS);
        buf.write_varint(fireworks.flight_duration);
        write_count(buf, fireworks.explosions.len())?;
        for explosion in &fireworks.explosions {
            if !(0..=4).contains(&explosion.shape) {
                return Err(CodecError::NotSupported("invalid firework shape"));
            }
            buf.write_varint(explosion.shape);
            write_count(buf, explosion.colors.len())?;
            for color in &explosion.colors {
                buf.put_i32(*color);
            }
            write_count(buf, explosion.fade_colors.len())?;
            for color in &explosion.fade_colors {
                buf.put_i32(*color);
            }
            buf.put_u8(u8::from(explosion.has_trail));
            buf.put_u8(u8::from(explosion.has_twinkle));
        }
    }
    Ok(())
}

pub(super) fn decode<B: Buf>(
    buf: &mut B,
    id: i32,
    c: &mut ExtendedItemComponents,
    depth: usize,
) -> Result<bool, CodecError> {
    match id {
        ids::REPAIR_COST => {
            c.repair_cost = buf.read_varint()?.max(0);
        }
        ids::STORED_ENCHANTMENTS => {
            c.stored_enchantments = read_bounded_vec(buf, 256, 2, |buf| {
                let raw = buf.read_varint()?;
                let id = u32::try_from(raw)
                    .ok()
                    .and_then(|raw| mc_data::required_registry_entry("enchantment", raw))
                    .ok_or(CodecError::NotSupported("unknown stored enchantment"))?;
                let level = buf.read_varint()?;
                if !(1..=255).contains(&level) {
                    return Err(CodecError::NotSupported("invalid enchantment level"));
                }
                Ok(ItemEnchantment { id, level })
            })?;
        }
        ids::CHARGED_PROJECTILES => {
            if depth >= super::copied_components::MAX_ITEM_COMPONENT_DEPTH {
                return Err(CodecError::NotSupported("nested item component depth"));
            }
            c.charged_projectiles = read_bounded_vec(buf, 1024, 4, |buf| {
                item_stack_template::decode(buf, depth + 1)
            })?;
        }
        ids::POTION_CONTENTS => {
            if buf.read_bool()? {
                let raw = buf.read_varint()?;
                c.potion = Some(
                    u32::try_from(raw)
                        .ok()
                        .and_then(mc_data::stations::potion_name)
                        .ok_or(CodecError::NotSupported("unknown potion"))?,
                );
            }
            c.potion_custom_color = if buf.read_bool()? {
                Some(buf.read_i32()?)
            } else {
                None
            };
            c.potion_custom_effects = read_bounded_vec(buf, MAX_POTION_CUSTOM_EFFECTS, 7, |buf| {
                let raw = buf.read_varint()?;
                let name = u32::try_from(raw)
                    .ok()
                    .and_then(mc_data::mob_effects_26_1_2::name_for_id)
                    .ok_or(CodecError::NotSupported("unknown potion effect"))?;
                decode_effect_details(
                    buf,
                    Identifier::parse(name).expect("canonical effect id"),
                    0,
                )
            })?;
            c.potion_custom_name = if buf.read_bool()? {
                Some(buf.read_string(32767)?)
            } else {
                None
            };
        }
        ids::TRIM => {
            c.trim = Some(ArmorTrim {
                material: read_holder(buf, "trim_material")?,
                pattern: read_holder(buf, "trim_pattern")?,
            });
        }
        ids::BANNER_PATTERNS => {
            c.banner_patterns = read_bounded_vec(buf, 20, 2, |buf| {
                let pattern = read_holder(buf, "banner_pattern")?;
                let color = buf.read_varint()?;
                if !(0..16).contains(&color) {
                    return Err(CodecError::NotSupported("invalid banner color"));
                }
                Ok(BannerPattern { pattern, color })
            })?;
        }
        ids::OMINOUS_BOTTLE_AMPLIFIER => {
            let amplifier = buf.read_varint()?;
            if !(0..=4).contains(&amplifier) {
                return Err(CodecError::NotSupported("invalid ominous bottle amplifier"));
            }
            c.ominous_bottle_amplifier = Some(amplifier);
        }
        ids::FIREWORKS => {
            let flight_duration = buf.read_varint()?;
            if !(0..=255).contains(&flight_duration) {
                return Err(CodecError::NotSupported("invalid firework flight duration"));
            }
            let explosions = read_bounded_vec(buf, 256, 5, |buf| {
                let shape = buf.read_varint()?;
                if !(0..=4).contains(&shape) {
                    return Err(CodecError::NotSupported("invalid firework shape"));
                }
                let colors = read_bounded_vec(buf, 16_384, 4, |buf| {
                    if buf.remaining() < 4 {
                        return Err(CodecError::Underflow {
                            needed: 4,
                            available: buf.remaining(),
                        });
                    }
                    Ok(buf.get_i32())
                })?;
                let fade_colors = read_bounded_vec(buf, 16_384, 4, |buf| {
                    if buf.remaining() < 4 {
                        return Err(CodecError::Underflow {
                            needed: 4,
                            available: buf.remaining(),
                        });
                    }
                    Ok(buf.get_i32())
                })?;
                Ok(FireworkExplosion {
                    shape,
                    colors,
                    fade_colors,
                    has_trail: buf.read_bool()?,
                    has_twinkle: buf.read_bool()?,
                })
            })?;
            c.fireworks = Some(Fireworks {
                flight_duration,
                explosions,
            });
        }
        _ => return Ok(false),
    }
    Ok(true)
}
