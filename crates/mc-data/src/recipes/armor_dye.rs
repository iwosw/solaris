use crate::{ItemStack, item_stack::PreservedItemComponent, items::ItemRegistry};
use mc_nbt::Tag;

const DYED_COLOR: &str = "minecraft:dyed_color";

/// The six 26.1.2 crafting_dye targets (the former dyeable tag). Like the
/// repair special recipe, this works without requiring an external sidecar.
#[must_use]
pub fn armor_dye_crafting_result(
    items: &ItemRegistry,
    input: &[ItemStack; 9],
) -> Option<ItemStack> {
    let mut target = None;
    let mut dyes = 0;
    let mut channels = [0; 3];
    let mut brightness = 0;
    for stack in input.iter().filter(|stack| !stack.is_empty()) {
        let name = items.name_of(stack.item_id)?.as_str();
        if matches!(
            name,
            "minecraft:leather_boots"
                | "minecraft:leather_leggings"
                | "minecraft:leather_chestplate"
                | "minecraft:leather_helmet"
                | "minecraft:leather_horse_armor"
                | "minecraft:wolf_armor"
        ) {
            if target.replace(stack).is_some() {
                return None;
            }
        } else {
            let color = dye_color(name)?;
            add_color(color, &mut channels, &mut brightness);
            dyes += 1;
        }
    }
    let target = target?;
    if dyes == 0 {
        return None;
    }
    let mut samples = dyes;
    if let Some(component) = target
        .extended()
        .preserved_components
        .iter()
        .find(|component| component.name == DYED_COLOR)
    {
        let mut bytes = component.value.as_slice();
        let value = mc_nbt::read_network_value(&mut bytes).ok()?;
        if !bytes.is_empty() {
            return None;
        }
        let color = crate::item_stack::decode_dyed_color(&value).ok()?;
        add_color(color, &mut channels, &mut brightness);
        samples += 1;
    }
    // DyedItemColor.applyDyes: integer channel means, then f32 brightness
    // normalization, truncating each scaled channel only at the end.
    let average = channels.map(|channel| channel / samples);
    let mean_brightness = brightness as f32 / samples as f32;
    let maximum = *average.iter().max()? as f32;
    let [red, green, blue] =
        average.map(|channel| (channel as f32 * mean_brightness / maximum) as i32);
    let color = ((red & 255) << 16) | ((green & 255) << 8) | (blue & 255);
    let mut output = target.clone();
    output.count = 1;
    let preserved = &mut output.extended_mut().preserved_components;
    preserved.retain(|component| component.name.trim_start_matches('!') != DYED_COLOR);
    let mut value = Vec::with_capacity(5);
    mc_nbt::write_network_value(&mut value, &Tag::Int(color)).ok()?;
    preserved.push(PreservedItemComponent {
        name: DYED_COLOR.into(),
        value,
        baseline: None,
    });
    Some(output)
}

fn add_color(color: i32, channels: &mut [i32; 3], brightness: &mut i32) {
    let rgb = [(color >> 16) & 255, (color >> 8) & 255, color & 255];
    for (sum, channel) in channels.iter_mut().zip(rgb) {
        *sum += channel;
    }
    *brightness += rgb[0].max(rgb[1]).max(rgb[2]);
}

fn dye_color(item: &str) -> Option<i32> {
    // DyeColor.getTextureDiffuseColor, not the distinct firework colors.
    Some(match item {
        "minecraft:white_dye" => 16_383_998,
        "minecraft:orange_dye" => 16_351_261,
        "minecraft:magenta_dye" => 13_061_821,
        "minecraft:light_blue_dye" => 3_847_130,
        "minecraft:yellow_dye" => 16_701_501,
        "minecraft:lime_dye" => 8_439_583,
        "minecraft:pink_dye" => 15_961_002,
        "minecraft:gray_dye" => 4_673_362,
        "minecraft:light_gray_dye" => 10_329_495,
        "minecraft:cyan_dye" => 1_481_884,
        "minecraft:purple_dye" => 8_991_416,
        "minecraft:blue_dye" => 3_949_738,
        "minecraft:brown_dye" => 8_606_770,
        "minecraft:green_dye" => 6_192_150,
        "minecraft:red_dye" => 11_546_150,
        "minecraft:black_dye" => 1_908_001,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Identifier;

    fn stack(items: &ItemRegistry, name: &str) -> ItemStack {
        ItemStack::new(
            items
                .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
                .unwrap(),
            1,
        )
    }

    fn color(stack: &ItemStack) -> i32 {
        let value = &stack
            .extended()
            .preserved_components
            .iter()
            .find(|component| component.name == DYED_COLOR)
            .unwrap()
            .value;
        let value = mc_nbt::read_network_value(&mut value.as_slice()).unwrap();
        crate::item_stack::decode_dyed_color(&value).unwrap()
    }

    #[test]
    fn armor_dye_blends_and_redyes_without_losing_other_components() {
        let items = crate::items::solaris_required_items();
        let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
        input[4] = stack(&items, "leather_boots")
            .with_damage(7)
            .with_custom_name("Dyed boots")
            .with_enchantment(Identifier::parse("minecraft:unbreaking").unwrap(), 2);
        input[0] = stack(&items, "red_dye");
        input[8] = stack(&items, "blue_dye");
        let before = input.clone();
        let output = crate::recipes::crafting_result_from_input(
            &items,
            &crate::item_components::ItemFactsTable::default(),
            &crate::tags::TagsData::default(),
            &[],
            &input,
        );
        assert_eq!(color(&output), 0xad5398);
        assert_eq!(output.count, 1);
        assert_eq!(output.damage, Some(7));
        assert_eq!(output.custom_name, input[4].custom_name);
        assert_eq!(output.enchantments, input[4].enchantments);
        assert_eq!(input, before);

        let bytes = serde_json::to_vec(&output).unwrap();
        input = std::array::from_fn(|_| ItemStack::EMPTY);
        input[0] = serde_json::from_slice(&bytes).unwrap();
        input[1] = stack(&items, "white_dye");
        let output = armor_dye_crafting_result(&items, &input).unwrap();
        assert_eq!(color(&output), 0xd6abcd);
        assert_eq!(output.extended().preserved_components.len(), 1);
        let fields = crate::item_nbt::stack_to_fields(&items, &output).unwrap();
        let restored = crate::item_nbt::stack_from_fields(&items, &fields)
            .unwrap()
            .unwrap();
        assert_eq!(color(&restored), 0xd6abcd);
    }

    #[test]
    fn armor_dye_accepts_all_six_targets_and_one_to_eight_dye_slots() {
        let items = crate::items::solaris_required_items();
        for target in [
            "leather_boots",
            "leather_leggings",
            "leather_chestplate",
            "leather_helmet",
            "leather_horse_armor",
            "wolf_armor",
        ] {
            for count in 1..=8 {
                let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
                input[0] = stack(&items, target);
                input[0].count = 2;
                for dye in &mut input[1..=count] {
                    *dye = stack(&items, "red_dye");
                    dye.count = 64; // One sample per occupied slot, not stack count.
                }
                let output = armor_dye_crafting_result(&items, &input).unwrap();
                assert_eq!(color(&output), 0xb02e26, "{target} {count}");
                assert_eq!(output.count, 1);
            }
        }
    }

    #[test]
    fn armor_dye_rejects_missing_duplicate_and_non_dye_ingredients() {
        let items = crate::items::solaris_required_items();
        for ingredients in [
            vec![],
            vec!["leather_boots"],
            vec!["red_dye"],
            vec!["leather_boots", "leather_helmet", "red_dye"],
            vec!["iron_boots", "red_dye"],
            vec!["leather_boots", "red_dye", "stone"],
        ] {
            let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
            for (slot, name) in ingredients.iter().enumerate() {
                input[slot] = stack(&items, name);
            }
            assert!(
                armor_dye_crafting_result(&items, &input).is_none(),
                "{ingredients:?}"
            );
        }
    }

    #[test]
    fn armor_dye_replaces_a_removed_color_patch_instead_of_duplicating_it() {
        let items = crate::items::solaris_required_items();
        let target = crate::item_nbt::stack_from_component_patch(
            &items,
            &Identifier::parse("minecraft:leather_boots").unwrap(),
            &[("!minecraft:dyed_color".into(), Tag::Compound(Vec::new()))],
        )
        .unwrap();
        let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
        input[0] = target;
        input[1] = stack(&items, "black_dye");
        let output = armor_dye_crafting_result(&items, &input).unwrap();
        assert_eq!(color(&output), 0x1d1d21);
        assert_eq!(output.extended().preserved_components.len(), 1);
    }
}
