//! Canonical Minecraft 26.1.2 block-entity identities, shared by disk and wire.

macro_rules! block_entity_kinds {
    ($( $kind:ident = $protocol_id:literal => $id:literal, [$($block:literal),+], )+) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u32)]
        pub enum BlockEntityKind {
            $( $kind = $protocol_id, )+
        }

        impl BlockEntityKind {
            pub const ALL: &'static [Self] = &[$( Self::$kind, )+];

            #[must_use]
            pub const fn protocol_id(self) -> u32 {
                self as u32
            }

            #[must_use]
            pub const fn vanilla_id(self) -> &'static str {
                match self {
                    $( Self::$kind => concat!("minecraft:", $id), )+
                }
            }

            #[must_use]
            pub fn for_block(path: &str) -> Option<Self> {
                // String matches are a linear chain in debug builds. Build the
                // canonical name index once instead of scanning every owner
                // for each block examined during chunk/setup work.
                static BY_BLOCK: std::sync::LazyLock<
                    std::collections::HashMap<&'static str, BlockEntityKind>,
                > = std::sync::LazyLock::new(|| {
                    [$( $( ($block, BlockEntityKind::$kind), )+ )+]
                        .into_iter()
                        .collect()
                });
                BY_BLOCK.get(path).copied()
            }

            #[must_use]
            pub const fn block_paths(self) -> &'static [&'static str] {
                match self {
                    $( Self::$kind => &[$($block),+], )+
                }
            }
        }
    };
}

// BlockEntityType's registration order and reports/registries.json, 26.1.2.
block_entity_kinds! {
    Furnace = 0 => "furnace", ["furnace"],
    Chest = 1 => "chest", ["chest", "copper_chest", "exposed_copper_chest", "weathered_copper_chest", "oxidized_copper_chest", "waxed_copper_chest", "waxed_exposed_copper_chest", "waxed_weathered_copper_chest", "waxed_oxidized_copper_chest"],
    TrappedChest = 2 => "trapped_chest", ["trapped_chest"],
    EnderChest = 3 => "ender_chest", ["ender_chest"],
    Jukebox = 4 => "jukebox", ["jukebox"],
    Dispenser = 5 => "dispenser", ["dispenser"],
    Dropper = 6 => "dropper", ["dropper"],
    Sign = 7 => "sign", ["oak_sign", "spruce_sign", "birch_sign", "acacia_sign", "cherry_sign", "jungle_sign", "dark_oak_sign", "pale_oak_sign", "oak_wall_sign", "spruce_wall_sign", "birch_wall_sign", "acacia_wall_sign", "cherry_wall_sign", "jungle_wall_sign", "dark_oak_wall_sign", "pale_oak_wall_sign", "crimson_sign", "crimson_wall_sign", "warped_sign", "warped_wall_sign", "mangrove_sign", "mangrove_wall_sign", "bamboo_sign", "bamboo_wall_sign"],
    HangingSign = 8 => "hanging_sign", ["oak_hanging_sign", "spruce_hanging_sign", "birch_hanging_sign", "acacia_hanging_sign", "cherry_hanging_sign", "jungle_hanging_sign", "dark_oak_hanging_sign", "pale_oak_hanging_sign", "crimson_hanging_sign", "warped_hanging_sign", "mangrove_hanging_sign", "bamboo_hanging_sign", "oak_wall_hanging_sign", "spruce_wall_hanging_sign", "birch_wall_hanging_sign", "acacia_wall_hanging_sign", "cherry_wall_hanging_sign", "jungle_wall_hanging_sign", "dark_oak_wall_hanging_sign", "pale_oak_wall_hanging_sign", "crimson_wall_hanging_sign", "warped_wall_hanging_sign", "mangrove_wall_hanging_sign", "bamboo_wall_hanging_sign"],
    MobSpawner = 9 => "mob_spawner", ["spawner"],
    CreakingHeart = 10 => "creaking_heart", ["creaking_heart"],
    Piston = 11 => "piston", ["moving_piston"],
    BrewingStand = 12 => "brewing_stand", ["brewing_stand"],
    EnchantingTable = 13 => "enchanting_table", ["enchanting_table"],
    EndPortal = 14 => "end_portal", ["end_portal"],
    Beacon = 15 => "beacon", ["beacon"],
    Skull = 16 => "skull", ["skeleton_skull", "skeleton_wall_skull", "creeper_head", "creeper_wall_head", "dragon_head", "dragon_wall_head", "zombie_head", "zombie_wall_head", "wither_skeleton_skull", "wither_skeleton_wall_skull", "player_head", "player_wall_head", "piglin_head", "piglin_wall_head"],
    DaylightDetector = 17 => "daylight_detector", ["daylight_detector"],
    Hopper = 18 => "hopper", ["hopper"],
    Comparator = 19 => "comparator", ["comparator"],
    Banner = 20 => "banner", ["white_banner", "orange_banner", "magenta_banner", "light_blue_banner", "yellow_banner", "lime_banner", "pink_banner", "gray_banner", "light_gray_banner", "cyan_banner", "purple_banner", "blue_banner", "brown_banner", "green_banner", "red_banner", "black_banner", "white_wall_banner", "orange_wall_banner", "magenta_wall_banner", "light_blue_wall_banner", "yellow_wall_banner", "lime_wall_banner", "pink_wall_banner", "gray_wall_banner", "light_gray_wall_banner", "cyan_wall_banner", "purple_wall_banner", "blue_wall_banner", "brown_wall_banner", "green_wall_banner", "red_wall_banner", "black_wall_banner"],
    StructureBlock = 21 => "structure_block", ["structure_block"],
    EndGateway = 22 => "end_gateway", ["end_gateway"],
    CommandBlock = 23 => "command_block", ["command_block", "chain_command_block", "repeating_command_block"],
    ShulkerBox = 24 => "shulker_box", ["shulker_box", "black_shulker_box", "blue_shulker_box", "brown_shulker_box", "cyan_shulker_box", "gray_shulker_box", "green_shulker_box", "light_blue_shulker_box", "light_gray_shulker_box", "lime_shulker_box", "magenta_shulker_box", "orange_shulker_box", "pink_shulker_box", "purple_shulker_box", "red_shulker_box", "white_shulker_box", "yellow_shulker_box"],
    Bed = 25 => "bed", ["red_bed", "black_bed", "blue_bed", "brown_bed", "cyan_bed", "gray_bed", "green_bed", "light_blue_bed", "light_gray_bed", "lime_bed", "magenta_bed", "orange_bed", "pink_bed", "purple_bed", "white_bed", "yellow_bed"],
    Conduit = 26 => "conduit", ["conduit"],
    Barrel = 27 => "barrel", ["barrel"],
    Smoker = 28 => "smoker", ["smoker"],
    BlastFurnace = 29 => "blast_furnace", ["blast_furnace"],
    Lectern = 30 => "lectern", ["lectern"],
    Bell = 31 => "bell", ["bell"],
    Jigsaw = 32 => "jigsaw", ["jigsaw"],
    Campfire = 33 => "campfire", ["campfire", "soul_campfire"],
    Beehive = 34 => "beehive", ["bee_nest", "beehive"],
    SculkSensor = 35 => "sculk_sensor", ["sculk_sensor"],
    CalibratedSculkSensor = 36 => "calibrated_sculk_sensor", ["calibrated_sculk_sensor"],
    SculkCatalyst = 37 => "sculk_catalyst", ["sculk_catalyst"],
    SculkShrieker = 38 => "sculk_shrieker", ["sculk_shrieker"],
    ChiseledBookshelf = 39 => "chiseled_bookshelf", ["chiseled_bookshelf"],
    Shelf = 40 => "shelf", ["acacia_shelf", "bamboo_shelf", "birch_shelf", "cherry_shelf", "crimson_shelf", "dark_oak_shelf", "jungle_shelf", "mangrove_shelf", "oak_shelf", "pale_oak_shelf", "spruce_shelf", "warped_shelf"],
    BrushableBlock = 41 => "brushable_block", ["suspicious_sand", "suspicious_gravel"],
    DecoratedPot = 42 => "decorated_pot", ["decorated_pot"],
    Crafter = 43 => "crafter", ["crafter"],
    TrialSpawner = 44 => "trial_spawner", ["trial_spawner"],
    Vault = 45 => "vault", ["vault"],
    TestBlock = 46 => "test_block", ["test_block"],
    TestInstanceBlock = 47 => "test_instance_block", ["test_instance_block"],
    CopperGolemStatue = 48 => "copper_golem_statue", ["copper_golem_statue", "exposed_copper_golem_statue", "weathered_copper_golem_statue", "oxidized_copper_golem_statue", "waxed_copper_golem_statue", "waxed_exposed_copper_golem_statue", "waxed_weathered_copper_golem_statue", "waxed_oxidized_copper_golem_statue"],
}

impl BlockEntityKind {
    /// Local inventories owned by v2 (ender-chest storage belongs to players).
    #[must_use]
    pub const fn inventory_size(self) -> Option<usize> {
        match self {
            Self::Chest | Self::TrappedChest | Self::Barrel | Self::ShulkerBox => Some(27),
            Self::Furnace | Self::Smoker | Self::BlastFurnace | Self::Shelf => Some(3),
            Self::Hopper | Self::BrewingStand => Some(5),
            Self::Dispenser | Self::Dropper | Self::Crafter => Some(9),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "block_entity_kinds_tests.rs"]
mod tests;
