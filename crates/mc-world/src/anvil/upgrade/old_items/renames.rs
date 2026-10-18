//! Generated per-version item renames (`ItemRenameFix.create` registrations before 2586).
//!
//! Provenance: `DataFixers.javap` registration ldc strings plus the referenced holder-class
//! `RENAMED_*` maps (`RenamedCoralFix`, `EntityPufferfishRenameFix`, `EntityCodSalmonFix`,
//! `EntityTheRenameningFix`, `DyeItemRenameFix`, `EntityRavagerRenameFix`,
//! `EntityZombifiedPiglinRenameFix`), dumped from the local 26.1.2 jar. Versions 1474
//! (identity lambda), 1475 and 1515 (block-only) carry no item renames and are absent here.
//! Full inventory: `legacy-item-fixers.md`.

/// Exact pre-2586 item rename for one schema version, or `None`.
pub(super) fn renamed(old: &str, version: i32) -> Option<&'static str> {
    let name = old.strip_prefix("minecraft:").unwrap_or(old);
    if old.contains(':') && !old.starts_with("minecraft:") {
        return None;
    }
    Some(match (version, name) {
        // 502: cooked_fished item renamer.
        (502, "cooked_fished") => "minecraft:cooked_fish",
        // 820: totem item renamer.
        (820, "totem") => "minecraft:totem_of_undying",
        // 1480: Rename coral items.
        (1480, "blue_coral") => "minecraft:tube_coral_block",
        (1480, "pink_coral") => "minecraft:brain_coral_block",
        (1480, "purple_coral") => "minecraft:bubble_coral_block",
        (1480, "red_coral") => "minecraft:fire_coral_block",
        (1480, "yellow_coral") => "minecraft:horn_coral_block",
        (1480, "blue_coral_plant") => "minecraft:tube_coral",
        (1480, "pink_coral_plant") => "minecraft:brain_coral",
        (1480, "purple_coral_plant") => "minecraft:bubble_coral",
        (1480, "red_coral_plant") => "minecraft:fire_coral",
        (1480, "yellow_coral_plant") => "minecraft:horn_coral",
        (1480, "blue_coral_fan") => "minecraft:tube_coral_fan",
        (1480, "pink_coral_fan") => "minecraft:brain_coral_fan",
        (1480, "purple_coral_fan") => "minecraft:bubble_coral_fan",
        (1480, "red_coral_fan") => "minecraft:fire_coral_fan",
        (1480, "yellow_coral_fan") => "minecraft:horn_coral_fan",
        (1480, "blue_dead_coral") => "minecraft:dead_tube_coral",
        (1480, "pink_dead_coral") => "minecraft:dead_brain_coral",
        (1480, "purple_dead_coral") => "minecraft:dead_bubble_coral",
        (1480, "red_dead_coral") => "minecraft:dead_fire_coral",
        (1480, "yellow_dead_coral") => "minecraft:dead_horn_coral",
        // 1483: Rename pufferfish egg item.
        (1483, "puffer_fish_spawn_egg") => "minecraft:pufferfish_spawn_egg",
        // 1484: Rename seagrass items.
        (1484, "sea_grass") => "minecraft:seagrass",
        (1484, "tall_sea_grass") => "minecraft:tall_seagrass",
        // 1486: Rename cod/salmon egg items.
        (1486, "salmon_mob_spawn_egg") => "minecraft:salmon_spawn_egg",
        (1486, "cod_mob_spawn_egg") => "minecraft:cod_spawn_egg",
        // 1487: Rename prismarine_brick(s) items.
        (1487, "prismarine_bricks_slab") => "minecraft:prismarine_brick_slab",
        (1487, "prismarine_bricks_stairs") => "minecraft:prismarine_brick_stairs",
        // 1488: Rename kelptop.
        (1488, "kelp_top") => "minecraft:kelp",
        // 1490: Rename melon_block/melon/speckled_melon.
        (1490, "melon_block") => "minecraft:melon",
        (1490, "melon") => "minecraft:melon_slice",
        (1490, "speckled_melon") => "minecraft:glistering_melon_slice",
        // 1510: Item renamening fix.
        (1510, "clownfish") => "minecraft:tropical_fish",
        (1510, "chorus_fruit_popped") => "minecraft:popped_chorus_fruit",
        (1510, "evocation_illager_spawn_egg") => "minecraft:evoker_spawn_egg",
        (1510, "vindication_illager_spawn_egg") => "minecraft:vindicator_spawn_egg",
        // 1800: Rename dye items.
        (1800, "cactus_green") => "minecraft:green_dye",
        (1800, "rose_red") => "minecraft:red_dye",
        (1800, "dandelion_yellow") => "minecraft:yellow_dye",
        // 1802: Rename sign item & stone slabs.
        (1802, "stone_slab") => "minecraft:smooth_stone_slab",
        (1802, "sign") => "minecraft:oak_sign",
        // 1928: Rename ravager egg item.
        (1928, "illager_beast_spawn_egg") => "minecraft:ravager_spawn_egg",
        // 2209: Rename bee_hive item to beehive.
        (2209, "bee_hive") => "minecraft:beehive",
        // 2508: Renamed fungi items to fungus.
        (2508, "warped_fungi") => "minecraft:warped_fungus",
        (2508, "crimson_fungi") => "minecraft:crimson_fungus",
        // 2509: Rename zombie pigman egg item.
        (2509, "zombie_pigman_spawn_egg") => "minecraft:zombified_piglin_spawn_egg",
        // 2528: Rename soul fire torch and soul fire lantern.
        (2528, "soul_fire_torch") => "minecraft:soul_torch",
        (2528, "soul_fire_lantern") => "minecraft:soul_lantern",
        _ => return None,
    })
}
