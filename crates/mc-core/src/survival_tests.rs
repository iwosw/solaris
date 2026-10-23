use mc_data::food::FoodEntry;
use mc_data::item_components::{ItemArmorFacts, ItemFacts, ItemFactsTable};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::tags::TagsData;
use mc_data::{Identifier, ItemEnchantment, ItemStack};

use super::{
    DamageKind, Difficulty, Experience, MAX_AIR, Vitals, armor_wear, fall_damage, reduce_damage,
};
use crate::inventory::ItemRules;

fn ticks(vitals: &mut Vitals, count: usize, difficulty: Difficulty) -> Vec<(DamageKind, f32)> {
    let mut damage = Vec::new();
    for _ in 0..count {
        damage.extend(vitals.tick(difficulty, false, true, true, true).damage);
    }
    damage
}

#[test]
fn saturation_heals_fast_when_food_is_full() {
    let mut v = Vitals {
        health: 10.0,
        saturation: 5.0,
        ..Vitals::default()
    };
    ticks(&mut v, 9, Difficulty::Normal);
    assert_eq!(v.health, 10.0);
    ticks(&mut v, 1, Difficulty::Normal);
    assert!((v.health - (10.0 + 5.0 / 6.0)).abs() < 1e-5);
    assert_eq!(v.saturation, 5.0);
    ticks(&mut v, 1, Difficulty::Normal);
    assert_eq!(v.saturation, 4.0);
}

#[test]
fn food_eighteen_heals_one_point_every_eighty_ticks() {
    let mut v = Vitals {
        health: 10.0,
        food: 18,
        saturation: 0.0,
        ..Vitals::default()
    };
    ticks(&mut v, 79, Difficulty::Normal);
    assert_eq!(v.health, 10.0);
    ticks(&mut v, 1, Difficulty::Normal);
    assert_eq!(v.health, 11.0);
    ticks(&mut v, 1, Difficulty::Normal);
    assert_eq!(
        v.food, 17,
        "6 exhaustion costs one food point with no saturation"
    );
}

#[test]
fn starvation_hurts_on_hard_but_stops_at_ten_on_easy() {
    let mut hard = Vitals {
        food: 0,
        saturation: 0.0,
        ..Vitals::default()
    };
    let damage = ticks(&mut hard, 80, Difficulty::Hard);
    assert_eq!(damage, vec![(DamageKind::Starvation, 1.0)]);

    let mut easy = Vitals {
        health: 10.0,
        food: 0,
        saturation: 0.0,
        ..Vitals::default()
    };
    assert!(ticks(&mut easy, 400, Difficulty::Easy).is_empty());
}

#[test]
fn drowning_starts_after_the_air_supply_runs_out() {
    let mut v = Vitals::default();
    let mut damage = Vec::new();
    for _ in 0..320 {
        damage.extend(v.tick(Difficulty::Normal, true, true, true, true).damage);
    }
    assert_eq!(damage, vec![(DamageKind::Drowning, 2.0)]);
    v.tick(Difficulty::Normal, false, true, true, true);
    assert_eq!(v.air, 4);
    let mut creative = Vitals::default();
    creative.tick(Difficulty::Normal, true, false, true, true);
    assert_eq!(creative.air, MAX_AIR);
}

#[test]
fn disabling_natural_regeneration_keeps_starvation_and_drowning() {
    let mut vitals = Vitals {
        health: 10.0,
        ..Vitals::default()
    };
    for _ in 0..80 {
        assert!(
            vitals
                .tick(Difficulty::Hard, false, true, true, false)
                .damage
                .is_empty()
        );
    }
    assert_eq!(vitals.health, 10.0);
    assert_eq!(vitals.saturation, 5.0);
    vitals.food = 0;
    vitals.saturation = 0.0;
    let mut damage = Vec::new();
    for _ in 0..320 {
        damage.extend(
            vitals
                .tick(Difficulty::Hard, true, true, true, false)
                .damage,
        );
    }
    assert_eq!(
        damage
            .iter()
            .filter(|(kind, _)| *kind == DamageKind::Starvation)
            .count(),
        4
    );
    assert!(damage.contains(&(DamageKind::Drowning, 2.0)));
}

#[test]
fn exhaustion_spends_saturation_before_food() {
    let mut v = Vitals {
        saturation: 1.0,
        ..Vitals::default()
    };
    v.add_exhaustion(4.0);
    ticks(&mut v, 1, Difficulty::Normal);
    assert_eq!((v.food, v.saturation), (20, 0.0));
    v.add_exhaustion(8.5);
    ticks(&mut v, 2, Difficulty::Normal);
    assert_eq!(v.food, 18);
    assert!((v.exhaustion - 0.5).abs() < 1e-6);
}

#[test]
fn eating_caps_food_and_saturation_by_food() {
    let mut v = Vitals {
        food: 15,
        saturation: 0.0,
        ..Vitals::default()
    };
    let steak = FoodEntry {
        food: 8,
        saturation: 12.8,
        can_always_eat: false,
    };
    assert!(v.can_eat(&steak));
    v.eat(&steak);
    assert_eq!(v.food, 20);
    assert_eq!(v.saturation, 12.8);
    assert!(!v.can_eat(&steak));
}

#[test]
fn experience_levels_follow_vanilla_thresholds() {
    let mut xp = Experience::default();
    xp.add_points(7);
    assert_eq!((xp.level, xp.progress, xp.total), (1, 0.0, 7));
    xp.add_points(-1);
    assert_eq!((xp.level, xp.total), (0, 6));
    assert!((xp.progress - 6.0 / 7.0).abs() < 1e-6, "{}", xp.progress);
    xp.add_points(-100);
    assert_eq!(xp, Experience::default());
    xp.add_points(7 + 9 + 11);
    assert_eq!(xp.level, 3);
    assert_eq!(Experience::points_for_level(15), 37);
    assert_eq!(Experience::points_for_level(30), 112);
    let veteran = Experience {
        level: 40,
        ..Experience::default()
    };
    assert_eq!(veteran.death_drop(), 100);
}

#[test]
fn experience_maximum_level_consumes_full_bars_without_wrapping() {
    for level in [i32::MAX - 1, i32::MAX] {
        let mut xp = Experience {
            level,
            progress: 0.75,
            total: i32::MAX,
        };
        xp.add_points(i32::MAX);
        assert_eq!(xp.level, i32::MAX);
        assert_eq!(xp.progress, 0.75);
        assert_eq!(xp.total, i32::MAX);
        xp.add_points(i32::MAX);
        assert_eq!(xp.level, i32::MAX);
        assert_eq!(xp.progress, 0.75);
        xp.add_points(-i32::MAX);
        assert_eq!(xp.level, i32::MAX - 1);
        assert_eq!(xp.progress, 0.75);
        assert_eq!(xp.total, 0);
    }
}

#[test]
fn falls_hurt_one_point_per_block_beyond_three() {
    assert_eq!(fall_damage(3.0), 0.0);
    assert_eq!(fall_damage(3.5), 1.0);
    assert_eq!(fall_damage(10.0), 7.0);
}

struct ArmorData {
    items: ItemRegistry,
    facts: ItemFactsTable,
    tags: TagsData,
}

impl ArmorData {
    fn new() -> Self {
        let armor = |slot: &str, armor| ItemFacts {
            armor: Some(ItemArmorFacts {
                slot: slot.to_owned(),
                armor,
                toughness: 2.0,
            }),
            ..ItemFacts::default()
        };
        let id = |name| Identifier::parse(name).unwrap();
        Self {
            items: ItemRegistry::from_report(&[
                ItemReport {
                    id: id("minecraft:diamond_chestplate"),
                    protocol_id: 1,
                },
                ItemReport {
                    id: id("minecraft:diamond_boots"),
                    protocol_id: 2,
                },
            ]),
            facts: ItemFactsTable::from_entries([
                (id("minecraft:diamond_chestplate"), armor("chest", 8.0)),
                (id("minecraft:diamond_boots"), armor("feet", 3.0)),
            ]),
            tags: TagsData::default(),
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

#[test]
fn armor_reduces_melee_but_not_falls_while_feather_falling_does() {
    let data = ArmorData::new();
    let rules = data.rules();
    let mut boots = ItemStack::new(2, 1);
    boots.enchantments = vec![ItemEnchantment {
        id: Identifier::parse("minecraft:feather_falling").unwrap(),
        level: 4,
    }];
    let chest_only = [
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::new(1, 1),
        ItemStack::EMPTY,
    ];
    let melee = reduce_damage(&rules, &chest_only, 10.0, DamageKind::Melee);
    assert!((melee - 8.4).abs() < 1e-4, "{melee}");
    assert_eq!(
        reduce_damage(&rules, &chest_only, 10.0, DamageKind::Fall),
        10.0
    );
    let chest_on_head = [
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::new(1, 1),
    ];
    assert_eq!(
        reduce_damage(&rules, &chest_on_head, 10.0, DamageKind::Melee),
        10.0
    );
    for kind in [DamageKind::Generic, DamageKind::OnFire] {
        assert_eq!(reduce_damage(&rules, &chest_only, 10.0, kind), 10.0);
    }
    let in_fire = reduce_damage(&rules, &chest_only, 10.0, DamageKind::InFire);
    assert!((in_fire - 8.4).abs() < 1e-4, "{in_fire}");

    let with_boots = [boots, ItemStack::EMPTY, ItemStack::EMPTY, ItemStack::EMPTY];
    let fall = reduce_damage(&rules, &with_boots, 10.0, DamageKind::Fall);
    assert!((fall - 5.2).abs() < 1e-4, "{fall}");
    assert_eq!(
        reduce_damage(&rules, &with_boots, 10.0, DamageKind::Void),
        10.0
    );
}

#[test]
fn armor_wears_a_quarter_of_the_damage_at_least_one() {
    assert_eq!(armor_wear(1.0, DamageKind::Melee), 1);
    assert_eq!(armor_wear(9.0, DamageKind::Melee), 2);
    assert_eq!(armor_wear(9.0, DamageKind::Fall), 0);
}

#[test]
fn lethal_damage_reports_death_once() {
    let mut v = Vitals::default();
    assert!(!v.hurt(DamageKind::Melee, 10.0));
    assert_eq!(v.exhaustion, 0.1);
    assert!(!v.hurt(DamageKind::Fall, 5.0));
    assert_eq!(v.exhaustion, 0.1);
    assert!(v.hurt(DamageKind::Melee, 100.0));
    assert!(!v.hurt(DamageKind::Melee, 1.0));
    v.respawn();
    assert_eq!(v, Vitals::default());
}

#[test]
fn parity_peaceful_food_never_spends_but_recovery_obeys_health_rule() {
    let mut v = Vitals {
        health: 10.0,
        food: 10,
        saturation: 0.0,
        ..Vitals::default()
    };
    for _ in 0..20 {
        v.add_exhaustion(4.0);
        assert!(
            v.tick(Difficulty::Peaceful, false, true, true, false)
                .damage
                .is_empty()
        );
    }
    assert_eq!(v.health, 10.0);
    assert_eq!(v.food, 12);
    for _ in 0..20 {
        v.add_exhaustion(4.0);
        v.tick(Difficulty::Peaceful, false, true, true, true);
    }
    assert_eq!(v.health, 11.0);
    assert_eq!(v.food, 14);
}

#[test]
fn parity_starvation_bypasses_protection_enchantments() {
    let data = ArmorData::new();
    let mut armor = ItemStack::new(1, 1);
    armor.enchantments.push(ItemEnchantment {
        id: Identifier::parse("minecraft:protection").unwrap(),
        level: 4,
    });
    let armor = std::array::from_fn::<_, 4, _>(|_| armor.clone());
    assert_eq!(
        reduce_damage(&data.rules(), &armor, 1.0, DamageKind::Starvation),
        1.0
    );
}
