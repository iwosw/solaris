use super::{ArmorTrim, BannerPattern, ExtendedItemComponents, FireworkExplosion, Fireworks};
use super::{MAX_POTION_CUSTOM_EFFECTS, MAX_POTION_HIDDEN_DEPTH, PotionEffect};
use crate::{Identifier, ItemEnchantment, items::ItemRegistry};
use mc_nbt::{ListTag, Tag, tag_type};

fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}
fn id(tag: &Tag) -> Result<Identifier, &'static str> {
    if let Tag::String(name) = tag {
        Identifier::parse(name.clone()).map_err(|_| "invalid component identifier")
    } else {
        Err("component identifier must be a string")
    }
}
fn compound(tag: &Tag) -> Result<&[(String, Tag)], &'static str> {
    if let Tag::Compound(fields) = tag {
        Ok(fields)
    } else {
        Err("component must be a compound")
    }
}
fn required<'a>(fields: &'a [(String, Tag)], name: &str) -> Result<&'a Tag, &'static str> {
    field(fields, name).ok_or("missing component field")
}
fn list(elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    })
}
fn enchantments(tag: &Tag) -> Result<Vec<ItemEnchantment>, &'static str> {
    compound(tag)?
        .iter()
        .map(|(name, value)| {
            let Tag::Int(level) = value else {
                return Err("enchantment level must be an int");
            };
            Ok(ItemEnchantment {
                id: Identifier::parse(name.clone()).map_err(|_| "invalid enchantment")?,
                level: *level,
            })
        })
        .collect()
}

const FIREWORK_SHAPES: [&str; 5] = ["small_ball", "large_ball", "star", "creeper", "burst"];

fn colors(fields: &[(String, Tag)], key: &str) -> Result<Vec<i32>, &'static str> {
    match field(fields, key) {
        None => Ok(Vec::new()),
        Some(Tag::IntArray(values)) => Ok(values.clone()),
        Some(Tag::List(values)) => values
            .elements
            .iter()
            .map(|value| {
                if let Tag::Int(value) = value {
                    Ok(*value)
                } else {
                    Err("firework color must be an int")
                }
            })
            .collect(),
        _ => Err("firework colors must be an int array"),
    }
}

fn effect_number(fields: &[(String, Tag)], name: &str, default: i32) -> Result<i32, &'static str> {
    match field(fields, name) {
        None => Ok(default),
        Some(Tag::Int(value)) => Ok(*value),
        Some(Tag::Short(value)) => Ok(i32::from(*value)),
        Some(Tag::Byte(value)) => Ok(i32::from(*value)),
        _ => Err("potion effect number must be an integer"),
    }
}

fn effect_bool(fields: &[(String, Tag)], name: &str, default: bool) -> Result<bool, &'static str> {
    match field(fields, name) {
        None => Ok(default),
        Some(Tag::Byte(value)) => Ok(*value != 0),
        _ => Err("potion effect flag must be a boolean"),
    }
}

fn potion_effect_from_nbt(
    tag: &Tag,
    inherited_id: Option<&Identifier>,
    depth: usize,
) -> Result<PotionEffect, &'static str> {
    if depth >= MAX_POTION_HIDDEN_DEPTH {
        return Err("potion hidden effect depth exceeded");
    }
    let fields = compound(tag)?;
    let effect_id = match inherited_id {
        Some(id) => id.clone(),
        None => id(required(fields, "id")?)?,
    };
    if crate::mob_effects_26_1_2::MobEffect::from_name(effect_id.as_str()).is_none() {
        return Err("unknown potion effect");
    }
    let amplifier = u8::try_from(effect_number(fields, "amplifier", 0)?)
        .map_err(|_| "invalid potion effect amplifier")?;
    let show_particles = effect_bool(fields, "show_particles", true)?;
    let hidden = field(fields, "hidden_effect")
        .map(|tag| potion_effect_from_nbt(tag, Some(&effect_id), depth + 1).map(Box::new))
        .transpose()?;
    Ok(PotionEffect {
        id: effect_id,
        amplifier,
        duration: effect_number(fields, "duration", 0)?,
        ambient: effect_bool(fields, "ambient", false)?,
        show_particles,
        show_icon: effect_bool(fields, "show_icon", show_particles)?,
        hidden,
    })
}

fn potion_effect_to_nbt(
    effect: &PotionEffect,
    inherited_id: Option<&Identifier>,
    depth: usize,
) -> Result<Tag, &'static str> {
    if depth >= MAX_POTION_HIDDEN_DEPTH {
        return Err("potion hidden effect depth exceeded");
    }
    if crate::mob_effects_26_1_2::MobEffect::from_name(effect.id.as_str()).is_none()
        || inherited_id.is_some_and(|id| id != &effect.id)
    {
        return Err("invalid potion effect identifier");
    }
    let mut fields = Vec::new();
    if inherited_id.is_none() {
        fields.push(("id".into(), Tag::String(effect.id.as_str().into())));
    }
    fields.extend([
        ("amplifier".into(), Tag::Int(i32::from(effect.amplifier))),
        ("duration".into(), Tag::Int(effect.duration)),
        ("ambient".into(), Tag::Byte(i8::from(effect.ambient))),
        (
            "show_particles".into(),
            Tag::Byte(i8::from(effect.show_particles)),
        ),
        ("show_icon".into(), Tag::Byte(i8::from(effect.show_icon))),
    ]);
    if let Some(hidden) = &effect.hidden {
        fields.push((
            "hidden_effect".into(),
            potion_effect_to_nbt(hidden, Some(&effect.id), depth + 1)?,
        ));
    }
    Ok(Tag::Compound(fields))
}

impl ExtendedItemComponents {
    /// Decode canonical vanilla component NBT. `fields` is the component compound, not the stack.
    pub fn from_nbt(fields: &[(String, Tag)], items: &ItemRegistry) -> Result<Self, &'static str> {
        let mut out = Self::default();
        match field(fields, super::MAX_DAMAGE_COMPONENT) {
            None => {}
            Some(Tag::Int(value)) => {
                let value = u32::try_from(*value).map_err(|_| "invalid max_damage")?;
                super::validate_max_damage(value)?;
                out.max_damage = Some(value);
            }
            Some(_) => return Err("max_damage must be an int"),
        }
        if let Some(tag) = field(fields, super::CUSTOM_DATA_COMPONENT) {
            out.custom_data = Some(super::custom_data_from_tag(tag.clone())?);
        }
        if let Some(potion) = field(fields, "minecraft:potion_contents") {
            if let Tag::String(_) = potion {
                out.potion = Some(id(potion)?);
            } else {
                let fields = compound(potion)?;
                out.potion = field(fields, "potion").map(id).transpose()?;
                out.potion_custom_color = match field(fields, "custom_color") {
                    None => None,
                    Some(Tag::Int(color)) => Some(*color),
                    _ => return Err("potion custom color must be an int"),
                };
                if let Some(effects) = field(fields, "custom_effects") {
                    let Tag::List(effects) = effects else {
                        return Err("potion custom effects must be a list");
                    };
                    if effects.elements.len() > MAX_POTION_CUSTOM_EFFECTS {
                        return Err("too many potion custom effects");
                    }
                    out.potion_custom_effects = effects
                        .elements
                        .iter()
                        .map(|tag| potion_effect_from_nbt(tag, None, 0))
                        .collect::<Result<_, _>>()?;
                }
                out.potion_custom_name = match field(fields, "custom_name") {
                    None => None,
                    Some(Tag::String(name)) => Some(name.clone()),
                    _ => return Err("potion custom name must be a string"),
                };
            }
        }
        if let Some(Tag::Int(amplifier)) = field(fields, "minecraft:ominous_bottle_amplifier") {
            if !(0..=4).contains(amplifier) {
                return Err("invalid ominous bottle amplifier");
            }
            out.ominous_bottle_amplifier = Some(*amplifier);
        }
        if let Some(fireworks) = field(fields, "minecraft:fireworks") {
            let fireworks = compound(fireworks)?;
            let flight_duration = match field(fireworks, "flight_duration") {
                Some(Tag::Byte(value)) => i32::from(*value as u8),
                Some(Tag::Int(value)) => *value,
                None => 0,
                _ => return Err("invalid firework flight duration"),
            };
            if !(0..=255).contains(&flight_duration) {
                return Err("invalid firework flight duration");
            }
            let mut explosions = Vec::new();
            if let Some(Tag::List(values)) = field(fireworks, "explosions") {
                if values.elements.len() > 256 {
                    return Err("too many firework explosions");
                }
                for value in &values.elements {
                    let value = compound(value)?;
                    let Tag::String(shape) = required(value, "shape")? else {
                        return Err("invalid firework shape");
                    };
                    let shape = FIREWORK_SHAPES
                        .iter()
                        .position(|name| *name == shape)
                        .ok_or("invalid firework shape")? as i32;
                    explosions.push(FireworkExplosion {
                        shape, colors: colors(value, "colors")?, fade_colors: colors(value, "fade_colors")?,
                        has_trail: matches!(field(value, "has_trail"), Some(Tag::Byte(value)) if *value != 0),
                        has_twinkle: matches!(field(value, "has_twinkle"), Some(Tag::Byte(value)) if *value != 0),
                    });
                }
            }
            out.fireworks = Some(Fireworks {
                flight_duration,
                explosions,
            });
        }
        if let Some(trim) = field(fields, "minecraft:trim") {
            let trim = compound(trim)?;
            out.trim = Some(ArmorTrim {
                material: id(required(trim, "material")?)?,
                pattern: id(required(trim, "pattern")?)?,
            });
        }
        if let Some(Tag::Int(cost)) = field(fields, "minecraft:repair_cost") {
            out.repair_cost = (*cost).max(0);
        }
        if let Some(enchant) = field(fields, "minecraft:stored_enchantments") {
            out.stored_enchantments = enchantments(enchant)?;
        }
        if let Some(Tag::List(layers)) = field(fields, "minecraft:banner_patterns") {
            for layer in &layers.elements {
                let layer = compound(layer)?;
                let Tag::String(color) = required(layer, "color")? else {
                    return Err("banner color must be a string");
                };
                let color = super::super::stations::DYE_COLORS
                    .iter()
                    .position(|name| *name == color)
                    .ok_or("invalid banner color")?;
                out.banner_patterns.push(BannerPattern {
                    pattern: id(required(layer, "pattern")?)?,
                    color: color as i32,
                });
            }
        }
        if let Some(projectiles) = field(fields, "minecraft:charged_projectiles") {
            let Tag::List(projectiles) = projectiles else {
                return Err("charged projectiles must be a list");
            };
            if projectiles.elements.len() > 3 {
                return Err("too many charged projectiles");
            }
            for projectile in &projectiles.elements {
                let projectile = compound(projectile)?;
                if let Some(Tag::Compound(c)) = field(projectile, "components")
                    && field(c, "minecraft:charged_projectiles").is_some()
                {
                    return Err("nested charged projectiles");
                }
                let stack = crate::item_nbt::stack_from_fields(items, projectile)
                    .map_err(|_| "invalid charged item")?
                    .ok_or("missing charged item")?;
                out.charged_projectiles.push(stack);
            }
        }
        Ok(out)
    }

    /// Encode station components using vanilla names and stack IDs. Empty components are omitted.
    pub fn to_nbt(&self, items: &ItemRegistry) -> Result<Vec<(String, Tag)>, &'static str> {
        let mut out = Vec::new();
        if let Some(value) = self.max_damage {
            super::validate_max_damage(value)?;
            out.push((super::MAX_DAMAGE_COMPONENT.into(), Tag::Int(value as i32)));
        }
        if let Some(bytes) = &self.custom_data {
            out.push((
                super::CUSTOM_DATA_COMPONENT.into(),
                super::decode_custom_data(bytes)?,
            ));
        }
        if self.has_potion_contents() {
            if self.potion_custom_effects.len() > MAX_POTION_CUSTOM_EFFECTS {
                return Err("too many potion custom effects");
            }
            let mut fields = Vec::new();
            if let Some(potion) = &self.potion {
                fields.push(("potion".into(), Tag::String(potion.as_str().into())));
            }
            if let Some(color) = self.potion_custom_color {
                fields.push(("custom_color".into(), Tag::Int(color)));
            }
            if !self.potion_custom_effects.is_empty() {
                fields.push((
                    "custom_effects".into(),
                    list(
                        self.potion_custom_effects
                            .iter()
                            .map(|effect| potion_effect_to_nbt(effect, None, 0))
                            .collect::<Result<_, _>>()?,
                    ),
                ));
            }
            if let Some(name) = &self.potion_custom_name {
                fields.push(("custom_name".into(), Tag::String(name.clone())));
            }
            out.push(("minecraft:potion_contents".into(), Tag::Compound(fields)));
        }
        if let Some(amplifier) = self.ominous_bottle_amplifier {
            if !(0..=4).contains(&amplifier) {
                return Err("invalid ominous bottle amplifier");
            }
            out.push((
                "minecraft:ominous_bottle_amplifier".into(),
                Tag::Int(amplifier),
            ));
        }
        if let Some(fireworks) = &self.fireworks {
            if !(0..=255).contains(&fireworks.flight_duration) || fireworks.explosions.len() > 256 {
                return Err("invalid fireworks");
            }
            let explosions = fireworks
                .explosions
                .iter()
                .map(|explosion| {
                    let shape = usize::try_from(explosion.shape)
                        .ok()
                        .and_then(|index| FIREWORK_SHAPES.get(index))
                        .ok_or("invalid firework shape")?;
                    Ok(Tag::Compound(vec![
                        ("shape".into(), Tag::String((*shape).into())),
                        ("colors".into(), Tag::IntArray(explosion.colors.clone())),
                        (
                            "fade_colors".into(),
                            Tag::IntArray(explosion.fade_colors.clone()),
                        ),
                        ("has_trail".into(), Tag::Byte(i8::from(explosion.has_trail))),
                        (
                            "has_twinkle".into(),
                            Tag::Byte(i8::from(explosion.has_twinkle)),
                        ),
                    ]))
                })
                .collect::<Result<Vec<_>, &'static str>>()?;
            out.push((
                "minecraft:fireworks".into(),
                Tag::Compound(vec![
                    (
                        "flight_duration".into(),
                        Tag::Byte(fireworks.flight_duration as i8),
                    ),
                    ("explosions".into(), list(explosions)),
                ]),
            ));
        }
        if let Some(trim) = &self.trim {
            out.push((
                "minecraft:trim".into(),
                Tag::Compound(vec![
                    (
                        "material".into(),
                        Tag::String(trim.material.as_str().into()),
                    ),
                    ("pattern".into(), Tag::String(trim.pattern.as_str().into())),
                ]),
            ));
        }
        if self.repair_cost != 0 {
            out.push(("minecraft:repair_cost".into(), Tag::Int(self.repair_cost)));
        }
        if !self.stored_enchantments.is_empty() {
            out.push((
                "minecraft:stored_enchantments".into(),
                Tag::Compound(
                    self.stored_enchantments
                        .iter()
                        .map(|e| (e.id.as_str().into(), Tag::Int(e.level)))
                        .collect(),
                ),
            ));
        }
        if !self.banner_patterns.is_empty() {
            let layers = self
                .banner_patterns
                .iter()
                .map(|layer| {
                    let color = usize::try_from(layer.color)
                        .ok()
                        .and_then(|i| super::super::stations::DYE_COLORS.get(i))
                        .ok_or("invalid banner color")?;
                    Ok(Tag::Compound(vec![
                        ("pattern".into(), Tag::String(layer.pattern.as_str().into())),
                        ("color".into(), Tag::String((*color).into())),
                    ]))
                })
                .collect::<Result<Vec<_>, &'static str>>()?;
            out.push(("minecraft:banner_patterns".into(), list(layers)));
        }
        if !self.charged_projectiles.is_empty() {
            let projectiles = self
                .charged_projectiles
                .iter()
                .map(|stack| {
                    crate::item_nbt::stack_to_fields(items, stack)
                        .map(Tag::Compound)
                        .map_err(|_| "invalid charged item")
                })
                .collect::<Result<Vec<_>, &'static str>>()?;
            out.push(("minecraft:charged_projectiles".into(), list(projectiles)));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_potion_contents_round_trip_with_hidden_details_and_without_base_potion() {
        let items = crate::items::solaris_required_items();
        let effect = PotionEffect {
            id: Identifier::parse("minecraft:poison").unwrap(),
            amplifier: 255,
            duration: -1,
            ambient: true,
            show_particles: false,
            show_icon: true,
            hidden: None,
        };
        let mut contents = ExtendedItemComponents {
            potion_custom_color: Some(0x123456),
            potion_custom_effects: vec![PotionEffect {
                hidden: Some(Box::new(effect.clone())),
                ..effect
            }],
            potion_custom_name: Some("custom".into()),
            ..ExtendedItemComponents::EMPTY
        };
        for base in [
            None,
            Some(Identifier::parse("minecraft:long_poison").unwrap()),
        ] {
            contents.potion = base;
            let nbt = contents.to_nbt(&items).unwrap();
            assert_eq!(
                ExtendedItemComponents::from_nbt(&nbt, &items).unwrap(),
                contents
            );
        }
    }

    #[test]
    fn potion_effect_nbt_defaults_and_bounds_match_vanilla() {
        let items = crate::items::solaris_required_items();
        let fields = vec![(
            "minecraft:potion_contents".into(),
            Tag::Compound(vec![(
                "custom_effects".into(),
                list(vec![Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:speed".into())),
                    ("show_particles".into(), Tag::Byte(0)),
                ])]),
            )]),
        )];
        let components = ExtendedItemComponents::from_nbt(&fields, &items).unwrap();
        let effect = &components.potion_custom_effects[0];
        assert_eq!(
            (effect.amplifier, effect.duration, effect.ambient),
            (0, 0, false)
        );
        assert!(!effect.show_particles && !effect.show_icon);
        for amplifier in [-1, 256] {
            assert!(
                potion_effect_from_nbt(
                    &Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:speed".into())),
                        ("amplifier".into(), Tag::Int(amplifier)),
                    ]),
                    None,
                    0
                )
                .is_err()
            );
        }
        let excessive = vec![(
            "minecraft:potion_contents".into(),
            Tag::Compound(vec![(
                "custom_effects".into(),
                list(vec![
                    Tag::Compound(Vec::new());
                    MAX_POTION_CUSTOM_EFFECTS + 1
                ]),
            )]),
        )];
        assert_eq!(
            ExtendedItemComponents::from_nbt(&excessive, &items).unwrap_err(),
            "too many potion custom effects"
        );
        let mut deep = Tag::Compound(Vec::new());
        for _ in 0..MAX_POTION_HIDDEN_DEPTH {
            deep = Tag::Compound(vec![("hidden_effect".into(), deep)]);
        }
        assert_eq!(
            potion_effect_from_nbt(
                &deep,
                Some(&Identifier::parse("minecraft:speed").unwrap()),
                0
            )
            .unwrap_err(),
            "potion hidden effect depth exceeded"
        );
    }
}
