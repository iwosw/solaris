//! Name-by-id tables of the 26.1.2 `block`, `entity_type` and `item`
//! registries (generated from that release's `registries.json` report).
//! They exist only to migrate durable records written before the 26.3 bump
//! that stored protocol ids, so the old ids resolve by name instead of
//! silently naming whatever the shifted 26.3 registry holds at that index.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::Identifier;
use crate::statistics::StatType;

const LEGACY_IDS: &str = include_str!("../data/legacy_ids_26_1_2.json");

#[derive(serde::Deserialize)]
struct LegacyIds {
    block: Vec<Identifier>,
    entity_type: Vec<Identifier>,
    item: Vec<Identifier>,
}

static LEGACY: LazyLock<LegacyIds> = LazyLock::new(|| {
    serde_json::from_str(LEGACY_IDS).expect("embedded 26.1.2 id tables are valid")
});

/// Current block registry ids by name, in block registration order.
static BLOCK_IDS: LazyLock<HashMap<Identifier, u32>> = LazyLock::new(|| {
    let mut blocks = crate::blocks::solaris_required_blocks_report();
    blocks.sort_by_key(|block| {
        block
            .states
            .iter()
            .map(|state| state.id)
            .min()
            .expect("every block has a state")
    });
    blocks
        .into_iter()
        .enumerate()
        .map(|(index, block)| (block.id, index as u32))
        .collect()
});

static ITEMS: LazyLock<crate::items::ItemRegistry> =
    LazyLock::new(crate::items::solaris_required_items);

static ENTITY_TYPES: LazyLock<crate::entity_types::EntityTypeRegistry> =
    LazyLock::new(crate::entity_types::solaris_required_entity_types);

#[must_use]
pub fn block_name(id: u32) -> Option<&'static Identifier> {
    LEGACY.block.get(id as usize)
}

#[must_use]
pub fn entity_type_name(id: u32) -> Option<&'static Identifier> {
    LEGACY.entity_type.get(id as usize)
}

#[must_use]
pub fn item_name(id: u32) -> Option<&'static Identifier> {
    LEGACY.item.get(id as usize)
}

/// The current registry id of a 26.1.2 statistic subject id: blocks for
/// `mined`, entity types for `killed`/`killed_by`, items otherwise. `None`
/// for `custom` (a fixed array, not registry ids) and for unknown ids.
#[must_use]
pub fn current_statistic_id(kind: StatType, legacy_id: u32) -> Option<u32> {
    match kind {
        StatType::Custom => None,
        StatType::Mined => BLOCK_IDS.get(block_name(legacy_id)?).copied(),
        StatType::Killed | StatType::KilledBy => ENTITY_TYPES.id_of(entity_type_name(legacy_id)?),
        _ => ITEMS.id_of(item_name(legacy_id)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_ids_resolve_by_name_to_current_ids() {
        let stone = Identifier::parse("minecraft:stone").unwrap();
        assert_eq!(block_name(1), Some(&stone));
        assert_eq!(
            current_statistic_id(StatType::Mined, 1),
            BLOCK_IDS.get(&stone).copied()
        );

        let zombie = Identifier::parse("minecraft:zombie").unwrap();
        let legacy_zombie = LEGACY
            .entity_type
            .iter()
            .position(|id| *id == zombie)
            .unwrap() as u32;
        assert_eq!(
            current_statistic_id(StatType::Killed, legacy_zombie),
            ENTITY_TYPES.id_of(&zombie)
        );
        assert_ne!(ENTITY_TYPES.id_of(&zombie), Some(legacy_zombie));

        let diamond = Identifier::parse("minecraft:diamond").unwrap();
        let legacy_diamond = LEGACY.item.iter().position(|id| *id == diamond).unwrap() as u32;
        assert_eq!(
            current_statistic_id(StatType::Used, legacy_diamond),
            ITEMS.id_of(&diamond)
        );
        assert_eq!(current_statistic_id(StatType::Custom, 0), None);
        assert_eq!(current_statistic_id(StatType::Used, u32::MAX), None);
    }
}
