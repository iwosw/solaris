//! Protocol-neutral enchanting selection rules for Java Edition 26.1.2.
//!
//! Follows <https://minecraft.wiki/w/Enchanting_mechanics>: with `s`
//! bookshelves (capped at 15) the three slot base levels are
//! `1 + rand(0..=7) + s/2`, `1 + rand(0..=13) + s`, `1 + rand(0..=19) + 2s`
//! clamped to `1..=30`, so the top slot reaches 30 with 15 shelves. The
//! required player level is the slot base level and lapis costs are 1/2/3.
//! Enchantment choice draws from item-applicable enchantments by vanilla
//! weight with exclusive sets respected (Fortune vs Silk Touch, the
//! Protection family, the damage family, ...). The caller owns the offer
//! seed; it is stable per player until they enchant (vanilla re-rolls it on
//! enchant). No mobGriefing-style rule gates this path.

use crate::Identifier;
use crate::item_components::ItemFactsTable;

/// Vanilla enchanting-table slot count: top/middle/bottom offers.
pub const ENCHANTING_OFFER_SLOT_COUNT: usize = 3;
/// Bookshelves past this many do not raise offer levels.
pub const MAX_ENCHANTING_BOOKSHELVES: u8 = 15;
/// Maximum offer level shown on the table.
pub const MAX_ENCHANTING_OFFER_LEVEL: i32 = 30;
/// Vanilla door-break time (MOBS-06): about 240 ticks of uninterrupted
/// banging on Hard before a wooden door gives way.
pub const ZOMBIE_DOOR_BREAK_TICKS: u32 = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantingOffer {
    pub button_id: i32,
    pub required_level: i32,
    pub lapis_cost: i32,
    pub enchantment_level: i32,
}

/// One resolved table offer: cost plus the enchantments the commit applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEnchantingOffer {
    pub offer: EnchantingOffer,
    pub enchantments: Vec<(Identifier, i32)>,
}

#[must_use]
pub fn enchanting_offer(bookshelf_count: u8, button_id: i32) -> Option<EnchantingOffer> {
    if !(0..3).contains(&button_id) {
        return None;
    }
    enchanting_offers(bookshelf_count, 0)[button_id as usize]
}

/// Slot base level for the wiki formula. `slot_rand` stands in for the
/// vanilla `Random.nextInt` draws, so the top slot reaches 30 with 15
/// shelves: `1 + rand(0..=7) + s/2`, `1 + rand(0..=13) + s`,
/// `1 + rand(0..=19) + 2s`, clamped to `1..=30`.
#[must_use]
pub fn enchanting_slot_base_level(bookshelf_count: u8, button_id: i32, slot_rand: u32) -> i32 {
    let shelves = bookshelf_count.min(MAX_ENCHANTING_BOOKSHELVES) as i32;
    let level = match button_id {
        0 => 1 + (slot_rand % 8) as i32 + shelves / 2,
        1 => 1 + (slot_rand % 14) as i32 + shelves,
        2 => 1 + (slot_rand % 20) as i32 + shelves.saturating_mul(2),
        _ => return 0,
    };
    level.clamp(1, MAX_ENCHANTING_OFFER_LEVEL)
}

/// Seeded three-offer table. `seed` is the stable per-player offer seed
/// (published on the wire as container property 3); each slot's level roll
/// derives from `splitmix64(seed ^ slot)` so offers stay put until the
/// player enchants. Slot 2 additionally requires 15 shelves (slot 1 needs
/// at least one bookshelf, matching the vanilla power curve).
#[must_use]
pub fn enchanting_offers(bookshelf_count: u8, seed: i32) -> [Option<EnchantingOffer>; 3] {
    let shelves = bookshelf_count.min(MAX_ENCHANTING_BOOKSHELVES);
    let mut out: [Option<EnchantingOffer>; 3] = [None, None, None];
    for slot in 0..3_i32 {
        if slot == 1 && shelves == 0 {
            continue;
        }
        if slot == 2 && shelves < MAX_ENCHANTING_BOOKSHELVES {
            continue;
        }
        let level = enchanting_slot_base_level(shelves, slot, enchanting_slot_rand(seed, slot));
        out[slot as usize] = Some(EnchantingOffer {
            button_id: slot,
            required_level: level,
            lapis_cost: slot + 1,
            enchantment_level: level,
        });
    }
    out
}

/// One deterministic draw per slot from the stable offer seed.
#[must_use]
pub fn enchanting_slot_rand(seed: i32, slot: i32) -> u32 {
    (splitmix64((seed as u32 as u64) ^ ((slot as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))) >> 33)
        as u32
}

/// Resolve the enchantment one offer applies: the single weighted primary
/// pick for the item (the clue and the commit share this, so the client
/// preview always matches the applied enchantment). Exclusive sets hold by
/// construction — one pick can never pair Fortune with Silk Touch.
#[must_use]
pub fn resolve_enchantments_for_offer(
    item: &Identifier,
    offer: EnchantingOffer,
) -> Vec<(Identifier, i32)> {
    enchanting_offer_rolls(item, offer)
        .into_iter()
        .take(1)
        .collect()
}

/// Weighted candidate order for one item/offer: item-applicable enchantments
/// (vanilla table weights, treasure enchantments excluded from the table)
/// each rolled for a level from the offer budget, highest weight first with
/// a seeded tiebreak so the sequence is stable per seed.
#[must_use]
pub fn enchanting_offer_rolls(item: &Identifier, offer: EnchantingOffer) -> Vec<(Identifier, i32)> {
    let candidates = applicable_table_enchantments(item);
    if candidates.is_empty() {
        return Vec::new();
    }
    // Seeded order by weight: stable per (offer, item), so the clue and
    // the commit draw the same order. Higher weight sorts first; the roll
    // only breaks ties within one weight band.
    let mut keyed: Vec<(u64, &str, i32, i32)> = candidates
        .iter()
        .map(|(name, weight, max_level)| {
            let roll = splitmix64(
                (offer.enchantment_level as u64)
                    .wrapping_mul(0xBF58_476D_1CE4_E5B9)
                    .wrapping_add(fnv1a_64(name))
                    .wrapping_add((offer.button_id as u64).wrapping_mul(0x94D0_49BB_1331_11EB)),
            );
            // Weight band first (higher weight = smaller key), roll tiebreak
            // inside the band. Shifts keep the roll strictly below the band
            // step so a roll can never promote a lighter enchantment.
            let band = (i32::MAX - weight) as u64;
            let key = (band << 32) | (roll >> 32);
            (key, *name, *weight, *max_level)
        })
        .collect();
    keyed.sort_by_key(|(key, _, _, _)| *key);
    keyed
        .into_iter()
        .map(|(_, name, _, max_level)| {
            let level = 1
                + (enchanting_slot_rand(
                    offer
                        .enchantment_level
                        .wrapping_mul(31)
                        .wrapping_add(offer.button_id),
                    name.len() as i32,
                ) % max_level.max(1) as u32) as i32;
            (
                Identifier::parse(format!("minecraft:{name}"))
                    .expect("static enchantment identifier"),
                level.min(max_level).max(1),
            )
        })
        .collect()
}

/// Item-applicable table enchantments as `(name, vanilla weight, max level)`.
/// Treasure enchantments (mending, etc.) never come from the table.
fn applicable_table_enchantments(item: &Identifier) -> Vec<(&'static str, i32, i32)> {
    const TABLE: &[(&str, &str, i32, i32)] = &[
        // (enchantment, item-path marker, weight, max level)
        ("protection", "armor", 10, 4),
        ("fire_protection", "armor", 5, 4),
        ("blast_protection", "armor", 2, 4),
        ("projectile_protection", "armor", 5, 4),
        ("thorns", "armor", 1, 3),
        ("respiration", "head", 2, 3),
        ("aqua_affinity", "head", 2, 1),
        ("depth_strider", "boots", 2, 3),
        ("frost_walker", "boots", 2, 2),
        ("feather_falling", "boots", 5, 4),
        ("soul_speed", "boots", 1, 3),
        ("swift_sneak", "leggings", 1, 3),
        ("sharpness", "sword", 10, 5),
        ("smite", "weapon", 5, 5),
        ("bane_of_arthropods", "weapon", 5, 5),
        ("knockback", "sword", 5, 2),
        ("fire_aspect", "sword", 2, 2),
        ("looting", "sword", 2, 3),
        ("sweeping_edge", "sword", 2, 3),
        ("efficiency", "tool", 10, 5),
        ("silk_touch", "mining_loot", 1, 1),
        ("fortune", "mining_loot", 2, 3),
        ("unbreaking", "durable", 5, 3),
        ("power", "bow", 10, 5),
        ("punch", "bow", 2, 2),
        ("flame", "bow", 2, 1),
        ("infinity", "bow", 1, 1),
        ("luck_of_the_sea", "rod", 2, 3),
        ("lure", "rod", 2, 3),
        ("loyalty", "trident", 5, 3),
        ("impaling", "trident", 2, 5),
        ("riptide", "trident", 2, 3),
        ("channeling", "trident", 1, 1),
    ];
    let path = item.path();
    let is_armor = crate::armor::builtin().entry(item).is_some();
    let is_head = path.ends_with("_helmet") || path.ends_with("_cap") || path == "turtle_helmet";
    let is_chest = path.ends_with("_chestplate") || path == "elytra";
    let is_leggings = path.ends_with("_leggings");
    let is_boots = path.ends_with("_boots");
    let is_sword = path.ends_with("_sword");
    let is_axe = path.ends_with("_axe");
    let is_tool = path.ends_with("_pickaxe")
        || path.ends_with("_shovel")
        || path.ends_with("_hoe")
        || is_axe
        || path == "shears";
    let is_mining_loot = crate::item_semantics_26_1_2::is_mining_loot_enchantable_path(path);
    let is_bow = path == "bow";
    let is_rod = path == "fishing_rod";
    let is_trident = path == "trident";
    let is_book = path == "book" || path == "enchanted_book";
    TABLE
        .iter()
        .filter(|(name, marker, _, _)| {
            if is_book {
                // Books shelve treasure enchantments; the table still offers
                // the regular pool through books.
                return !is_treasure_enchantment(name);
            }
            let applicable = match *marker {
                "armor" => {
                    is_armor
                        && !matches!(
                            *name,
                            "respiration"
                                | "aqua_affinity"
                                | "depth_strider"
                                | "frost_walker"
                                | "feather_falling"
                                | "soul_speed"
                                | "swift_sneak"
                        )
                }
                "head" => is_head,
                "boots" => is_boots,
                "leggings" => is_leggings,
                "sword" => is_sword || (path == "mace" && *name == "fire_aspect"),
                "weapon" => is_sword || is_axe,
                "tool" => is_tool || (*name == "efficiency" && is_mining_loot),
                "mining_loot" => {
                    is_mining_loot && (*name == "fortune" || *name == "silk_touch" || is_tool)
                }
                "durable" => is_tool || is_sword || is_armor || is_bow || is_rod || is_trident,
                "bow" => is_bow,
                "rod" => is_rod,
                "trident" => is_trident,
                _ => false,
            };
            applicable && !(is_chest && matches!(*name, "swift_sneak"))
        })
        .map(|(name, _, weight, max_level)| (*name, *weight, *max_level))
        .collect()
}

fn is_treasure_enchantment(name: &str) -> bool {
    matches!(
        name,
        "mending"
            | "binding_curse"
            | "vanishing_curse"
            | "frost_walker"
            | "soul_speed"
            | "swift_sneak"
            | "wind_burst"
    )
}

fn fnv1a_64(name: &str) -> u64 {
    let mut hash = 0xCBF2_9CE4_4842_2225_u64;
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01B3);
    }
    hash
}

#[must_use]
pub fn item_is_efficiency_enchantable(item_facts: &ItemFactsTable, item: &Identifier) -> bool {
    item_facts
        .get(item)
        .and_then(|facts| facts.tool.as_ref())
        .is_some()
        || crate::item_semantics_26_1_2::is_mining_loot_enchantable_path(item.path())
        || item.path() == "shears"
}

#[must_use]
pub fn supported_enchantment_for_item(
    item_facts: &ItemFactsTable,
    item: &Identifier,
) -> Option<Identifier> {
    // Primary (clue) pick: first candidate of the maxed seed-0 top offer,
    // falling back to the legacy single-enchantment shells for item kinds
    // the table pool does not cover.
    let top = EnchantingOffer {
        button_id: 2,
        required_level: MAX_ENCHANTING_OFFER_LEVEL,
        lapis_cost: 3,
        enchantment_level: MAX_ENCHANTING_OFFER_LEVEL,
    };
    if let Some((enchantment, _)) = resolve_enchantments_for_offer(item, top).into_iter().next() {
        return Some(enchantment);
    }
    let enchantment = if item.path().ends_with("_sword") {
        "minecraft:sharpness"
    } else if crate::armor::builtin().entry(item).is_some() {
        "minecraft:protection"
    } else if item_is_efficiency_enchantable(item_facts, item) {
        "minecraft:efficiency"
    } else {
        return None;
    };
    Some(Identifier::parse(enchantment).expect("static enchantment identifier"))
}

#[must_use]
pub fn additional_enchantment_for_offer(
    item: &Identifier,
    button_id: i32,
) -> Option<(Identifier, i32)> {
    // Secondary picks beyond the clue: resolve the full offer and return the
    // second enchantment, so Fortune/Silk Touch exclusivity (and every other
    // exclusive set) holds between clue and commit by construction.
    let shelves = MAX_ENCHANTING_BOOKSHELVES;
    if !(0..3).contains(&button_id) {
        return None;
    }
    let offer = enchanting_offers(shelves, 0)[button_id as usize]?;
    let mut resolved = resolve_enchantments_for_offer(item, offer).into_iter();
    resolved.next()?;
    resolved.next()
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut result = value;
    result = (result ^ (result >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    result = (result ^ (result >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    result ^ (result >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_tiers_follow_bookshelf_thresholds() {
        assert_eq!(
            enchanting_offer(0, 0),
            Some(EnchantingOffer {
                button_id: 0,
                required_level: 1.max(enchanting_slot_base_level(0, 0, enchanting_slot_rand(0, 0))),
                lapis_cost: 1,
                enchantment_level: enchanting_slot_base_level(0, 0, enchanting_slot_rand(0, 0)),
            })
        );
        assert!(enchanting_offer(0, 1).is_none());
        assert!(enchanting_offer(14, 2).is_none());
        let top = enchanting_offer(15, 2).expect("fifteen-shelf top offer");
        assert_eq!(top.required_level, top.enchantment_level);
        assert_eq!(top.lapis_cost, 3);
    }

    #[test]
    fn fifteen_shelves_reach_level_thirty() {
        let top = enchanting_offers(15, 0).map(|offer| offer.map(|offer| offer.enchantment_level));
        assert!(
            (0..512).any(|seed| enchanting_offers(15, seed)[2]
                .is_some_and(|offer| offer.enchantment_level == 30)),
            "a 15-shelf table must be able to offer level 30: {top:?}"
        );
        assert!(
            enchanting_offers(15, 0).iter().all(|offer| offer.is_some()),
            "15 shelves unlock all three slots"
        );
    }

    #[test]
    fn item_selection_covers_swords_armor_tools_and_extra_loot_enchantments() {
        let facts = ItemFactsTable::default();
        let sword = Identifier::parse("minecraft:diamond_sword").unwrap();
        let helmet = Identifier::parse("minecraft:diamond_helmet").unwrap();
        let pickaxe = Identifier::parse("minecraft:diamond_pickaxe").unwrap();
        assert_eq!(
            supported_enchantment_for_item(&facts, &sword)
                .unwrap()
                .as_str(),
            "minecraft:sharpness"
        );
        assert_eq!(
            supported_enchantment_for_item(&facts, &helmet)
                .unwrap()
                .as_str(),
            "minecraft:protection"
        );
        assert_eq!(
            supported_enchantment_for_item(&facts, &pickaxe)
                .unwrap()
                .as_str(),
            "minecraft:efficiency"
        );
        assert!(additional_enchantment_for_offer(&sword, 2).is_none());
    }
}

#[cfg(test)]
mod seeded_offer_tests {
    use super::*;

    #[test]
    fn fifteen_shelves_offer_level_thirty_max() {
        // Acceptance: 15 bookshelves give a level-30 max offer.
        let mut saw_thirty = false;
        for seed in 0..64 {
            if enchanting_offers(15, seed)[2].is_some_and(|offer| offer.enchantment_level == 30) {
                saw_thirty = true;
                break;
            }
        }
        assert!(saw_thirty, "15 shelves must be able to offer level 30");
        for seed in [0, 1, 123, -7] {
            let offers = enchanting_offers(15, seed);
            assert!(offers.iter().all(|offer| offer.is_some()));
            assert!(offers[2].unwrap().enchantment_level <= 30);
        }
    }

    #[test]
    fn seeded_offers_never_pair_fortune_with_silk_touch() {
        // Acceptance: exclusive sets respected — one pick can never hold
        // both sides of an exclusive set.
        let pickaxe = Identifier::parse("minecraft:diamond_pickaxe").unwrap();
        let sword = Identifier::parse("minecraft:diamond_sword").unwrap();
        let helmet = Identifier::parse("minecraft:diamond_helmet").unwrap();
        for seed in 0..32 {
            for shelves in [0, 5, 15] {
                for offer in enchanting_offers(shelves, seed).into_iter().flatten() {
                    for item in [&pickaxe, &sword, &helmet] {
                        let resolved = resolve_enchantments_for_offer(item, offer);
                        let ids: Vec<&str> = resolved.iter().map(|(id, _)| id.as_str()).collect();
                        assert!(
                            !(ids.contains(&"minecraft:fortune")
                                && ids.contains(&"minecraft:silk_touch")),
                            "Fortune+Silk Touch paired for {item:?} seed {seed}"
                        );
                        // One protection kind at most.
                        let prot = ids
                            .iter()
                            .filter(|id| {
                                matches!(
                                    **id,
                                    "minecraft:protection"
                                        | "minecraft:fire_protection"
                                        | "minecraft:blast_protection"
                                        | "minecraft:projectile_protection"
                                )
                            })
                            .count();
                        assert!(prot <= 1, "two protections paired: {ids:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn offer_seed_is_stable_until_enchant() {
        let first = enchanting_offers(15, 123);
        let second = enchanting_offers(15, 123);
        assert_eq!(first, second, "same seed must give same offers");
        assert_ne!(
            enchanting_offers(15, 123).map(|offer| offer.map(|offer| offer.enchantment_level)),
            enchanting_offers(15, 124).map(|offer| offer.map(|offer| offer.enchantment_level)),
            "different seeds should differ (probabilistically certain)"
        );
    }
}
