use std::sync::Arc;

use crate::{Identifier, ItemEnchantment};
mod station_nbt;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StewEffect {
    pub id: Identifier,
    #[serde(default = "default_stew_duration")]
    pub duration: i32,
}

const fn default_stew_duration() -> i32 {
    160
}

pub const STEW_EFFECTS_COMPONENT: &str = "minecraft:suspicious_stew_effects";

pub const MAX_DAMAGE_COMPONENT: &str = "minecraft:max_damage";
pub const CUSTOM_DATA_COMPONENT: &str = "minecraft:custom_data";
pub const MAX_CUSTOM_DATA_BYTES: usize = 16 * 1024;
pub const MAX_CUSTOM_DATA_DEPTH: usize = 32;

/// DyedItemColor.CODEC uses ExtraCodecs.RGB_COLOR_CODEC: an integer or RGB
/// float vector. STREAM_CODEC sends the resulting signed integer unchanged.
pub fn decode_dyed_color(value: &mc_nbt::Tag) -> Result<i32, &'static str> {
    use mc_nbt::Tag;
    match value {
        Tag::Byte(value) => Ok(i32::from(*value)),
        Tag::Short(value) => Ok(i32::from(*value)),
        Tag::Int(value) => Ok(*value),
        Tag::List(list) if list.elements.len() == 3 => {
            let mut rgb = 0;
            for value in &list.elements {
                let channel = match value {
                    Tag::Byte(value) => f32::from(*value),
                    Tag::Short(value) => f32::from(*value),
                    Tag::Int(value) => *value as f32,
                    Tag::Long(value) => *value as f32,
                    Tag::Float(value) => *value,
                    Tag::Double(value) => *value as f32,
                    _ => return Err("expected numeric RGB channel"),
                };
                // ARGB.as8BitChannel floors, and ARGB.color masks each byte.
                rgb = (rgb << 8) | ((channel * 255.0).floor() as i32 & 255);
            }
            // ExtraCodecs supplies alpha=1 to ARGB.colorFromFloat.
            Ok(rgb | !0x00ff_ffff)
        }
        _ => Err("expected integer or three RGB channels"),
    }
}

pub fn validate_max_damage(value: u32) -> Result<(), &'static str> {
    if value == 0 || value > i32::MAX as u32 {
        return Err("max_damage must be a positive signed integer");
    }
    Ok(())
}

/// Admit a compound in the native named-root-empty carrier, canonicalizing maps
/// once so transfer equality and inventory fences compare semantic NBT identity.
pub fn canonicalize_custom_data(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    encode_custom_data(&decode_custom_data(bytes)?)
}

pub fn deserialize_max_damage<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u32>, D::Error> {
    let value = <Option<u32> as serde::Deserialize>::deserialize(deserializer)?;
    if let Some(value) = value {
        validate_max_damage(value).map_err(serde::de::Error::custom)?;
    }
    Ok(value)
}

/// Normalize the native byte carrier once when a durable/definition DTO loads.
pub fn deserialize_custom_data<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<u8>>, D::Error> {
    let value = <Option<Vec<u8>> as serde::Deserialize>::deserialize(deserializer)?;
    value
        .map(|mut bytes| {
            let tag = decode_custom_data(&bytes).map_err(serde::de::Error::custom)?;
            bytes.clear();
            mc_nbt::write_named(&mut bytes, "", &tag).map_err(serde::de::Error::custom)?;
            if bytes.len() > MAX_CUSTOM_DATA_BYTES {
                return Err(serde::de::Error::custom("custom_data exceeds byte budget"));
            }
            Ok(bytes)
        })
        .transpose()
}

pub fn decode_custom_data(bytes: &[u8]) -> Result<mc_nbt::Tag, &'static str> {
    if bytes.len() > MAX_CUSTOM_DATA_BYTES {
        return Err("custom_data exceeds byte budget");
    }
    let mut remaining = bytes;
    let (name, mut tag) =
        mc_nbt::read_named(&mut remaining).map_err(|_| "custom_data must be valid compound NBT")?;
    if !name.is_empty() || !remaining.is_empty() {
        return Err("custom_data requires an empty root name and no trailing bytes");
    }
    validate_custom_depth(&tag, 1)?;
    canonicalize_custom_tag(&mut tag, 1)?;
    Ok(tag)
}

pub fn custom_data_from_tag(mut tag: mc_nbt::Tag) -> Result<Vec<u8>, &'static str> {
    if !matches!(tag, mc_nbt::Tag::Compound(_)) {
        return Err("custom_data must be a compound");
    }
    validate_custom_depth(&tag, 1)?;
    canonicalize_custom_tag(&mut tag, 1)?;
    encode_custom_data(&tag)
}

fn encode_custom_data(tag: &mc_nbt::Tag) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", tag)
        .map_err(|_| "custom_data must be valid compound NBT")?;
    if bytes.len() > MAX_CUSTOM_DATA_BYTES {
        return Err("custom_data exceeds byte budget");
    }
    Ok(bytes)
}

fn validate_custom_depth(tag: &mc_nbt::Tag, depth: usize) -> Result<(), &'static str> {
    use mc_nbt::Tag;
    match tag {
        Tag::Compound(fields) => {
            if depth > MAX_CUSTOM_DATA_DEPTH {
                return Err("custom_data exceeds depth budget");
            }
            for (_, child) in fields {
                validate_custom_depth(child, depth + 1)?;
            }
        }
        Tag::List(list) => {
            if depth > MAX_CUSTOM_DATA_DEPTH {
                return Err("custom_data exceeds depth budget");
            }
            for child in &list.elements {
                validate_custom_depth(child, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_custom_list_wrapper(tag: &mc_nbt::Tag) -> bool {
    matches!(tag, mc_nbt::Tag::Compound(fields) if fields.len() == 1 && fields[0].0.is_empty())
}

fn canonicalize_custom_tag(tag: &mut mc_nbt::Tag, depth: usize) -> Result<(), &'static str> {
    use mc_nbt::Tag;
    match tag {
        Tag::Compound(fields) => {
            if depth > MAX_CUSTOM_DATA_DEPTH {
                return Err("custom_data exceeds depth budget");
            }
            fields.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            if fields.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err("custom_data contains duplicate compound names");
            }
            for (_, child) in fields {
                canonicalize_custom_tag(child, depth + 1)?;
            }
        }
        Tag::List(list) => {
            if depth > MAX_CUSTOM_DATA_DEPTH {
                return Err("custom_data exceeds depth budget");
            }
            if list.element_type > mc_nbt::tag_type::LONG_ARRAY
                || list
                    .elements
                    .iter()
                    .any(|child| child.type_id() != list.element_type)
            {
                return Err("custom_data contains an unsupported list element type");
            }
            // 26.1.2 ListTag.addAndUnwrap / write: a singleton empty-name
            // compound is the wire wrapper for a heterogeneous list element.
            for child in &mut list.elements {
                if list.element_type == mc_nbt::tag_type::COMPOUND && is_custom_list_wrapper(child)
                {
                    let Tag::Compound(fields) = child else {
                        unreachable!()
                    };
                    *child = fields.pop().expect("one wrapper value").1;
                }
                canonicalize_custom_tag(child, depth + 1)?;
            }
            let first_type = list
                .elements
                .first()
                .map_or(mc_nbt::tag_type::END, Tag::type_id);
            list.element_type = if list
                .elements
                .iter()
                .all(|child| child.type_id() == first_type)
            {
                first_type
            } else {
                mc_nbt::tag_type::COMPOUND
            };
            if list.element_type == mc_nbt::tag_type::COMPOUND {
                for child in &mut list.elements {
                    if !matches!(child, Tag::Compound(_)) || is_custom_list_wrapper(child) {
                        let value = std::mem::replace(child, Tag::Compound(Vec::new()));
                        *child = Tag::Compound(vec![(String::new(), value)]);
                    }
                }
            }
        }
        Tag::Float(value) if value.is_nan() => *value = f32::NAN,
        Tag::Double(value) if value.is_nan() => *value = f64::NAN,
        Tag::Float(value) if *value == 0.0 => *value = 0.0,
        Tag::Double(value) if *value == 0.0 => *value = 0.0,
        _ => {}
    }
    Ok(())
}

pub fn decode_stew_effects(tag: &mc_nbt::Tag) -> Result<Vec<StewEffect>, &'static str> {
    use mc_nbt::Tag;
    let Tag::List(list) = tag else {
        return Err("stew effects must be a list");
    };
    list.elements
        .iter()
        .map(|entry| {
            let Tag::Compound(fields) = entry else {
                return Err("stew effect must be a compound");
            };
            let Some(Tag::String(name)) = fields
                .iter()
                .find(|(name, _)| name == "id")
                .map(|(_, tag)| tag)
            else {
                return Err("stew effect id is missing");
            };
            let id = Identifier::parse(name.clone()).map_err(|_| "invalid stew effect id")?;
            if crate::mob_effects_26_1_2::MobEffect::from_name(id.as_str()).is_none() {
                return Err("unknown stew effect id");
            }
            let duration = match fields
                .iter()
                .find(|(name, _)| name == "duration")
                .map(|(_, tag)| tag)
            {
                Some(Tag::Int(duration)) => *duration,
                _ => default_stew_duration(),
            };
            Ok(StewEffect { id, duration })
        })
        .collect()
}

#[must_use]
pub fn encode_stew_effects(effects: &[StewEffect]) -> mc_nbt::Tag {
    use mc_nbt::Tag;
    Tag::List(mc_nbt::ListTag {
        element_type: mc_nbt::tag_type::COMPOUND,
        elements: effects
            .iter()
            .map(|effect| {
                Tag::Compound(vec![
                    ("id".into(), Tag::String(effect.id.as_str().into())),
                    ("duration".into(), Tag::Int(effect.duration)),
                ])
            })
            .collect(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArmorTrim {
    pub material: Identifier,
    pub pattern: Identifier,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BannerPattern {
    pub pattern: Identifier,
    pub color: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FireworkExplosion {
    pub shape: i32,
    pub colors: Vec<i32>,
    pub fade_colors: Vec<i32>,
    pub has_trail: bool,
    pub has_twinkle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fireworks {
    pub flight_duration: i32,
    pub explosions: Vec<FireworkExplosion>,
}

/// Registry-neutral potion effect component, matching MobEffectInstance details.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PotionEffect {
    pub id: Identifier,
    pub amplifier: u8,
    pub duration: i32,
    pub ambient: bool,
    pub show_particles: bool,
    pub show_icon: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<Box<PotionEffect>>,
}

pub const MAX_POTION_CUSTOM_EFFECTS: usize = 256;
pub const MAX_POTION_HIDDEN_DEPTH: usize = 32;

/// Lossless native NBT for a component not fully represented by typed fields.
/// The baseline prevents later typed mutations from resurrecting stale NBT.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreservedItemComponent {
    pub name: String,
    pub value: Vec<u8>,
    pub baseline: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ExtendedItemComponents {
    pub potion: Option<Identifier>,
    pub potion_custom_color: Option<i32>,
    pub potion_custom_effects: Vec<PotionEffect>,
    pub potion_custom_name: Option<String>,
    pub trim: Option<ArmorTrim>,
    pub banner_patterns: Vec<BannerPattern>,
    pub repair_cost: i32,
    pub stored_enchantments: Vec<ItemEnchantment>,
    pub charged_projectiles: Vec<ItemStack>,
    pub fireworks: Option<Fireworks>,
    pub ominous_bottle_amplifier: Option<i32>,
    #[serde(
        deserialize_with = "deserialize_max_damage",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_damage: Option<u32>,
    #[serde(
        deserialize_with = "deserialize_custom_data",
        skip_serializing_if = "Option::is_none"
    )]
    pub custom_data: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub preserved_components: Vec<PreservedItemComponent>,
}

impl ExtendedItemComponents {
    pub fn has_potion_contents(&self) -> bool {
        self.potion.is_some()
            || self.potion_custom_color.is_some()
            || !self.potion_custom_effects.is_empty()
            || self.potion_custom_name.is_some()
    }
    pub const EMPTY: Self = Self {
        potion: None,
        potion_custom_color: None,
        potion_custom_effects: Vec::new(),
        potion_custom_name: None,
        trim: None,
        banner_patterns: Vec::new(),
        repair_cost: 0,
        stored_enchantments: Vec::new(),
        charged_projectiles: Vec::new(),
        fireworks: None,
        ominous_bottle_amplifier: None,
        max_damage: None,
        custom_data: None,
        preserved_components: Vec::new(),
    };

    #[must_use]
    pub fn empty() -> &'static Self {
        static EMPTY: ExtendedItemComponents = ExtendedItemComponents::EMPTY;
        &EMPTY
    }

    pub fn into_optional(self) -> Option<Box<Self>> {
        (self != Self::EMPTY).then(|| Box::new(self))
    }
}

#[derive(Debug, Clone, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ItemStack {
    pub count: i32,
    pub item_id: u32,
    pub damage: Option<i32>,
    pub enchantments: Vec<ItemEnchantment>,
    pub custom_name: Option<Box<str>>,
    pub item_model: Option<Arc<Identifier>>,
    pub stew_effects: Vec<StewEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Box<ExtendedItemComponents>>,
}

impl PartialEq for ItemStack {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count
            && self.item_id == other.item_id
            && self.damage == other.damage
            && self.enchantments == other.enchantments
            && self.custom_name == other.custom_name
            && self.item_model == other.item_model
            && self.stew_effects == other.stew_effects
            && {
                let left = self.extended();
                let right = other.extended();
                // Component-free stacks share the same immutable empty image.
                std::ptr::eq(left, right) || left == right
            }
    }
}

impl ItemStack {
    pub const EMPTY: ItemStack = ItemStack {
        count: 0,
        item_id: 0,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
        components: None,
    };

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count <= 0
    }

    #[must_use]
    pub fn new(item_id: u32, count: i32) -> Self {
        Self {
            count,
            item_id,
            damage: None,
            enchantments: Vec::new(),
            custom_name: None,
            item_model: None,
            stew_effects: Vec::new(),
            components: None,
        }
    }

    #[must_use]
    pub fn extended(&self) -> &ExtendedItemComponents {
        self.components
            .as_deref()
            .unwrap_or(ExtendedItemComponents::empty())
    }

    pub fn extended_mut(&mut self) -> &mut ExtendedItemComponents {
        self.components.get_or_insert_with(Default::default)
    }

    fn clear_preserved_component(&mut self, name: &str) {
        if let Some(components) = &mut self.components {
            components
                .preserved_components
                .retain(|component| component.name != name);
        }
    }

    #[must_use]
    pub fn with_damage(mut self, damage: i32) -> Self {
        self.damage = Some(damage.max(0));
        self.clear_preserved_component("minecraft:damage");
        self
    }

    #[must_use]
    pub fn with_enchantment(mut self, id: Identifier, level: i32) -> Self {
        self.enchantments.retain(|enchantment| enchantment.id != id);
        self.enchantments.push(ItemEnchantment { id, level });
        self.enchantments
            .sort_unstable_by(|left, right| left.id.cmp(&right.id));
        self.clear_preserved_component("minecraft:enchantments");
        self
    }

    #[must_use]
    pub fn with_custom_name(mut self, name: impl Into<String>) -> Self {
        self.custom_name = Some(name.into().into_boxed_str());
        self.clear_preserved_component("minecraft:custom_name");
        self
    }

    #[must_use]
    pub fn with_item_model(mut self, model: Identifier) -> Self {
        self.item_model = Some(Arc::new(model));
        self.clear_preserved_component("minecraft:item_model");
        self
    }

    pub fn with_max_damage(mut self, max_damage: u32) -> Result<Self, &'static str> {
        validate_max_damage(max_damage)?;
        self.clear_preserved_component("minecraft:max_damage");
        self.extended_mut().max_damage = Some(max_damage);
        Ok(self)
    }

    pub fn with_custom_data(mut self, custom_data: &[u8]) -> Result<Self, &'static str> {
        let custom_data = canonicalize_custom_data(custom_data)?;
        self.clear_preserved_component("minecraft:custom_data");
        self.extended_mut().custom_data = Some(custom_data);
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_nbt::Tag;

    #[test]
    fn equality_preserves_optional_empty_and_owned_component_values() {
        let plain = ItemStack::new(1, 3);
        let mut explicit_empty = plain.clone();
        explicit_empty.extended_mut();
        let mut changed = plain.clone();
        changed.extended_mut().repair_cost = 2;
        let stacks = [plain, explicit_empty, changed];
        for (i, left) in stacks.iter().enumerate() {
            for (j, right) in stacks.iter().enumerate() {
                assert_eq!(left == right, i == j || (i < 2 && j < 2));
            }
            assert_eq!(left, &left.clone());
            let mut different_count = left.clone();
            different_count.count += 1;
            assert_ne!(left, &different_count);
        }
    }

    #[test]
    fn custom_data_canonicalizes_nested_map_order_without_changing_values() {
        let first = Tag::Compound(vec![
            (
                "z".into(),
                Tag::Compound(vec![("b".into(), Tag::Int(2)), ("a".into(), Tag::Int(1))]),
            ),
            ("a".into(), Tag::ByteArray(vec![0, -1, 7])),
        ]);
        let second = Tag::Compound(vec![
            ("a".into(), Tag::ByteArray(vec![0, -1, 7])),
            (
                "z".into(),
                Tag::Compound(vec![("a".into(), Tag::Int(1)), ("b".into(), Tag::Int(2))]),
            ),
        ]);
        let canonical = custom_data_from_tag(first).unwrap();
        assert_eq!(canonical, custom_data_from_tag(second.clone()).unwrap());
        assert_eq!(decode_custom_data(&canonical).unwrap(), second);
        let changed = custom_data_from_tag(Tag::Compound(vec![(
            "a".into(),
            Tag::ByteArray(vec![0, -1, 8]),
        )]))
        .unwrap();
        assert_ne!(canonical, changed);
    }

    #[test]
    fn custom_data_rejects_malformed_duplicate_trailing_and_overbudget_compounds() {
        assert!(canonicalize_custom_data(&[3, 0, 0, 0, 1]).is_err());
        assert!(canonicalize_custom_data(&[10, 0, 0]).is_err());
        assert!(canonicalize_custom_data(&[10, 0, 0, 0, 1]).is_err());
        assert!(canonicalize_custom_data(&vec![0; MAX_CUSTOM_DATA_BYTES + 1]).is_err());
        assert!(
            custom_data_from_tag(Tag::Compound(vec![
                ("a".into(), Tag::Int(1)),
                ("a".into(), Tag::Int(2)),
            ]))
            .is_err()
        );
        assert!(
            custom_data_from_tag(Tag::Compound(vec![(
                "a".into(),
                Tag::ByteArray(vec![0; MAX_CUSTOM_DATA_BYTES])
            ),]))
            .is_err()
        );
        let mut deep = Tag::Compound(Vec::new());
        for _ in 0..MAX_CUSTOM_DATA_DEPTH {
            deep = Tag::Compound(vec![("nested".into(), deep)]);
        }
        assert!(custom_data_from_tag(deep).is_err());
        assert!(validate_max_damage(0).is_err());
        assert!(validate_max_damage(i32::MAX as u32 + 1).is_err());
        assert!(validate_max_damage(i32::MAX as u32).is_ok());
    }
}

#[cfg(test)]
mod vanilla_identity_tests {
    use super::*;
    use mc_nbt::{ListTag, Tag, tag_type};

    #[test]
    fn vanilla_list_wrappers_and_numeric_admission_have_one_canonical_identity() {
        let direct = Tag::Compound(vec![(
            "list".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT,
                elements: vec![Tag::Int(7)],
            }),
        )]);
        let wrapped = Tag::Compound(vec![(
            "list".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![(String::new(), Tag::Int(7))])],
            }),
        )]);
        assert_eq!(
            custom_data_from_tag(direct).unwrap(),
            custom_data_from_tag(wrapped).unwrap()
        );
        let mixed = Tag::Compound(vec![(
            "list".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![
                    Tag::Compound(vec![(String::new(), Tag::Int(7))]),
                    Tag::Compound(vec![("value".into(), Tag::String("text".into()))]),
                    Tag::Compound(vec![(
                        String::new(),
                        Tag::Compound(vec![(String::new(), Tag::Int(8))]),
                    )]),
                ],
            }),
        )]);
        let bytes = custom_data_from_tag(mixed).unwrap();
        assert_eq!(canonicalize_custom_data(&bytes).unwrap(), bytes);
        let numeric = |float, double| {
            Tag::Compound(vec![
                ("float".into(), Tag::Float(float)),
                ("double".into(), Tag::Double(double)),
            ])
        };
        assert_eq!(
            custom_data_from_tag(numeric(-0.0, -0.0)).unwrap(),
            custom_data_from_tag(numeric(0.0, 0.0)).unwrap()
        );
        assert_eq!(
            custom_data_from_tag(numeric(
                f32::from_bits(0x7fc0_0001),
                f64::from_bits(0x7ff8_0000_0000_0001)
            ))
            .unwrap(),
            custom_data_from_tag(numeric(f32::NAN, f64::NAN)).unwrap()
        );
        assert!(
            custom_data_from_tag(Tag::Compound(vec![(
                "list".into(),
                Tag::List(ListTag {
                    element_type: 99,
                    elements: Vec::new(),
                })
            )]))
            .is_err()
        );
    }
}
