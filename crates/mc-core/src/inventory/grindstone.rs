//! Vanilla 26.1.2 `GrindstoneMenu.computeResult` and extraction XP.
//! The caller consumes both input slots only when the nonempty result is taken.

use super::anvil::{
    crafting_enchantments, crafting_enchantments_mut, increased_repair_cost, is_book, logical_item,
    max_damage, same_identity,
};
use super::{
    ItemRules,
    enchanting::{EnchantmentCatalog, LegacyRandom},
};
use mc_data::{Identifier, ItemStack};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrindstoneResult {
    pub item: ItemStack,
    pub experience: i32,
}

fn empty() -> GrindstoneResult {
    GrindstoneResult {
        item: ItemStack::EMPTY,
        experience: 0,
    }
}

pub fn result(
    catalog: &EnchantmentCatalog,
    rules: &ItemRules<'_>,
    left: &ItemStack,
    right: &ItemStack,
    seed: u64,
) -> GrindstoneResult {
    if (left.is_empty() && right.is_empty()) || left.count > 1 || right.count > 1 {
        return empty();
    }
    if (!left.is_empty() && logical_item(rules, left).is_none())
        || (!right.is_empty() && logical_item(rules, right).is_none())
    {
        return empty();
    }
    let mut item;
    if left.is_empty() || right.is_empty() {
        let input = if left.is_empty() { right } else { left };
        if input.enchantments.is_empty() && input.extended().stored_enchantments.is_empty() {
            return empty();
        }
        item = input.clone();
    } else {
        if !same_identity(rules, left, right) {
            return empty();
        }
        let left_max = max_damage(rules, left);
        let right_max = max_damage(rules, right);
        item = left.clone();
        if let Some(left_max) = left_max {
            let right_max = right_max.unwrap_or(0);
            let max = left_max.max(right_max);
            let remaining = i64::from(left_max - left.damage.unwrap_or(0).clamp(0, left_max))
                + i64::from(right_max - right.damage.unwrap_or(0).clamp(0, right_max))
                + i64::from(max) * 5 / 100;
            item.count = 1;
            item.extended_mut().max_damage = Some(max as u32);
            item.damage = Some((i64::from(max) - remaining).max(0) as i32);
        } else {
            // Vanilla requires complete component identity for nondamageable pairs,
            // not merely equal enchantments after stripping them.
            if is_book(rules, left) || rules.max_stack(left) < 2 || left != right {
                return empty();
            }
            item.count = 2;
        }
    }
    let curses = crafting_enchantments_mut(rules, &mut item);
    curses.retain(|e| catalog.get(&e.id).is_some_and(|d| d.curse));
    if !left.is_empty() && !right.is_empty() {
        // Merging noncurses then immediately removing them is unnecessary.
        // An existing left curse retains its level even if the right is stronger.
        for addition in crafting_enchantments(rules, right) {
            if addition.level > 0
                && catalog.get(&addition.id).is_some_and(|d| d.curse)
                && !curses.iter().any(|e| e.id == addition.id)
            {
                curses.push(addition.clone());
            }
        }
    }
    curses.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    let curse_count = curses.len();
    if is_book(rules, &item) && curse_count == 0 {
        let book = Identifier::parse("minecraft:book").expect("vanilla book identifier");
        let Some(raw) = rules.items.id_of(&book) else {
            return empty();
        };
        item.item_id = raw;
    }
    let mut penalty = 0;
    for _ in 0..curse_count {
        penalty = increased_repair_cost(penalty);
    }
    item.extended_mut().repair_cost = penalty;
    let amount = experience_from(catalog, rules, left)
        .saturating_add(experience_from(catalog, rules, right));
    let experience = if amount > 0 {
        let half = amount / 2 + amount % 2;
        // ceil(sum / 2) + nextInt(ceil(sum / 2)): an odd sum is reachable.
        half.saturating_add(LegacyRandom::new(seed as i64).next_int(half))
    } else {
        0
    };
    GrindstoneResult { item, experience }
}

fn experience_from(catalog: &EnchantmentCatalog, rules: &ItemRules<'_>, input: &ItemStack) -> i32 {
    if input.is_empty() {
        return 0;
    }
    crafting_enchantments(rules, input)
        .iter()
        .fold(0_i32, |sum, enchantment| {
            match catalog.get(&enchantment.id) {
                Some(definition) if !definition.curse => {
                    sum.saturating_add(definition.min_cost(enchantment.level))
                }
                _ => sum,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::super::anvil::tests::{Data, catalog, id};
    use super::*;
    use mc_data::ItemEnchantment;

    #[test]
    fn empty_oversized_and_unenchanted_single_inputs_have_no_output_or_xp() {
        let data = Data::new();
        let catalog = catalog();
        assert_eq!(
            result(
                &catalog,
                &data.rules(),
                &ItemStack::EMPTY,
                &ItemStack::EMPTY,
                0
            ),
            empty()
        );
        assert_eq!(
            result(
                &catalog,
                &data.rules(),
                &data.stack("diamond_sword").with_damage(50),
                &ItemStack::EMPTY,
                0
            ),
            empty()
        );
        let mut left = data.book("sharpness", 1);
        left.count = 2;
        assert_eq!(
            result(&catalog, &data.rules(), &left, &ItemStack::EMPTY, 0),
            empty()
        );
    }

    #[test]
    fn removes_noncurses_preserves_components_and_resets_penalty_per_curse() {
        let data = Data::new();
        let catalog = catalog();
        let mut left = data
            .stack("diamond_sword")
            .with_damage(123)
            .with_custom_name("Named")
            .with_enchantment(id("sharpness"), 3)
            .with_enchantment(id("vanishing_curse"), 1);
        left.extended_mut().repair_cost = 127;
        left.extended_mut().custom_data = Some(vec![10, 0, 0, 0]);
        let output = result(&catalog, &data.rules(), &left, &ItemStack::EMPTY, 42);
        assert_eq!(output.item.damage, Some(123));
        assert_eq!(output.item.custom_name, left.custom_name);
        assert_eq!(
            output.item.extended().custom_data,
            left.extended().custom_data
        );
        assert_eq!(
            output.item.enchantments,
            vec![ItemEnchantment {
                id: id("vanishing_curse"),
                level: 1
            }]
        );
        assert_eq!(output.item.extended().repair_cost, 1);
        let sum = catalog.get(&id("sharpness")).unwrap().min_cost(3);
        assert!(output.experience >= (sum + 1) / 2 && output.experience < sum);
        let right_only = result(&catalog, &data.rules(), &ItemStack::EMPTY, &left, 42);
        assert_eq!(right_only, output);
        let curse_only = data
            .stack("diamond_sword")
            .with_enchantment(id("vanishing_curse"), 1);
        assert_eq!(
            result(&catalog, &data.rules(), &curse_only, &ItemStack::EMPTY, 0).experience,
            0
        );
    }

    #[test]
    fn curse_free_books_become_books_and_cursed_books_keep_stored_enchants() {
        let data = Data::new();
        let catalog = catalog();
        let left = data.book("sharpness", 2).with_custom_name("Book");
        let output = result(&catalog, &data.rules(), &left, &ItemStack::EMPTY, 123);
        assert_eq!(output.item.item_id, data.stack("book").item_id);
        assert!(output.item.extended().stored_enchantments.is_empty());
        assert_eq!(output.item.custom_name.as_deref(), Some("Book"));
        assert_eq!(output.item.extended().repair_cost, 0);
        let mut cursed = data.book("binding_curse", 1);
        cursed
            .extended_mut()
            .stored_enchantments
            .push(ItemEnchantment {
                id: id("vanishing_curse"),
                level: 1,
            });
        let output = result(&catalog, &data.rules(), &cursed, &ItemStack::EMPTY, 123);
        assert_eq!(output.item.item_id, cursed.item_id);
        assert_eq!(output.item.extended().stored_enchantments.len(), 2);
        assert_eq!(output.item.extended().repair_cost, 3);
        assert_eq!(output.experience, 0);
        assert_eq!(result(&catalog, &data.rules(), &left, &left, 0), empty());
    }

    #[test]
    fn combines_durability_with_five_percent_and_maximum_component_override() {
        let data = Data::new();
        let catalog = catalog();
        let left = data
            .stack("diamond_sword")
            .with_damage(1400)
            .with_custom_name("Left");
        let right = data
            .stack("diamond_sword")
            .with_damage(1300)
            .with_custom_name("Right");
        let output = result(&catalog, &data.rules(), &left, &right, 0);
        assert_eq!(output.item.damage, Some(1061));
        assert_eq!(output.item.extended().max_damage, Some(1561));
        assert_eq!(output.item.custom_name.as_deref(), Some("Left"));
        assert_eq!(output.item.count, 1);
        assert_eq!(output.experience, 0);
        let left = left.with_max_damage(100).unwrap().with_damage(90);
        let right = right.with_max_damage(200).unwrap().with_damage(180);
        let output = result(&catalog, &data.rules(), &left, &right, 0);
        assert_eq!(output.item.extended().max_damage, Some(200));
        assert_eq!(output.item.damage, Some(160));
        let repaired = result(
            &catalog,
            &data.rules(),
            &data.stack("diamond_sword"),
            &data.stack("diamond_sword"),
            0,
        );
        assert_eq!(repaired.item.damage, Some(0));
    }

    #[test]
    fn merging_keeps_left_curse_level_adds_right_curses_and_counts_both_xp_inputs() {
        let data = Data::new();
        let catalog = catalog();
        let left = data
            .stack("diamond_sword")
            .with_enchantment(id("vanishing_curse"), 1)
            .with_enchantment(id("sharpness"), 2);
        let right = data
            .stack("diamond_sword")
            .with_enchantment(id("vanishing_curse"), 2)
            .with_enchantment(id("binding_curse"), 1)
            .with_enchantment(id("sharpness"), 3);
        let output = result(&catalog, &data.rules(), &left, &right, 5);
        assert_eq!(output.item.enchantments.len(), 2);
        assert_eq!(
            output
                .item
                .enchantments
                .iter()
                .find(|e| e.id == id("vanishing_curse"))
                .unwrap()
                .level,
            1
        );
        assert_eq!(output.item.extended().repair_cost, 3);
        let definition = catalog.get(&id("sharpness")).unwrap();
        let sum = definition.min_cost(2) + definition.min_cost(3);
        let half = (sum + 1) / 2;
        assert!(output.experience >= half && output.experience < 2 * half);
        assert_eq!(output.experience, sum); // odd total can refund the entire sum
    }

    #[test]
    fn nondamageable_pairs_require_full_component_identity_and_stackability() {
        let data = Data::new();
        let catalog = catalog();
        let left = data.stack("stone").with_enchantment(id("sharpness"), 1);
        let output = result(&catalog, &data.rules(), &left, &left, 0);
        assert_eq!(output.item.count, 2);
        assert!(output.item.enchantments.is_empty());
        let named = left.clone().with_custom_name("Different");
        assert_eq!(result(&catalog, &data.rules(), &left, &named, 0), empty());
        let different = data.stack("diamond_sword");
        assert_eq!(
            result(&catalog, &data.rules(), &left, &different, 0),
            empty()
        );
        let mut changed = left.clone();
        changed.extended_mut().repair_cost = 1;
        assert_eq!(result(&catalog, &data.rules(), &left, &changed, 0), empty());
        let modeled = left.clone().with_item_model(id("example:custom_stone"));
        assert_eq!(result(&catalog, &data.rules(), &left, &modeled, 0), empty());
        let sword = data.stack("diamond_sword");
        let modeled = sword.clone().with_item_model(id("example:custom_sword"));
        let output = result(&catalog, &data.rules(), &sword, &modeled, 0);
        assert!(!output.item.is_empty());
        assert_eq!(output.item.item_model, sword.item_model);
    }

    #[test]
    fn random_half_award_is_deterministic_and_uses_vanilla_bounds() {
        assert_eq!(LegacyRandom::new(0).next_int(1), 0);
        assert_eq!(LegacyRandom::new(0).next_int(10), 0);
        assert_eq!(LegacyRandom::new(0).next_int(16), 11);
        let data = Data::new();
        let catalog = catalog();
        let left = data.book("sharpness", 1);
        // Sharpness I has min cost one: ceil(1/2) + nextInt(1) = one.
        assert_eq!(
            result(&catalog, &data.rules(), &left, &ItemStack::EMPTY, 0).experience,
            1
        );
        let left = data.book("efficiency", 5);
        let sum = catalog.get(&id("efficiency")).unwrap().min_cost(5);
        for seed in 0..256 {
            let output = result(&catalog, &data.rules(), &left, &ItemStack::EMPTY, seed);
            let half = (sum + 1) / 2;
            assert!(output.experience >= half && output.experience < 2 * half);
            assert_eq!(
                output,
                result(&catalog, &data.rules(), &left, &ItemStack::EMPTY, seed)
            );
        }
    }
}
