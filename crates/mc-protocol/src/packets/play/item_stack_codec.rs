//! Ordinary inventory stacks and length-delimited untrusted creative stacks.
//! Only the outer creative patch is delimited; nested templates keep their codec.
use super::*;
use mc_data::item_stack::ExtendedItemComponents;

pub(super) fn decode_stack<B: Buf>(
    buf: &mut B,
    depth: usize,
    delimited: bool,
) -> Result<ItemStack, CodecError> {
    if depth > copied_components::MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    let count = buf.read_varint()?;
    if count < 0 {
        return Err(CodecError::NegativeLength(count));
    }
    if count == 0 {
        return Ok(ItemStack::EMPTY);
    }
    let item_id = u32::try_from(buf.read_varint()?)
        .map_err(|_| CodecError::NotSupported("negative item id"))?;
    if copied_components::items().name_of(item_id).is_none() {
        return Err(CodecError::NotSupported("unknown item id"));
    }
    decode_components(buf, item_id, count, depth, delimited)
}

pub(super) fn decode_components<B: Buf>(
    buf: &mut B,
    item_id: u32,
    count: i32,
    depth: usize,
    delimited: bool,
) -> Result<ItemStack, CodecError> {
    if depth > copied_components::MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    let n_add = buf.read_varint()?;
    let n_remove = buf.read_varint()?;
    if n_add < 0 || n_remove < 0 {
        return Err(CodecError::NegativeLength(n_add.min(n_remove)));
    }
    const MAX_COMPONENTS: usize = mc_data::data_component_types_26_1_2::COUNT;
    if n_add > MAX_COMPONENTS as i32
        || n_remove > MAX_COMPONENTS as i32
        || n_add + n_remove > MAX_COMPONENTS as i32
    {
        return Err(CodecError::NotSupported(
            "ItemStack with unsupported DataComponentPatch shape",
        ));
    }
    let mut stack = ItemStack {
        count,
        item_id,
        ..ItemStack::EMPTY
    };
    let mut components = ExtendedItemComponents::EMPTY;
    let mut seen = [0; MAX_COMPONENTS];
    let mut copied = Vec::new();
    for index in 0..n_add as usize {
        let id = buf.read_varint()?;
        if seen[..index].contains(&id) {
            return Err(CodecError::NotSupported("duplicate item component"));
        }
        seen[index] = id;
        if delimited {
            let length = buf.read_varint()?;
            let length = usize::try_from(length).map_err(|_| CodecError::NegativeLength(length))?;
            if length > buf.remaining() {
                return Err(CodecError::Underflow {
                    needed: length,
                    available: buf.remaining(),
                });
            }
            let mut value = (&mut *buf).take(length);
            // Erase only the bounded creative cursor type: recursive templates
            // must not instantiate Take<Take<...>> indefinitely. No byte copy.
            let mut bounded: &mut dyn Buf = &mut value;
            decode_component(
                &mut bounded,
                id,
                depth,
                &mut stack,
                &mut components,
                &mut copied,
            )?;
            if value.has_remaining() {
                return Err(CodecError::NotSupported(
                    "trailing creative component bytes",
                ));
            }
        } else {
            decode_component(buf, id, depth, &mut stack, &mut components, &mut copied)?;
        }
    }
    for index in n_add as usize..(n_add + n_remove) as usize {
        let id = buf.read_varint()?;
        if seen[..index].contains(&id) {
            return Err(CodecError::NotSupported("duplicate item component"));
        }
        let name = u32::try_from(id)
            .ok()
            .and_then(mc_data::data_component_types_26_1_2::name)
            .ok_or(CodecError::NotSupported("unknown removed item component"))?;
        seen[index] = id;
        copied.push((format!("!{name}"), Tag::Compound(Vec::new())));
    }
    stack.components = components.into_optional();
    copied_components::finish(stack, copied)
}

fn decode_component<B: Buf>(
    buf: &mut B,
    id: i32,
    depth: usize,
    stack: &mut ItemStack,
    components: &mut ExtendedItemComponents,
    copied: &mut Vec<(String, Tag)>,
) -> Result<(), CodecError> {
    if let Some(component) = copied_components::decode(buf, id, depth)? {
        copied.push(component);
        return Ok(());
    }
    match id {
        DATA_COMPONENT_CUSTOM_DATA_ID => {
            let mut bounded = (&mut *buf).take(mc_data::item_stack::MAX_CUSTOM_DATA_BYTES - 2);
            let tag = mc_nbt::read_network(&mut bounded)?;
            components.custom_data = Some(
                mc_data::item_stack::custom_data_from_tag(tag).map_err(CodecError::NotSupported)?,
            );
        }
        DATA_COMPONENT_MAX_DAMAGE_ID => {
            let value = u32::try_from(buf.read_varint()?)
                .map_err(|_| CodecError::NotSupported("invalid max_damage"))?;
            mc_data::item_stack::validate_max_damage(value).map_err(CodecError::NotSupported)?;
            components.max_damage = Some(value);
        }
        DATA_COMPONENT_DAMAGE_ID => stack.damage = Some(buf.read_varint()?.max(0)),
        DATA_COMPONENT_ITEM_MODEL_ID => {
            stack.item_model = Some(std::sync::Arc::new(buf.read_identifier()?))
        }
        DATA_COMPONENT_ENCHANTMENTS_ID => {
            stack.enchantments = read_bounded_vec(buf, 256, 2, |buf| {
                let protocol_id = u32::try_from(buf.read_varint()?)
                    .map_err(|_| CodecError::NotSupported("negative enchantment registry id"))?;
                let id = mc_data::required_registry_entry("enchantment", protocol_id).ok_or(
                    CodecError::NotSupported("unknown enchantment registry entry"),
                )?;
                let level = buf.read_varint()?;
                if !(1..=255).contains(&level) {
                    return Err(CodecError::NotSupported(
                        "invalid ItemStack enchantments component",
                    ));
                }
                Ok(mc_data::ItemEnchantment { id, level })
            })?;
            stack
                .enchantments
                .sort_unstable_by(|left, right| left.id.cmp(&right.id));
            if stack
                .enchantments
                .windows(2)
                .any(|entries| entries[0].id == entries[1].id)
            {
                return Err(CodecError::NotSupported(
                    "invalid ItemStack enchantments component",
                ));
            }
        }
        DATA_COMPONENT_SUSPICIOUS_STEW_EFFECTS_ID => {
            stack.stew_effects = read_bounded_vec(buf, 256, 2, |buf| {
                let raw = buf.read_varint()?;
                let name = u32::try_from(raw)
                    .ok()
                    .and_then(mc_data::mob_effects_26_1_2::name_for_id)
                    .ok_or(CodecError::NotSupported("unknown stew effect"))?;
                Ok(mc_data::item_stack::StewEffect {
                    id: Identifier::parse(name).expect("canonical effect identifier"),
                    duration: buf.read_varint()?,
                })
            })?;
        }
        id if station_components::decode(buf, id, components, depth)? => {}
        _ => {
            return Err(CodecError::NotSupported(
                "ItemStack with unsupported DataComponentPatch component",
            ));
        }
    }
    Ok(())
}

pub(super) fn encode_untrusted<B: BufMut>(
    stack: &ItemStack,
    buf: &mut B,
) -> Result<(), CodecError> {
    if stack.is_empty() {
        buf.write_varint(0);
        return Ok(());
    }
    // Creative encoding is not the inventory publication hotpath. Project via
    // the durable NBT owner so typed edits obey the same baseline/removal rules;
    // ordinary stack encoding still has its allocation-free plain-stack path.
    let fields = mc_data::item_nbt::stack_to_fields(copied_components::items(), stack)
        .map_err(|_| CodecError::NotSupported("invalid creative item stack"))?;
    let mut components = fields
        .into_iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("components", Tag::Compound(values)) => Some(values),
            _ => None,
        })
        .unwrap_or_default();
    let additions = components
        .iter()
        .filter(|(name, _)| !name.starts_with('!'))
        .count();
    buf.write_varint(stack.count);
    buf.write_varint(stack.item_id as i32);
    write_count(buf, additions)?;
    write_count(buf, components.len() - additions)?;
    let mut encoded = Vec::new();
    for (name, value) in components.extract_if(.., |(name, _)| !name.starts_with('!')) {
        let id = component_id(&name)?;
        encoded.clear();
        copied_components::encode_one(&mut encoded, id, value, 0)?;
        let mut payload = encoded.as_slice();
        let encoded_id = payload.read_varint()?;
        debug_assert_eq!(encoded_id, id);
        buf.write_varint(id);
        write_count(buf, payload.len())?;
        buf.put_slice(payload);
    }
    for (name, _) in components {
        buf.write_varint(component_id(&name[1..])?);
    }
    Ok(())
}

fn component_id(name: &str) -> Result<i32, CodecError> {
    Identifier::parse(name)
        .ok()
        .as_ref()
        .and_then(mc_data::data_component_types_26_1_2::id)
        .map(|id| id as i32)
        .ok_or(CodecError::NotSupported("unknown creative item component"))
}
