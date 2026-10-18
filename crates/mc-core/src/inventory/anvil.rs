//! Vanilla 26.1.2 `AnvilMenu.createResultInternal`, without menu/player mutation.
//! `material_cost` is the number of right-hand items consumed (zero for rename-only).

use super::{ItemRules, enchanting::EnchantmentCatalog};
use mc_data::{ItemEnchantment, ItemStack};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnvilResult {
    pub item: ItemStack,
    pub cost: i32,
    pub material_cost: i32,
}

fn empty() -> AnvilResult {
    AnvilResult {
        item: ItemStack::EMPTY,
        cost: 0,
        material_cost: 0,
    }
}

pub fn result(
    catalog: &EnchantmentCatalog,
    rules: &ItemRules<'_>,
    left: &ItemStack,
    right: &ItemStack,
    name: Option<&str>,
    creative: bool,
) -> AnvilResult {
    if left.is_empty() || logical_item(rules, left).is_none() {
        return empty();
    }
    let mut item = left.clone();
    let mut enchantments = std::mem::take(crafting_enchantments_mut(rules, &mut item));
    let tax = i64::from(left.extended().repair_cost) + i64::from(right.extended().repair_cost);
    let mut price = 0_i32;
    let mut material_cost = 0;
    if !right.is_empty() {
        if logical_item(rules, right).is_none() {
            return empty();
        }
        let using_book = is_book(rules, right) || !right.extended().stored_enchantments.is_empty();
        if let Some(max) = max_damage(rules, left)
            && valid_repair_material(rules, left, right)
        {
            let mut damage = left.damage.unwrap_or(0).clamp(0, max);
            let mut amount = damage.min(max / 4);
            if amount <= 0 {
                return empty();
            }
            while amount > 0 && material_cost < right.count {
                damage -= amount;
                material_cost += 1;
                price = price.saturating_add(1);
                amount = damage.min(max / 4);
            }
            item.damage = Some(damage);
        } else {
            if !using_book
                && (!same_identity(rules, left, right) || max_damage(rules, left).is_none())
            {
                return empty();
            }
            if !using_book && let Some(max) = max_damage(rules, left) {
                let right_max = max_damage(rules, right).unwrap_or(0);
                let remaining = i64::from(max - left.damage.unwrap_or(0).clamp(0, max))
                    + i64::from(right_max - right.damage.unwrap_or(0).clamp(0, right_max))
                    + i64::from(max) * 12 / 100;
                let damage = (i64::from(max) - remaining).max(0) as i32;
                if damage < left.damage.unwrap_or(0) {
                    item.damage = Some(damage);
                    price = price.saturating_add(2);
                }
            }
            let mut accepted = false;
            let mut rejected = false;
            for addition in crafting_enchantments(rules, right) {
                let Some(definition) = catalog.get(&addition.id) else {
                    continue;
                };
                let current = enchantments
                    .iter()
                    .find(|e| e.id == addition.id)
                    .map_or(0, |e| e.level);
                let level = if current == addition.level {
                    current.saturating_add(1)
                } else {
                    current.max(addition.level)
                };
                let mut compatible =
                    creative || is_book(rules, left) || catalog.supports(rules, left, &addition.id);
                for existing in &enchantments {
                    if existing.id != addition.id && !catalog.compatible(&addition.id, &existing.id)
                    {
                        compatible = false;
                        price = price.saturating_add(1);
                    }
                }
                if !compatible {
                    rejected = true;
                    continue;
                }
                accepted = true;
                let level = level.min(definition.max_level);
                if let Some(existing) = enchantments.iter_mut().find(|e| e.id == addition.id) {
                    existing.level = level;
                } else if level > 0 {
                    enchantments.push(ItemEnchantment {
                        id: addition.id.clone(),
                        level,
                    });
                }
                let fee = if using_book {
                    (definition.anvil_cost / 2).max(1)
                } else {
                    definition.anvil_cost
                };
                price = price.saturating_add(fee.saturating_mul(level));
                if left.count > 1 {
                    price = 40;
                }
            }
            if rejected && !accepted {
                return empty();
            }
            material_cost = right.count;
        }
    }
    let mut naming_cost = 0;
    if let Some(name) = name {
        if name.trim().is_empty() {
            if left.custom_name.is_some() {
                item.custom_name = None;
                naming_cost = 1;
            }
        } else if left.custom_name.as_deref() != Some(name) {
            item.custom_name = Some(name.into());
            naming_cost = 1;
        }
        price = price.saturating_add(naming_cost);
    }
    if price <= 0 {
        return empty();
    }
    let only_renaming = naming_cost > 0 && naming_cost == price;
    let mut cost = (tax + i64::from(price)).clamp(0, i64::from(i32::MAX)) as i32;
    if only_renaming {
        cost = cost.min(39);
        material_cost = 0;
    }
    if cost >= 40 && !creative {
        return AnvilResult {
            item: ItemStack::EMPTY,
            cost,
            material_cost: 0,
        };
    }
    let mut repair_cost = left
        .extended()
        .repair_cost
        .max(right.extended().repair_cost);
    if !only_renaming {
        repair_cost = increased_repair_cost(repair_cost);
    }
    item.extended_mut().repair_cost = repair_cost;
    set_crafting_enchantments(rules, &mut item, enchantments);
    AnvilResult {
        item,
        cost,
        material_cost,
    }
}

pub(super) fn increased_repair_cost(cost: i32) -> i32 {
    (i64::from(cost) * 2 + 1).min(i64::from(i32::MAX)) as i32
}

pub(super) fn is_book(rules: &ItemRules<'_>, item: &ItemStack) -> bool {
    logical_item(rules, item).is_some_and(|id| id.as_str() == "minecraft:enchanted_book")
}

pub(super) fn crafting_enchantments<'a>(
    rules: &ItemRules<'_>,
    item: &'a ItemStack,
) -> &'a [ItemEnchantment] {
    if is_book(rules, item) {
        &item.extended().stored_enchantments
    } else {
        &item.enchantments
    }
}

pub(super) fn crafting_enchantments_mut<'a>(
    rules: &ItemRules<'_>,
    item: &'a mut ItemStack,
) -> &'a mut Vec<ItemEnchantment> {
    if is_book(rules, item) {
        &mut item.extended_mut().stored_enchantments
    } else {
        &mut item.enchantments
    }
}

pub(super) fn set_crafting_enchantments(
    rules: &ItemRules<'_>,
    item: &mut ItemStack,
    mut enchantments: Vec<ItemEnchantment>,
) {
    enchantments.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    if is_book(rules, item) {
        item.extended_mut().stored_enchantments = enchantments;
    } else {
        item.enchantments = enchantments;
    }
}

pub(super) fn logical_item<'a>(
    rules: &ItemRules<'a>,
    stack: &ItemStack,
) -> Option<&'a mc_data::Identifier> {
    if let Some(model) = stack.item_model.as_deref()
        && rules.facts.custom(model).is_some()
    {
        return rules
            .facts
            .custom_for_stack(stack, rules.items)
            .map(|definition| &definition.id);
    }
    rules.items.name_of(stack.item_id)
}

pub(super) fn same_identity(rules: &ItemRules<'_>, left: &ItemStack, right: &ItemStack) -> bool {
    let Some(left_id) = logical_item(rules, left) else {
        return false;
    };
    logical_item(rules, right) == Some(left_id)
}

pub(super) fn max_damage(rules: &ItemRules<'_>, stack: &ItemStack) -> Option<i32> {
    if let Some(model) = stack.item_model.as_deref()
        && rules.facts.custom(model).is_some()
    {
        let facts = rules.facts.custom_for_stack(stack, rules.items)?;
        return stack
            .extended()
            .max_damage
            .or(facts.facts.max_damage)
            .and_then(|max| i32::try_from(max).ok())
            .filter(|max| *max > 0);
    }
    mc_data::item_semantics_26_1_2::max_damage_for_stack(rules.facts, rules.items, stack).or_else(
        || {
            let id = rules.items.name_of(stack.item_id)?;
            if id.namespace() != "minecraft" {
                return None;
            }
            let max = match id.path() {
                "bow" => 384,
                "brush" => 64,
                "carrot_on_a_stick" => 25,
                "crossbow" => 465,
                "elytra" => 432,
                "fishing_rod" | "flint_and_steel" => 64,
                "mace" => 500,
                "shears" => 238,
                "shield" => 336,
                "trident" => 250,
                "turtle_helmet" => 275,
                "warped_fungus_on_a_stick" => 100,
                "wolf_armor" => 64,
                "copper_axe" | "copper_hoe" | "copper_pickaxe" | "copper_shovel"
                | "copper_sword" | "copper_spear" => 190,
                "wooden_spear" => 59,
                "stone_spear" => 131,
                "iron_spear" => 250,
                "golden_spear" => 32,
                "diamond_spear" => 1561,
                "netherite_spear" => 2031,
                "leather_boots" => 65,
                "leather_chestplate" => 80,
                "leather_helmet" => 55,
                "leather_leggings" => 75,
                "chainmail_boots" | "iron_boots" => 195,
                "chainmail_chestplate" | "iron_chestplate" => 240,
                "chainmail_helmet" | "iron_helmet" => 165,
                "chainmail_leggings" | "iron_leggings" => 225,
                "copper_boots" => 143,
                "copper_chestplate" => 176,
                "copper_helmet" => 121,
                "copper_leggings" => 165,
                "diamond_boots" => 429,
                "diamond_chestplate" => 528,
                "diamond_helmet" => 363,
                "diamond_leggings" => 495,
                "golden_boots" => 91,
                "golden_chestplate" => 112,
                "golden_helmet" => 77,
                "golden_leggings" => 105,
                "netherite_boots" => 481,
                "netherite_chestplate" => 592,
                "netherite_helmet" => 407,
                "netherite_leggings" => 555,
                _ => return None,
            };
            Some(max)
        },
    )
}

fn valid_repair_material(rules: &ItemRules<'_>, left: &ItemStack, right: &ItemStack) -> bool {
    // Exact repairable component selectors from the vanilla item component report.
    // ItemFacts does not yet expose REPAIRABLE; use the versioned vanilla selectors,
    // with loaded tag membership authoritative when that tag is present.
    let Some(item) = logical_item(rules, left) else {
        return false;
    };
    let selector = match item.as_str() {
        "minecraft:copper_axe"
        | "minecraft:copper_hoe"
        | "minecraft:copper_pickaxe"
        | "minecraft:copper_shovel"
        | "minecraft:copper_spear"
        | "minecraft:copper_sword" => "#minecraft:copper_tool_materials",
        "minecraft:diamond_axe"
        | "minecraft:diamond_hoe"
        | "minecraft:diamond_pickaxe"
        | "minecraft:diamond_shovel"
        | "minecraft:diamond_spear"
        | "minecraft:diamond_sword" => "#minecraft:diamond_tool_materials",
        "minecraft:golden_axe"
        | "minecraft:golden_hoe"
        | "minecraft:golden_pickaxe"
        | "minecraft:golden_shovel"
        | "minecraft:golden_spear"
        | "minecraft:golden_sword" => "#minecraft:gold_tool_materials",
        "minecraft:iron_axe"
        | "minecraft:iron_hoe"
        | "minecraft:iron_pickaxe"
        | "minecraft:iron_shovel"
        | "minecraft:iron_spear"
        | "minecraft:iron_sword" => "#minecraft:iron_tool_materials",
        "minecraft:netherite_axe"
        | "minecraft:netherite_hoe"
        | "minecraft:netherite_pickaxe"
        | "minecraft:netherite_shovel"
        | "minecraft:netherite_spear"
        | "minecraft:netherite_sword" => "#minecraft:netherite_tool_materials",
        "minecraft:chainmail_boots"
        | "minecraft:chainmail_chestplate"
        | "minecraft:chainmail_helmet"
        | "minecraft:chainmail_leggings" => "#minecraft:repairs_chain_armor",
        "minecraft:copper_boots"
        | "minecraft:copper_chestplate"
        | "minecraft:copper_helmet"
        | "minecraft:copper_leggings" => "#minecraft:repairs_copper_armor",
        "minecraft:diamond_boots"
        | "minecraft:diamond_chestplate"
        | "minecraft:diamond_helmet"
        | "minecraft:diamond_leggings" => "#minecraft:repairs_diamond_armor",
        "minecraft:golden_boots"
        | "minecraft:golden_chestplate"
        | "minecraft:golden_helmet"
        | "minecraft:golden_leggings" => "#minecraft:repairs_gold_armor",
        "minecraft:iron_boots"
        | "minecraft:iron_chestplate"
        | "minecraft:iron_helmet"
        | "minecraft:iron_leggings" => "#minecraft:repairs_iron_armor",
        "minecraft:leather_boots"
        | "minecraft:leather_chestplate"
        | "minecraft:leather_helmet"
        | "minecraft:leather_leggings" => "#minecraft:repairs_leather_armor",
        "minecraft:netherite_boots"
        | "minecraft:netherite_chestplate"
        | "minecraft:netherite_helmet"
        | "minecraft:netherite_leggings" => "#minecraft:repairs_netherite_armor",
        "minecraft:turtle_helmet" => "#minecraft:repairs_turtle_helmet",
        "minecraft:wolf_armor" => "#minecraft:repairs_wolf_armor",
        "minecraft:stone_axe"
        | "minecraft:stone_hoe"
        | "minecraft:stone_pickaxe"
        | "minecraft:stone_shovel"
        | "minecraft:stone_spear"
        | "minecraft:stone_sword" => "#minecraft:stone_tool_materials",
        "minecraft:shield"
        | "minecraft:wooden_axe"
        | "minecraft:wooden_hoe"
        | "minecraft:wooden_pickaxe"
        | "minecraft:wooden_shovel"
        | "minecraft:wooden_spear"
        | "minecraft:wooden_sword" => "#minecraft:wooden_tool_materials",
        "minecraft:mace" => "minecraft:breeze_rod",
        "minecraft:elytra" => "minecraft:phantom_membrane",
        _ => return false,
    };
    let Some(material) = logical_item(rules, right) else {
        return false;
    };
    if material.namespace() != "minecraft" {
        return false;
    }
    if let Some(tag) = selector.strip_prefix('#')
        && let Some(members) = rules
            .tags
            .registries
            .get("minecraft:item")
            .and_then(|tags| tags.get(tag))
    {
        return i32::try_from(right.item_id).is_ok_and(|raw| members.contains(&raw));
    }
    match selector {
        "#minecraft:copper_tool_materials" | "#minecraft:repairs_copper_armor" => {
            material.as_str() == "minecraft:copper_ingot"
        }
        "#minecraft:diamond_tool_materials" | "#minecraft:repairs_diamond_armor" => {
            material.as_str() == "minecraft:diamond"
        }
        "#minecraft:gold_tool_materials" | "#minecraft:repairs_gold_armor" => {
            material.as_str() == "minecraft:gold_ingot"
        }
        "#minecraft:iron_tool_materials"
        | "#minecraft:repairs_iron_armor"
        | "#minecraft:repairs_chain_armor" => material.as_str() == "minecraft:iron_ingot",
        "#minecraft:netherite_tool_materials" | "#minecraft:repairs_netherite_armor" => {
            material.as_str() == "minecraft:netherite_ingot"
        }
        "#minecraft:repairs_leather_armor" => material.as_str() == "minecraft:leather",
        "#minecraft:repairs_turtle_helmet" => material.as_str() == "minecraft:turtle_scute",
        "#minecraft:repairs_wolf_armor" => material.as_str() == "minecraft:armadillo_scute",
        "#minecraft:stone_tool_materials" => matches!(
            material.as_str(),
            "minecraft:blackstone" | "minecraft:cobbled_deepslate" | "minecraft:cobblestone"
        ),
        "#minecraft:wooden_tool_materials" => matches!(
            material.as_str(),
            "minecraft:acacia_planks"
                | "minecraft:bamboo_planks"
                | "minecraft:birch_planks"
                | "minecraft:cherry_planks"
                | "minecraft:crimson_planks"
                | "minecraft:dark_oak_planks"
                | "minecraft:jungle_planks"
                | "minecraft:mangrove_planks"
                | "minecraft:oak_planks"
                | "minecraft:pale_oak_planks"
                | "minecraft:spruce_planks"
                | "minecraft:warped_planks"
        ),
        direct => material.as_str() == direct,
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use mc_data::Identifier;
    use mc_data::item_components::ItemFactsTable;
    use mc_data::items::{ItemRegistry, ItemReport};
    use mc_data::tags::TagsData;

    pub(in crate::inventory) struct Data {
        pub(in crate::inventory) items: ItemRegistry,
        facts: ItemFactsTable,
        pub(in crate::inventory) tags: TagsData,
    }

    pub(in crate::inventory) fn catalog() -> EnchantmentCatalog {
        EnchantmentCatalog::runtime()
    }

    pub(in crate::inventory) fn id(name: &str) -> Identifier {
        Identifier::parse(name).unwrap()
    }

    impl Data {
        pub(in crate::inventory) fn new() -> Self {
            let names = [
                "diamond_sword",
                "diamond",
                "enchanted_book",
                "book",
                "stone",
                "oak_planks",
                "wooden_sword",
                "iron_ingot",
                "bow",
                "diamond_pickaxe",
                "diamond_boots",
            ];
            Self {
                items: ItemRegistry::from_report(
                    &names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| ItemReport {
                            id: id(name),
                            protocol_id: i as u32 + 1,
                        })
                        .collect::<Vec<_>>(),
                ),
                facts: ItemFactsTable::default(),
                tags: TagsData::default(),
            }
        }
        pub(in crate::inventory) fn rules(&self) -> ItemRules<'_> {
            ItemRules {
                items: &self.items,
                facts: &self.facts,
                tags: &self.tags,
                recipes: &[],
            }
        }
        pub(in crate::inventory) fn stack(&self, name: &str) -> ItemStack {
            ItemStack::new(self.items.id_of(&id(name)).unwrap(), 1)
        }
        pub(in crate::inventory) fn book(&self, name: &str, level: i32) -> ItemStack {
            let mut book = self.stack("enchanted_book");
            book.extended_mut()
                .stored_enchantments
                .push(ItemEnchantment {
                    id: id(name),
                    level,
                });
            book
        }
    }

    #[test]
    fn material_repairs_quarters_and_consumes_only_needed_items() {
        let data = Data::new();
        let catalog = catalog();
        let left = data
            .stack("diamond_sword")
            .with_damage(800)
            .with_custom_name("Sword");
        let mut right = data.stack("diamond");
        right.count = 9;
        let output = result(&catalog, &data.rules(), &left, &right, None, false);
        assert_eq!(output.item.damage, Some(0));
        assert_eq!(output.cost, 3);
        assert_eq!(output.material_cost, 3);
        assert_eq!(output.item.extended().repair_cost, 1);
        assert_eq!(output.item.custom_name.as_deref(), Some("Sword"));
        let partial = result(
            &catalog,
            &data.rules(),
            &left,
            &data.stack("diamond"),
            None,
            false,
        );
        assert_eq!(partial.item.damage, Some(410));
        assert_eq!(partial.cost, 1);
        assert_eq!(partial.material_cost, 1);
        assert!(
            result(
                &catalog,
                &data.rules(),
                &data.stack("diamond_sword"),
                &right,
                Some("New"),
                false
            )
            .item
            .is_empty()
        );
    }

    #[test]
    fn loaded_repair_component_tags_are_authoritative() {
        use std::collections::BTreeMap;
        let mut data = Data::new();
        let raw = data.stack("stone").item_id as i32;
        data.tags = TagsData::from_registries(BTreeMap::from([(
            id("minecraft:item"),
            BTreeMap::from([(id("minecraft:diamond_tool_materials"), vec![raw])]),
        )]));
        let catalog = catalog();
        let left = data.stack("diamond_sword").with_damage(100);
        assert_eq!(
            result(
                &catalog,
                &data.rules(),
                &left,
                &data.stack("stone"),
                None,
                false
            )
            .material_cost,
            1
        );
        assert!(
            result(
                &catalog,
                &data.rules(),
                &left,
                &data.stack("diamond"),
                None,
                false
            )
            .item
            .is_empty()
        );
    }

    #[test]
    fn sacrifice_repairs_twelve_percent_and_adds_two_levels_plus_prior_work() {
        let data = Data::new();
        let catalog = catalog();
        let mut left = data.stack("diamond_sword").with_damage(1400);
        let mut right = data.stack("diamond_sword").with_damage(1300);
        left.extended_mut().repair_cost = 3;
        right.extended_mut().repair_cost = 7;
        let output = result(&catalog, &data.rules(), &left, &right, None, false);
        assert_eq!(output.item.damage, Some(952));
        assert_eq!(output.cost, 12);
        assert_eq!(output.material_cost, 1);
        assert_eq!(output.item.extended().repair_cost, 15);
    }

    #[test]
    fn books_discount_enchants_upgrade_equal_levels_and_clamp_maximum() {
        let data = Data::new();
        let catalog = catalog();
        let left = data
            .stack("diamond_sword")
            .with_enchantment(id("sharpness"), 2);
        let output = result(
            &catalog,
            &data.rules(),
            &left,
            &data.book("sharpness", 2),
            None,
            false,
        );
        assert_eq!(output.item.enchantments[0].level, 3);
        assert_eq!(output.cost, 3);
        let maximum = data
            .stack("diamond_sword")
            .with_enchantment(id("sharpness"), 5);
        let output = result(
            &catalog,
            &data.rules(),
            &maximum,
            &data.book("sharpness", 5),
            None,
            false,
        );
        assert_eq!(output.item.enchantments[0].level, 5);
        assert_eq!(output.cost, 5);
        let book = data.book("unbreaking", 2);
        let book_output = result(
            &catalog,
            &data.rules(),
            &book,
            &data.book("unbreaking", 2),
            None,
            false,
        );
        assert!(book_output.item.enchantments.is_empty());
        assert_eq!(book_output.item.extended().stored_enchantments[0].level, 3);
        let tool_output = result(
            &catalog,
            &data.rules(),
            &data.stack("diamond_sword"),
            &data
                .stack("diamond_sword")
                .with_enchantment(id("unbreaking"), 2),
            None,
            false,
        );
        let book_output = result(
            &catalog,
            &data.rules(),
            &data.stack("diamond_sword"),
            &book,
            None,
            false,
        );
        assert_eq!(tool_output.cost, 4);
        assert_eq!(book_output.cost, 2);
    }

    #[test]
    fn incompatibilities_charge_each_conflict_but_all_rejected_is_empty() {
        let data = Data::new();
        let catalog = catalog();
        let left = data
            .stack("diamond_sword")
            .with_enchantment(id("sharpness"), 2);
        assert!(
            result(
                &catalog,
                &data.rules(),
                &left,
                &data.book("smite", 2),
                None,
                true
            )
            .item
            .is_empty()
        );
        let mut mixed = data.book("smite", 2);
        mixed
            .extended_mut()
            .stored_enchantments
            .push(ItemEnchantment {
                id: id("unbreaking"),
                level: 1,
            });
        let output = result(&catalog, &data.rules(), &left, &mixed, None, false);
        assert_eq!(output.cost, 2);
        assert_eq!(output.item.enchantments.len(), 2);
        assert!(output.item.enchantments.iter().all(|e| e.id != id("smite")));
        assert!(
            result(
                &catalog,
                &data.rules(),
                &data.stack("stone"),
                &data.book("sharpness", 1),
                None,
                false
            )
            .item
            .is_empty()
        );
        assert!(
            !result(
                &catalog,
                &data.rules(),
                &data.stack("stone"),
                &data.book("sharpness", 1),
                None,
                true
            )
            .item
            .is_empty()
        );
    }

    #[test]
    fn renaming_and_clearing_preserve_penalty_and_cap_cost_at_thirty_nine() {
        let data = Data::new();
        let catalog = catalog();
        let mut left = data.stack("diamond_sword").with_custom_name("Original");
        left.extended_mut().repair_cost = 63;
        assert!(
            result(
                &catalog,
                &data.rules(),
                &left,
                &ItemStack::EMPTY,
                None,
                false
            )
            .item
            .is_empty()
        );
        assert!(
            result(
                &catalog,
                &data.rules(),
                &left,
                &ItemStack::EMPTY,
                Some("Original"),
                false
            )
            .item
            .is_empty()
        );
        let output = result(
            &catalog,
            &data.rules(),
            &left,
            &ItemStack::EMPTY,
            Some("New"),
            false,
        );
        assert_eq!(output.cost, 39);
        assert_eq!(output.item.custom_name.as_deref(), Some("New"));
        assert_eq!(output.item.extended().repair_cost, 63);
        assert_eq!(output.material_cost, 0);
        let cleared = result(
            &catalog,
            &data.rules(),
            &left,
            &ItemStack::EMPTY,
            Some(""),
            false,
        );
        assert_eq!(cleared.item.custom_name, None);
        assert_eq!(cleared.cost, 39);
        let mut right = data.book("unbreaking", 1);
        right.extended_mut().repair_cost = 127;
        let named = result(
            &catalog,
            &data.rules(),
            &data
                .stack("diamond_sword")
                .with_enchantment(id("sharpness"), 1),
            &right,
            Some("New"),
            true,
        );
        assert!(named.cost > 39);
    }

    #[test]
    fn survival_cost_limit_and_overflow_saturate_without_losing_display_price() {
        let data = Data::new();
        let catalog = catalog();
        let mut left = data.stack("diamond_sword");
        left.extended_mut().repair_cost = 39;
        let right = data.book("unbreaking", 1);
        let survival = result(&catalog, &data.rules(), &left, &right, None, false);
        assert_eq!(survival.cost, 40);
        assert!(survival.item.is_empty());
        let creative = result(&catalog, &data.rules(), &left, &right, None, true);
        assert_eq!(creative.item.extended().repair_cost, 79);
        left.extended_mut().repair_cost = i32::MAX;
        let saturated = result(&catalog, &data.rules(), &left, &right, None, true);
        assert_eq!(saturated.cost, i32::MAX);
        assert_eq!(saturated.item.extended().repair_cost, i32::MAX);
        let mut stacked = data.stack("stone");
        stacked.count = 2;
        assert!(
            result(&catalog, &data.rules(), &stacked, &right, None, false)
                .item
                .is_empty()
        );
        assert_eq!(
            result(&catalog, &data.rules(), &stacked, &right, None, true).cost,
            40
        );
    }
}

#[cfg(test)]
mod additional_tests {
    use super::tests::{Data, catalog, id};
    use super::*;

    #[test]
    fn rename_only_preserves_unused_sacrifice_and_uses_highest_penalty() {
        let data = Data::new();
        let catalog = catalog();
        let left = data.stack("diamond_sword");
        let mut right = data.stack("diamond_sword");
        right.extended_mut().repair_cost = 127;
        let output = result(&catalog, &data.rules(), &left, &right, Some("Name"), false);
        assert_eq!(output.cost, 39);
        assert_eq!(output.material_cost, 0);
        assert_eq!(output.item.extended().repair_cost, 127);
        assert!(
            result(&catalog, &data.rules(), &left, &right, None, false)
                .item
                .is_empty()
        );
    }

    #[test]
    fn material_repair_ignores_sacrifice_enchants_and_uses_stack_durability_override() {
        let data = Data::new();
        let catalog = catalog();
        let left = data
            .stack("wooden_sword")
            .with_max_damage(100)
            .unwrap()
            .with_damage(60);
        let mut right = data
            .stack("oak_planks")
            .with_enchantment(id("sharpness"), 5);
        right.count = 2;
        let output = result(&catalog, &data.rules(), &left, &right, None, false);
        assert_eq!(output.item.damage, Some(10));
        assert!(output.item.enchantments.is_empty());
        assert_eq!(output.cost, 2);
        assert_eq!(output.material_cost, 2);
        let tiny = left.with_max_damage(3).unwrap().with_damage(2);
        assert!(
            result(&catalog, &data.rules(), &tiny, &right, None, false)
                .item
                .is_empty()
        );
        assert!(
            result(
                &catalog,
                &data.rules(),
                &data.stack("diamond_sword"),
                &data.stack("stone"),
                Some("Name"),
                true
            )
            .item
            .is_empty()
        );
        assert_eq!(
            result(
                &catalog,
                &data.rules(),
                &ItemStack::EMPTY,
                &right,
                Some("Name"),
                true
            ),
            empty()
        );
    }
}

#[cfg(test)]
mod custom_identity_tests {
    use super::tests::{Data, catalog, id};
    use super::*;
    use mc_data::item_components::{CustomItemDefinition, ItemFacts, ItemFactsTable};

    #[test]
    fn custom_items_never_combine_or_repair_as_their_carrier() {
        let data = Data::new();
        let facts = ItemFactsTable::default()
            .with_custom_items(
                [
                    CustomItemDefinition {
                        id: id("example:sword"),
                        carrier: id("diamond_sword"),
                        name: "Custom sword".into(),
                        crafting_ingredient: None,
                        facts: ItemFacts {
                            max_stack_size: Some(1),
                            max_damage: Some(100),
                            ..Default::default()
                        },
                    },
                    CustomItemDefinition {
                        id: id("example:token"),
                        carrier: id("diamond_sword"),
                        name: "Token".into(),
                        crafting_ingredient: None,
                        facts: ItemFacts {
                            max_stack_size: Some(64),
                            ..Default::default()
                        },
                    },
                    CustomItemDefinition {
                        id: id("example:gem"),
                        carrier: id("diamond"),
                        name: "Gem".into(),
                        crafting_ingredient: None,
                        facts: ItemFacts {
                            max_stack_size: Some(64),
                            ..Default::default()
                        },
                    },
                ],
                &data.items,
            )
            .unwrap();
        let rules = ItemRules {
            facts: &facts,
            ..data.rules()
        };
        let catalog = catalog();
        let vanilla = data.stack("diamond_sword").with_damage(100);
        let custom = data
            .stack("diamond_sword")
            .with_item_model(id("example:sword"))
            .with_damage(80);
        let token = data
            .stack("diamond_sword")
            .with_item_model(id("example:token"));
        assert_eq!(max_damage(&rules, &token), None);
        assert_eq!(max_damage(&rules, &custom), Some(100));
        assert!(!same_identity(&rules, &vanilla, &custom));
        assert!(!same_identity(&rules, &token, &custom));
        assert!(
            result(
                &catalog,
                &rules,
                &custom,
                &data.stack("diamond"),
                None,
                false
            )
            .item
            .is_empty()
        );
        let gem = data.stack("diamond").with_item_model(id("example:gem"));
        assert!(
            result(&catalog, &rules, &vanilla, &gem, None, false)
                .item
                .is_empty()
        );
        assert!(
            result(&catalog, &rules, &vanilla, &custom, None, false)
                .item
                .is_empty()
        );
        assert!(
            super::super::grindstone::result(&catalog, &rules, &vanilla, &custom, 0)
                .item
                .is_empty()
        );
        assert!(
            super::super::grindstone::result(&catalog, &rules, &token, &vanilla, 0)
                .item
                .is_empty()
        );
        let repaired = super::super::grindstone::result(&catalog, &rules, &custom, &custom, 0);
        assert_eq!(repaired.item.damage, Some(55));
        assert_eq!(repaired.item.extended().max_damage, Some(100));
        let tokens = super::super::grindstone::result(&catalog, &rules, &token, &token, 0);
        assert_eq!(tokens.item.count, 2);
        assert_eq!(tokens.item.damage, None);
        let malformed = data.stack("stone").with_item_model(id("example:sword"));
        assert_eq!(logical_item(&rules, &malformed), None);
        assert!(
            result(
                &catalog,
                &rules,
                &malformed,
                &ItemStack::EMPTY,
                Some("Name"),
                true
            )
            .item
            .is_empty()
        );
    }
}
