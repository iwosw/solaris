//! Block-entity custody rules compiled from the startup-selected vanilla sources.
use super::{LootError, LootTables, block_loot_from_table};
use crate::Identifier;
use std::path::Path;

macro_rules! runtime_json {
    ($($name:literal),* $(,)?) => {
        [$(($name, concat!("data/minecraft/loot_table/blocks/", $name, ".json"))),*]
    };
}
const SOURCES: &[(&str, &str)] = &runtime_json!(
    "barrel",
    "beacon",
    "bee_nest",
    "beehive",
    "black_banner",
    "black_shulker_box",
    "blast_furnace",
    "blue_banner",
    "blue_shulker_box",
    "brewing_stand",
    "brown_banner",
    "brown_shulker_box",
    "chest",
    "copper_chest",
    "copper_golem_statue",
    "creeper_head",
    "cyan_banner",
    "cyan_shulker_box",
    "decorated_pot",
    "dispenser",
    "dragon_head",
    "dropper",
    "enchanting_table",
    "exposed_copper_chest",
    "exposed_copper_golem_statue",
    "furnace",
    "gray_banner",
    "gray_shulker_box",
    "green_banner",
    "green_shulker_box",
    "hopper",
    "light_blue_banner",
    "light_blue_shulker_box",
    "light_gray_banner",
    "light_gray_shulker_box",
    "lime_banner",
    "lime_shulker_box",
    "magenta_banner",
    "magenta_shulker_box",
    "orange_banner",
    "orange_shulker_box",
    "oxidized_copper_chest",
    "oxidized_copper_golem_statue",
    "piglin_head",
    "pink_banner",
    "pink_shulker_box",
    "player_head",
    "purple_banner",
    "purple_shulker_box",
    "red_banner",
    "red_shulker_box",
    "shulker_box",
    "skeleton_skull",
    "smoker",
    "trapped_chest",
    "waxed_copper_chest",
    "waxed_copper_golem_statue",
    "waxed_exposed_copper_chest",
    "waxed_exposed_copper_golem_statue",
    "waxed_oxidized_copper_chest",
    "waxed_oxidized_copper_golem_statue",
    "waxed_weathered_copper_chest",
    "waxed_weathered_copper_golem_statue",
    "weathered_copper_chest",
    "weathered_copper_golem_statue",
    "white_banner",
    "white_shulker_box",
    "wither_skeleton_skull",
    "yellow_banner",
    "yellow_shulker_box",
    "zombie_head",
);

pub(super) fn apply(tables: &mut LootTables, root: &Path) -> Result<(), LootError> {
    for &(name, relative) in SOURCES {
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
        let id = Identifier::parse(format!("minecraft:{name}")).expect("static block id");
        tables.block_loot.insert(id, rule);
    }
    Ok(())
}
#[cfg(test)]
#[path = "embedded_block_components_tests.rs"]
mod tests;
