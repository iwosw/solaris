//! Neutral vanilla player-record NBT codec: the shared keys every
//! crate reads and writes (`Pos`, `Rotation`, `Health`, `foodLevel`,
//! `foodSaturationLevel`, `foodExhaustionLevel`, `Air`, `SelectedItemSlot`,
//! `XpLevel`/`XpP`/`XpTotal`, `playerGameType`, `Inventory` list with `Slot`,
//! `equipment` compound, item compounds via [`crate::item_nbt`]).
//! Crate-specific extras (`active_effects`, `solaris:*`, spawn records...)
//! stay in their crates; only the shared subset lives here, once.
//!
//! Vanilla slot model (26.1.2 `ddl.java`, `cdd.java:6`, `bfm.java:134`):
//! `Inventory` holds the 36-slot player inventory only (`Slot` byte
//! 0-35; hotbar 0-8, main 9-35). Armor, offhand, body and saddle ride in
//! the `equipment` compound keyed by slot name (`cgo.java`, `cgv.java`:
//! `mainhand`, `offhand`, `feet`, `legs`, `chest`, `head`, `body`,
//! `saddle`), each an item compound without a `Slot` key. Solaris v2
//! inventory indices mirror this: 0-35 storage, 36-39 armor
//! (feet/legs/chest/head), 40 offhand.

use mc_nbt::{ListTag, Tag, tag_type};

use crate::item_nbt::{ItemNbtError, field, stack_from_fields, stack_to_fields};
use crate::{ItemStack, items::ItemRegistry};

/// Neutral vanilla player record: position, vitals, xp, mode, inventory,
/// equipment. `inventory` holds storage slots 0-35 (`Slot` byte); armor
/// (feet/legs/chest/head), offhand, body and saddle ride `equipment`
/// keyed by slot name, exactly like vanilla.
#[derive(Debug, Clone, PartialEq)]
pub struct VanillaPlayer {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub exhaustion: f32,
    pub air: i32,
    pub selected: u8,
    pub xp_level: i32,
    pub xp_progress: f32,
    pub xp_total: i32,
    pub game_mode: i32,
    /// `(storage slot 0-35, stack)` pairs, sorted by slot on encode.
    pub inventory: Vec<(u8, ItemStack)>,
    /// Equipment by vanilla slot name (`feet`, `legs`, `chest`, `head`,
    /// `offhand`, `body`, `saddle`, ...). Empty stacks never encode.
    pub equipment: Vec<(String, ItemStack)>,
    /// Cursor custody (Solaris-only; encoded as `solaris:Carried` by callers
    /// that persist it, ignored by the vanilla codec itself).
    pub carried: ItemStack,
    /// Crafting-grid custody (ditto, `solaris:CraftingGrid`).
    pub grid: Vec<ItemStack>,
}

impl Default for VanillaPlayer {
    fn default() -> Self {
        Self {
            pos: [0.0, 80.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            health: 20.0,
            food: 20,
            saturation: 5.0,
            exhaustion: 0.0,
            air: 300,
            selected: 0,
            xp_level: 0,
            xp_progress: 0.0,
            xp_total: 0,
            game_mode: 0,
            inventory: Vec::new(),
            equipment: Vec::new(),
            carried: ItemStack::EMPTY,
            grid: Vec::new(),
        }
    }
}

fn int_field(fields: &[(String, Tag)], name: &str) -> Option<i32> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(i32::from(*value)),
        Tag::Short(value) => Some(i32::from(*value)),
        Tag::Int(value) => Some(*value),
        _ => None,
    }
}

fn float_field(fields: &[(String, Tag)], name: &str) -> Option<f32> {
    match field(fields, name)? {
        Tag::Float(value) => Some(*value),
        _ => None,
    }
}

fn double_list<const N: usize>(tag: &Tag) -> Option<[f64; N]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (index, element) in list.elements.iter().enumerate() {
        let Tag::Double(value) = element else {
            return None;
        };
        out[index] = *value;
    }
    Some(out)
}

fn float_list<const N: usize>(tag: &Tag) -> Option<[f32; N]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (index, element) in list.elements.iter().enumerate() {
        let Tag::Float(value) = element else {
            return None;
        };
        out[index] = *value;
    }
    Some(out)
}

/// Decode the shared vanilla keys. Unknown/extra keys are ignored (the
/// caller's extras live elsewhere); missing keys keep `base` values.
pub fn player_from_fields(
    items: &ItemRegistry,
    base: VanillaPlayer,
    fields: &[(String, Tag)],
) -> Result<VanillaPlayer, ItemNbtError> {
    let mut out = base;
    if let Some(pos) = field(fields, "Pos").and_then(double_list::<3>) {
        out.pos = pos;
    }
    if let Some(rot) = field(fields, "Rotation").and_then(float_list::<2>) {
        out.yaw = rot[0];
        out.pitch = rot[1];
    }
    if let Some(health) = float_field(fields, "Health") {
        out.health = health.max(0.0);
    }
    if let Some(food) = int_field(fields, "foodLevel") {
        out.food = food.max(0);
    }
    if let Some(saturation) = float_field(fields, "foodSaturationLevel") {
        out.saturation = saturation.max(0.0);
    }
    if let Some(exhaustion) = float_field(fields, "foodExhaustionLevel") {
        out.exhaustion = exhaustion.max(0.0);
    }
    if let Some(air) = int_field(fields, "Air") {
        out.air = air;
    }
    if let Some(selected) = int_field(fields, "SelectedItemSlot") {
        out.selected = selected.clamp(0, 8) as u8;
    }
    if let Some(level) = int_field(fields, "XpLevel") {
        out.xp_level = level.max(0);
    }
    if let Some(progress) = float_field(fields, "XpP") {
        out.xp_progress = progress.clamp(0.0, 1.0);
    }
    if let Some(total) = int_field(fields, "XpTotal") {
        out.xp_total = total.max(0);
    }
    if let Some(mode) = int_field(fields, "playerGameType") {
        out.game_mode = mode;
    }
    if let Some(Tag::List(list)) = field(fields, "Inventory") {
        let mut inventory = Vec::new();
        for element in &list.elements {
            let Tag::Compound(item_fields) = element else {
                continue;
            };
            // `Slot` is an unsigned byte on the wire (`bfm.java:134`); only
            // 0-35 belong to the player inventory (armor/offhand ride in
            // `equipment`). Anything else is ignored, like vanilla.
            let slot = match field(item_fields, "Slot") {
                Some(Tag::Byte(slot)) => u8::try_from(*slot).ok(),
                _ => None,
            };
            let Some(slot) = slot.filter(|slot| *slot < 36) else {
                continue;
            };
            if let Some(stack) = stack_from_fields(items, item_fields)? {
                inventory.push((slot, stack));
            }
        }
        inventory.sort_by_key(|(slot, _)| *slot);
        out.inventory = inventory;
    }
    if let Some(Tag::Compound(slots)) = field(fields, "equipment") {
        let mut equipment = Vec::new();
        for (name, tag) in slots {
            let Tag::Compound(item_fields) = tag else {
                continue;
            };
            if let Some(stack) = stack_from_fields(items, item_fields)? {
                equipment.push((name.clone(), stack));
            }
        }
        equipment.sort_by(|left, right| left.0.cmp(&right.0));
        out.equipment = equipment;
    }
    Ok(out)
}

/// Encode the shared vanilla keys (`Slot` as byte, like vanilla playerdata).
pub fn player_to_fields(
    items: &ItemRegistry,
    player: &VanillaPlayer,
) -> Result<Vec<(String, Tag)>, ItemNbtError> {
    let mut fields = vec![
        (
            "Pos".into(),
            Tag::List(ListTag {
                element_type: tag_type::DOUBLE,
                elements: player.pos.map(Tag::Double).to_vec(),
            }),
        ),
        (
            "Rotation".into(),
            Tag::List(ListTag {
                element_type: tag_type::FLOAT,
                elements: vec![Tag::Float(player.yaw), Tag::Float(player.pitch)],
            }),
        ),
        ("Health".into(), Tag::Float(player.health)),
        ("foodLevel".into(), Tag::Int(player.food)),
        ("foodSaturationLevel".into(), Tag::Float(player.saturation)),
        ("foodExhaustionLevel".into(), Tag::Float(player.exhaustion)),
        ("Air".into(), Tag::Int(player.air)),
        (
            "SelectedItemSlot".into(),
            Tag::Int(i32::from(player.selected)),
        ),
        ("XpLevel".into(), Tag::Int(player.xp_level)),
        ("XpP".into(), Tag::Float(player.xp_progress)),
        ("XpTotal".into(), Tag::Int(player.xp_total)),
        ("playerGameType".into(), Tag::Int(player.game_mode)),
    ];
    let mut elements = Vec::new();
    let mut sorted = player.inventory.clone();
    sorted.sort_by_key(|(slot, _)| *slot);
    for (slot, stack) in &sorted {
        if stack.is_empty() || *slot >= 36 {
            continue;
        }
        let mut item_fields = vec![("Slot".into(), Tag::Byte(*slot as i8))];
        item_fields.append(&mut stack_to_fields(items, stack)?);
        elements.push(Tag::Compound(item_fields));
    }
    fields.push((
        "Inventory".into(),
        Tag::List(ListTag {
            element_type: if elements.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements,
        }),
    ));
    // Armor/offhand/body/saddle keyed by slot name (vanilla `equipment`
    // compound); omitted entirely when empty (vanilla `chl.java:622`).
    let mut gear: Vec<(String, Tag)> = Vec::new();
    let mut sorted_gear = player.equipment.clone();
    sorted_gear.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, stack) in &sorted_gear {
        if stack.is_empty() {
            continue;
        }
        gear.push((name.clone(), Tag::Compound(stack_to_fields(items, stack)?)));
    }
    if !gear.is_empty() {
        fields.push(("equipment".into(), Tag::Compound(gear)));
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> ItemRegistry {
        crate::items::solaris_required_items()
    }

    #[test]
    fn player_round_trip_keeps_vanilla_keys() {
        let items = items();
        let sword_id = items
            .id_of(&crate::Identifier::parse("minecraft:diamond_sword").expect("id"))
            .expect("sword");
        let player = VanillaPlayer {
            pos: [5.0, 80.0, 5.0],
            yaw: 1.0,
            pitch: -0.5,
            health: 14.0,
            food: 17,
            saturation: 3.0,
            exhaustion: 1.0,
            air: 290,
            selected: 2,
            xp_level: 7,
            xp_progress: 0.3,
            xp_total: 100,
            game_mode: 0,
            equipment: Vec::new(),
            carried: ItemStack::EMPTY,
            grid: Vec::new(),
            inventory: vec![(
                2,
                ItemStack {
                    item_id: sword_id,
                    count: 1,
                    damage: Some(37),
                    enchantments: vec![crate::ItemEnchantment {
                        id: crate::Identifier::parse("minecraft:sharpness").expect("e"),
                        level: 3,
                    }],
                    custom_name: Some(" quests blade ".into()),
                    ..ItemStack::EMPTY
                },
            )],
        };
        let fields = player_to_fields(&items, &player).expect("encode");
        // Vanilla keys present; Solaris-private keys absent.
        for key in [
            "Pos",
            "Rotation",
            "Health",
            "foodLevel",
            "foodSaturationLevel",
            "foodExhaustionLevel",
            "Air",
            "SelectedItemSlot",
            "XpLevel",
            "XpP",
            "XpTotal",
            "playerGameType",
            "Inventory",
        ] {
            assert!(fields.iter().any(|(name, _)| name == key), "has {key}");
        }
        assert!(fields.iter().all(|(name, _)| !name.starts_with("solaris:")));
        assert!(
            fields
                .iter()
                .all(|(name, _)| name != "Tick" && name != "Uuid")
        );
        let back = player_from_fields(&items, VanillaPlayer::default(), &fields).expect("decode");
        assert_eq!(back, player);
    }

    /// Storage-only `Inventory` (slots 0-35) + `equipment` compound: armor
    /// and offhand never ride `Inventory`; vanilla indices survive.
    #[test]
    fn armor_and_offhand_ride_equipment_compound() {
        let items = items();
        let boots_id = items
            .id_of(&crate::Identifier::parse("minecraft:diamond_boots").expect("id"))
            .expect("boots");
        let player = VanillaPlayer {
            pos: [0.0, 80.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            health: 20.0,
            food: 20,
            saturation: 5.0,
            exhaustion: 0.0,
            air: 300,
            selected: 0,
            xp_level: 0,
            xp_progress: 0.0,
            xp_total: 0,
            game_mode: 0,
            inventory: vec![
                (0, ItemStack::new(boots_id, 1)),
                (9, ItemStack::new(boots_id, 2)),
            ],
            equipment: vec![
                ("feet".to_owned(), ItemStack::new(boots_id, 1)),
                ("offhand".to_owned(), ItemStack::new(boots_id, 1)),
            ],
            carried: ItemStack::EMPTY,
            grid: Vec::new(),
        };
        let fields = player_to_fields(&items, &player).expect("encode");
        // `Inventory` holds only storage slots 0-35.
        let (_, inventory) = fields
            .iter()
            .find(|(name, _)| name == "Inventory")
            .expect("Inventory");
        let Tag::List(list) = inventory else {
            panic!("list");
        };
        let mut slots: Vec<u8> = list
            .elements
            .iter()
            .map(|element| {
                let Tag::Compound(item_fields) = element else {
                    panic!("compound");
                };
                match field(item_fields, "Slot") {
                    Some(Tag::Byte(slot)) => *slot as u8,
                    other => panic!("slot {other:?}"),
                }
            })
            .collect();
        slots.sort_unstable();
        assert_eq!(slots, vec![0, 9], "armor/offhand absent from Inventory");
        // `equipment` compound keyed by slot name, no `Slot` key inside.
        let (_, gear) = fields
            .iter()
            .find(|(name, _)| name == "equipment")
            .expect("equipment");
        let Tag::Compound(slots) = gear else {
            panic!("compound");
        };
        assert_eq!(slots.len(), 2);
        for (name, tag) in slots {
            assert!(name == "feet" || name == "offhand", "slot name {name}");
            let Tag::Compound(item_fields) = tag else {
                panic!("item");
            };
            assert!(
                field(item_fields, "Slot").is_none(),
                "equipment items carry no Slot key"
            );
        }
        let back = player_from_fields(&items, VanillaPlayer::default(), &fields).expect("decode");
        assert_eq!(back, player);
    }

    /// Vanilla-issued `Inventory` entries outside 0-35 are ignored (vanilla
    /// `ddl.java:349-365` loads only indices < 36 into storage).
    #[test]
    fn out_of_range_wire_slots_are_ignored() {
        let items = items();
        let boots_id = items
            .id_of(&crate::Identifier::parse("minecraft:diamond_boots").expect("id"))
            .expect("boots");
        let stack_fields =
            crate::item_nbt::stack_to_fields(&items, &ItemStack::new(boots_id, 1)).expect("stack");
        let mut bogus = vec![("Slot".into(), Tag::Byte(45))];
        bogus.extend(stack_fields);
        let fields = vec![(
            "Inventory".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![Tag::Compound(bogus)],
            }),
        )];
        let back = player_from_fields(&items, VanillaPlayer::default(), &fields).expect("decode");
        assert!(back.inventory.is_empty(), "slot 45 ignored in Inventory");
    }
}
