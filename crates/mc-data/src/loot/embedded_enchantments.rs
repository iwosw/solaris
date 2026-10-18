//! Canonical mining-enchantment rules, using the same compiler as full sidecars.
use super::{LootError, LootTables, block_loot_from_table};
use crate::Identifier;
use std::path::Path;

macro_rules! runtime_json {
    ($($name:literal),* $(,)?) => {
        [$(($name, concat!("data/minecraft/loot_table/blocks/", $name, ".json"))),*]
    };
}

pub(super) fn apply(tables: &mut LootTables, root: &Path) -> Result<(), LootError> {
    for (name, relative) in runtime_json!(
        "coal_ore",
        "copper_ore",
        "diamond_ore",
        "emerald_ore",
        "gold_ore",
        "iron_ore",
        "lapis_ore",
        "redstone_ore",
        "deepslate_coal_ore",
        "deepslate_copper_ore",
        "deepslate_diamond_ore",
        "deepslate_emerald_ore",
        "deepslate_gold_ore",
        "deepslate_iron_ore",
        "deepslate_lapis_ore",
        "deepslate_redstone_ore",
        "nether_gold_ore",
        "nether_quartz_ore",
        "stone",
        "deepslate",
        "glass",
        "glass_pane",
        "grass_block",
        "podzol",
        "mycelium",
        "ice",
        "packed_ice",
        "blue_ice"
    ) {
        let source =
            crate::sidecar::read_string(&root.join(relative)).map_err(|source| LootError::Io {
                path: root.join(relative),
                source,
            })?;
        let raw: &str = &source;
        let value = serde_json::from_str(raw).map_err(|source| LootError::Malformed {
            path: root.join(relative),
            source,
        })?;
        let Some(rule) = block_loot_from_table(&root.join(relative), &value)? else {
            return Err(LootError::Io {
                path: root.join(relative),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsupported vanilla block loot {name}"),
                ),
            });
        };
        // Keep flat lookup metadata for existing shared-library consumers;
        // contextual evaluation always selects this canonical compiled rule.
        let id = Identifier::parse(format!("minecraft:{name}")).expect("static block id");
        tables.block_loot.insert(id, rule);
    }
    Ok(())
}
