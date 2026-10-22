use std::collections::BTreeMap;

use mc_data::item_components::{ItemFacts, ItemFactsTable};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::tags::TagsData;
use mc_data::{Identifier, ItemEnchantment, ItemStack};

use super::{HIT_KNOCKBACK, Swing, attack_strength, knockback_velocity, melee_hit};
use crate::inventory::ItemRules;

const SWORD: u32 = 1;
const STICK: u32 = 2;

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}

struct Data {
    items: ItemRegistry,
    facts: ItemFactsTable,
    tags: TagsData,
}

impl Data {
    fn new() -> Self {
        let swords = BTreeMap::from([(id("minecraft:swords"), vec![SWORD as i32])]);
        Self {
            items: ItemRegistry::from_report(&[
                ItemReport {
                    id: id("minecraft:iron_sword"),
                    protocol_id: SWORD,
                },
                ItemReport {
                    id: id("minecraft:stick"),
                    protocol_id: STICK,
                },
            ]),
            facts: ItemFactsTable::from_entries([(
                id("minecraft:iron_sword"),
                ItemFacts {
                    max_stack_size: Some(1),
                    attack_damage_modifier: Some(5.0),
                    attack_speed_modifier: Some(-2.4),
                    weapon: true,
                    weapon_damage_per_attack: Some(1),
                    ..ItemFacts::default()
                },
            )]),
            tags: TagsData::from_registries(BTreeMap::from([(id("minecraft:item"), swords)])),
        }
    }

    fn rules(&self) -> ItemRules<'_> {
        ItemRules {
            items: &self.items,
            facts: &self.facts,
            tags: &self.tags,
            recipes: &[],
        }
    }
}

fn swing(ticks: u32) -> Swing {
    Swing {
        ticks_since_reset: ticks,
        falling: false,
        on_ground: true,
        sprinting: false,
        crit_blocked: false,
        melee_bonus: 0.0,
        attack_speed_multiplier: 1.0,
    }
}

#[test]
fn iron_sword_recharges_in_twelve_and_a_half_ticks() {
    let data = Data::new();
    let sword = ItemStack::new(SWORD, 1);
    assert!(attack_strength(&data.rules(), &sword, 12) == 1.0);
    assert!(attack_strength(&data.rules(), &sword, 5) < 0.5);
    let charged = melee_hit(&data.rules(), &sword, swing(13));
    assert_eq!(charged.strength, 1.0);
    assert_eq!(charged.damage, 6.0);
}

#[test]
fn full_swing_deals_attribute_damage_and_spam_clicking_deals_a_fifth() {
    let data = Data::new();
    let sword = ItemStack::new(SWORD, 1);
    let full = melee_hit(&data.rules(), &sword, swing(20));
    assert_eq!(full.damage, 6.0);
    assert_eq!(full.weapon_wear, 1);
    let spam = melee_hit(&data.rules(), &sword, swing(0));
    assert!(spam.damage < 6.0 * 0.25, "{}", spam.damage);
    assert!(spam.sweep_damage.is_none());
}

#[test]
fn falling_full_swing_crits_for_one_and_a_half() {
    let data = Data::new();
    let sword = ItemStack::new(SWORD, 1);
    let crit = melee_hit(
        &data.rules(),
        &sword,
        Swing {
            falling: true,
            on_ground: false,
            ..swing(20)
        },
    );
    assert!(crit.critical);
    assert_eq!(crit.damage, 9.0);
    let in_water = melee_hit(
        &data.rules(),
        &sword,
        Swing {
            falling: true,
            on_ground: false,
            crit_blocked: true,
            ..swing(20)
        },
    );
    assert!(!in_water.critical);
}

#[test]
fn sharpness_adds_after_the_crit() {
    let data = Data::new();
    let mut sword = ItemStack::new(SWORD, 1);
    sword.enchantments = vec![ItemEnchantment {
        id: id("minecraft:sharpness"),
        level: 5,
    }];
    let hit = melee_hit(
        &data.rules(),
        &sword,
        Swing {
            falling: true,
            on_ground: false,
            ..swing(20)
        },
    );
    assert_eq!(hit.damage, 9.0 + 3.0);
}

#[test]
fn only_grounded_non_sprinting_sword_swings_sweep() {
    let data = Data::new();
    let sword = ItemStack::new(SWORD, 1);
    assert_eq!(
        melee_hit(&data.rules(), &sword, swing(20)).sweep_damage,
        Some(1.0)
    );
    let sprint = melee_hit(
        &data.rules(),
        &sword,
        Swing {
            sprinting: true,
            ..swing(20)
        },
    );
    assert!(sprint.sweep_damage.is_none());
    assert_eq!(sprint.bonus_knockback, 0.5);
    let stick = melee_hit(&data.rules(), &ItemStack::new(STICK, 1), swing(20));
    assert!(stick.sweep_damage.is_none());
    assert_eq!(stick.damage, 1.0);
    assert_eq!(stick.weapon_wear, 0);
}

#[test]
fn sweeping_edge_scales_weapon_damage_and_partial_charge_and_sharpness() {
    let data = Data::new();
    let mut sword = ItemStack::new(SWORD, 1);
    sword.enchantments = vec![ItemEnchantment {
        id: id("minecraft:sweeping_edge"),
        level: 3,
    }];
    assert_eq!(
        melee_hit(&data.rules(), &sword, swing(20)).sweep_damage,
        Some(5.5)
    );
    let partial = melee_hit(&data.rules(), &sword, swing(11));
    assert!(partial.strength > 0.9 && partial.strength < 1.0);
    assert_eq!(
        partial.sweep_damage,
        Some((1.0 + 0.75 * partial.damage) * partial.strength)
    );
    sword.enchantments.push(ItemEnchantment {
        id: id("minecraft:sharpness"),
        level: 1,
    });
    assert_eq!(
        melee_hit(&data.rules(), &sword, swing(20)).sweep_damage,
        Some(6.5)
    );
}

#[test]
fn knockback_pushes_away_from_the_attacker_and_lifts_grounded_targets() {
    let velocity = knockback_velocity([0.0, 0.0, 0.0], true, HIT_KNOCKBACK, 0.0, 1.0, 0.0);
    assert_eq!(velocity, [0.0, 0.4, -0.4]);
    let resisted = knockback_velocity([0.2, 0.0, 0.0], true, HIT_KNOCKBACK, 0.0, 1.0, 1.0);
    assert_eq!(resisted, [0.2, 0.0, 0.0]);
}
#[test]
fn same_tick_swap_keeps_old_attributes_but_live_enchantments() {
    // MC-28289 oracle numbers: full-strength iron sword (5.0 modifier) hits
    // 6.0; a same-tick swap to a plain stick must still hit 6.0 (OLD
    // attack_damage), while enchantments read live from the new item.
    use super::melee_hit_with_attributes;
    let data = Data::new();
    let sword = ItemStack::new(SWORD, 1);
    let stick = ItemStack::new(STICK, 1);
    let full_sword = melee_hit(&data.rules(), &sword, swing(20));
    assert_eq!(full_sword.damage, 6.0);
    // Same-tick swap sword -> stick: attributes from the sword.
    let swapped = melee_hit_with_attributes(&data.rules(), &stick, &sword, swing(20));
    assert_eq!(swapped.damage, full_sword.damage);
    assert_eq!(swapped.strength, full_sword.strength);
    // Same-tick swap stick -> sword: attributes from the stick (1.0 base).
    let swapped_back = melee_hit_with_attributes(&data.rules(), &sword, &stick, swing(20));
    assert_eq!(swapped_back.damage, 1.0);
    // Enchantments stay live: sharpness 5 on the NEW stick still applies.
    let mut sharp_stick = ItemStack::new(STICK, 1);
    sharp_stick.enchantments = vec![ItemEnchantment {
        id: id("minecraft:sharpness"),
        level: 5,
    }];
    let enchanted_swap = melee_hit_with_attributes(&data.rules(), &sharp_stick, &sword, swing(20));
    assert_eq!(enchanted_swap.damage, 6.0 + 3.0);
}
