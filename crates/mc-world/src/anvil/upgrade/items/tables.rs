// Constant tables transcribed from vanilla javap registrations and fix initializers.
pub(super) const HORNS: &[&str] = &[
    "minecraft:ponder_goat_horn",
    "minecraft:sing_goat_horn",
    "minecraft:seek_goat_horn",
    "minecraft:feel_goat_horn",
    "minecraft:admire_goat_horn",
    "minecraft:call_goat_horn",
    "minecraft:yearn_goat_horn",
    "minecraft:dream_goat_horn",
];
pub(super) const SHERDS: &[&str] = &[
    "angler",
    "archer",
    "arms_up",
    "blade",
    "brewer",
    "burn",
    "danger",
    "explorer",
    "friend",
    "heart",
    "heartbreak",
    "howl",
    "miner",
    "mourner",
    "plenty",
    "prize",
    "sheaf",
    "shelter",
    "skull",
    "snort",
];
pub(super) const BOOLEAN_PROPERTIES: &[&str] = &[
    "attached",
    "bottom",
    "conditional",
    "disarmed",
    "drag",
    "enabled",
    "extended",
    "eye",
    "falling",
    "hanging",
    "has_bottle_0",
    "has_bottle_1",
    "has_bottle_2",
    "has_record",
    "has_book",
    "inverted",
    "in_wall",
    "lit",
    "locked",
    "occupied",
    "open",
    "persistent",
    "powered",
    "short",
    "signal_fire",
    "snowy",
    "triggered",
    "unstable",
    "waterlogged",
    "berries",
    "bloom",
    "shrieking",
    "can_summon",
    "up",
    "down",
    "north",
    "east",
    "south",
    "west",
    "slot_0_occupied",
    "slot_1_occupied",
    "slot_2_occupied",
    "slot_3_occupied",
    "slot_4_occupied",
    "slot_5_occupied",
    "cracked",
    "crafting",
];
pub(super) const MAP_DECORATIONS: &[&str] = &[
    "player",
    "frame",
    "red_marker",
    "blue_marker",
    "target_x",
    "target_point",
    "player_off_map",
    "player_off_limits",
    "mansion",
    "monument",
    "banner_white",
    "banner_orange",
    "banner_magenta",
    "banner_light_blue",
    "banner_yellow",
    "banner_lime",
    "banner_pink",
    "banner_gray",
    "banner_light_gray",
    "banner_cyan",
    "banner_purple",
    "banner_blue",
    "banner_brown",
    "banner_green",
    "banner_red",
    "banner_black",
    "red_x",
    "village_desert",
    "village_plains",
    "village_savanna",
    "village_snowy",
    "village_taiga",
    "jungle_temple",
    "swamp_hut",
];
pub(super) const SPAWN_EGGS: &[(&str, &str)] = &[
    ("minecraft:bat", "minecraft:bat_spawn_egg"),
    ("minecraft:blaze", "minecraft:blaze_spawn_egg"),
    ("minecraft:cave_spider", "minecraft:cave_spider_spawn_egg"),
    ("minecraft:chicken", "minecraft:chicken_spawn_egg"),
    ("minecraft:cow", "minecraft:cow_spawn_egg"),
    ("minecraft:creeper", "minecraft:creeper_spawn_egg"),
    ("minecraft:donkey", "minecraft:donkey_spawn_egg"),
    (
        "minecraft:elder_guardian",
        "minecraft:elder_guardian_spawn_egg",
    ),
    ("minecraft:ender_dragon", "minecraft:ender_dragon_spawn_egg"),
    ("minecraft:enderman", "minecraft:enderman_spawn_egg"),
    ("minecraft:endermite", "minecraft:endermite_spawn_egg"),
    (
        "minecraft:evocation_illager",
        "minecraft:evocation_illager_spawn_egg",
    ),
    ("minecraft:ghast", "minecraft:ghast_spawn_egg"),
    ("minecraft:guardian", "minecraft:guardian_spawn_egg"),
    ("minecraft:horse", "minecraft:horse_spawn_egg"),
    ("minecraft:husk", "minecraft:husk_spawn_egg"),
    ("minecraft:iron_golem", "minecraft:iron_golem_spawn_egg"),
    ("minecraft:llama", "minecraft:llama_spawn_egg"),
    ("minecraft:magma_cube", "minecraft:magma_cube_spawn_egg"),
    ("minecraft:mooshroom", "minecraft:mooshroom_spawn_egg"),
    ("minecraft:mule", "minecraft:mule_spawn_egg"),
    ("minecraft:ocelot", "minecraft:ocelot_spawn_egg"),
    ("minecraft:pufferfish", "minecraft:pufferfish_spawn_egg"),
    ("minecraft:parrot", "minecraft:parrot_spawn_egg"),
    ("minecraft:pig", "minecraft:pig_spawn_egg"),
    ("minecraft:polar_bear", "minecraft:polar_bear_spawn_egg"),
    ("minecraft:rabbit", "minecraft:rabbit_spawn_egg"),
    ("minecraft:sheep", "minecraft:sheep_spawn_egg"),
    ("minecraft:shulker", "minecraft:shulker_spawn_egg"),
    ("minecraft:silverfish", "minecraft:silverfish_spawn_egg"),
    ("minecraft:skeleton", "minecraft:skeleton_spawn_egg"),
    (
        "minecraft:skeleton_horse",
        "minecraft:skeleton_horse_spawn_egg",
    ),
    ("minecraft:slime", "minecraft:slime_spawn_egg"),
    ("minecraft:snow_golem", "minecraft:snow_golem_spawn_egg"),
    ("minecraft:spider", "minecraft:spider_spawn_egg"),
    ("minecraft:squid", "minecraft:squid_spawn_egg"),
    ("minecraft:stray", "minecraft:stray_spawn_egg"),
    ("minecraft:turtle", "minecraft:turtle_spawn_egg"),
    ("minecraft:vex", "minecraft:vex_spawn_egg"),
    ("minecraft:villager", "minecraft:villager_spawn_egg"),
    (
        "minecraft:vindication_illager",
        "minecraft:vindication_illager_spawn_egg",
    ),
    ("minecraft:witch", "minecraft:witch_spawn_egg"),
    ("minecraft:wither", "minecraft:wither_spawn_egg"),
    (
        "minecraft:wither_skeleton",
        "minecraft:wither_skeleton_spawn_egg",
    ),
    ("minecraft:wolf", "minecraft:wolf_spawn_egg"),
    ("minecraft:zombie", "minecraft:zombie_spawn_egg"),
    ("minecraft:zombie_horse", "minecraft:zombie_horse_spawn_egg"),
    (
        "minecraft:zombie_pigman",
        "minecraft:zombie_pigman_spawn_egg",
    ),
    (
        "minecraft:zombie_villager",
        "minecraft:zombie_villager_spawn_egg",
    ),
];
pub(super) const MODIFIER_IDS: &[(&str, &str)] = &[
    (
        "736565d2-e1a7-403d-a3f8-1aeb3e302542",
        "minecraft:creative_mode_block_range",
    ),
    (
        "98491ef6-97b1-4584-ae82-71a8cc85cf73",
        "minecraft:creative_mode_entity_range",
    ),
    (
        "91aeaa56-376b-4498-935b-2f7f68070635",
        "minecraft:effect.speed",
    ),
    (
        "7107de5e-7ce8-4030-940e-514c1f160890",
        "minecraft:effect.slowness",
    ),
    (
        "af8b6e3f-3328-4c0a-aa36-5ba2bb9dbef3",
        "minecraft:effect.haste",
    ),
    (
        "55fced67-e92a-486e-9800-b47f202c4386",
        "minecraft:effect.mining_fatigue",
    ),
    (
        "648d7064-6a60-4f59-8abe-c2c23a6dd7a9",
        "minecraft:effect.strength",
    ),
    (
        "c0105bf3-aef8-46b0-9ebc-92943757ccbe",
        "minecraft:effect.jump_boost",
    ),
    (
        "22653b89-116e-49dc-9b6b-9971489b5be5",
        "minecraft:effect.weakness",
    ),
    (
        "5d6f0ba2-1186-46ac-b896-c61c5cee99cc",
        "minecraft:effect.health_boost",
    ),
    (
        "eae29cf0-701e-4ed6-883a-96f798f3dab5",
        "minecraft:effect.absorption",
    ),
    (
        "03c3c89d-7037-4b42-869f-b146bcb64d2e",
        "minecraft:effect.luck",
    ),
    (
        "cc5af142-2bd2-4215-b636-2605aed11727",
        "minecraft:effect.unluck",
    ),
    ("6555be74-63b3-41f1-a245-77833b3c2562", "minecraft:evil"),
    (
        "1eaf83ff-7207-4596-b37a-d7a07b3ec4ce",
        "minecraft:powder_snow",
    ),
    (
        "662a6b8d-da3e-4c1c-8813-96ea6097278d",
        "minecraft:sprinting",
    ),
    (
        "020e0dfb-87ae-4653-9556-831010e291a0",
        "minecraft:attacking",
    ),
    ("766bfa64-11f3-11ea-8d71-362b9e155667", "minecraft:baby"),
    ("7e0292f2-9434-48d5-a29f-9583af7df27f", "minecraft:covered"),
    (
        "9e362924-01de-4ddd-a2b2-d0f7a405a174",
        "minecraft:suffocating",
    ),
    ("5cd17e52-a79a-43d3-a529-90fde04b181e", "minecraft:drinking"),
    ("b9766b59-9566-4402-bc1f-2ee2a276d836", "minecraft:baby"),
    (
        "49455a49-7ec5-45ba-b886-3b90b23a1718",
        "minecraft:attacking",
    ),
    (
        "845db27c-c624-495f-8c9f-6020a9a58b6b",
        "minecraft:armor.boots",
    ),
    (
        "d8499b04-0e66-4726-ab29-64469d734e0d",
        "minecraft:armor.leggings",
    ),
    (
        "9f3d476d-c118-4544-8365-64846904b48e",
        "minecraft:armor.chestplate",
    ),
    (
        "2ad3f246-fee1-4e67-b886-69fd380bb150",
        "minecraft:armor.helmet",
    ),
    (
        "c1c72771-8b8e-ba4a-ace0-81a93c8928b2",
        "minecraft:armor.body",
    ),
    (
        "b572ecd2-ac0c-4071-abde-9594af072a37",
        "minecraft:enchantment.fire_protection",
    ),
    (
        "40a9968f-5c66-4e2f-b7f4-2ec2f4b3e450",
        "minecraft:enchantment.blast_protection",
    ),
    (
        "07a65791-f64d-4e79-86c7-f83932f007ec",
        "minecraft:enchantment.respiration",
    ),
    (
        "60b1b7db-fffd-4ad0-817c-d6c6a93d8a45",
        "minecraft:enchantment.aqua_affinity",
    ),
    (
        "11dc269a-4476-46c0-aff3-9e17d7eb6801",
        "minecraft:enchantment.depth_strider",
    ),
    (
        "87f46a96-686f-4796-b035-22e16ee9e038",
        "minecraft:enchantment.soul_speed",
    ),
    (
        "b9716dbd-50df-4080-850e-70347d24e687",
        "minecraft:enchantment.soul_speed",
    ),
    (
        "92437d00-c3a7-4f2e-8f6c-1f21585d5dd0",
        "minecraft:enchantment.swift_sneak",
    ),
    (
        "5d3d087b-debe-4037-b53e-d84f3ff51f17",
        "minecraft:enchantment.sweeping_edge",
    ),
    (
        "3ceb37c0-db62-46b5-bd02-785457b01d96",
        "minecraft:enchantment.efficiency",
    ),
    (
        "cb3f55d3-645c-4f38-a497-9c13a33db5cf",
        "minecraft:base_attack_damage",
    ),
    (
        "fa233e1c-4180-4865-b01b-bcce9785aca3",
        "minecraft:base_attack_speed",
    ),
];

// Resolve UUID text at compile time, not once per migrated modifier.
pub(super) const MODIFIER_UUIDS: [([i32; 4], &str); MODIFIER_IDS.len()] = {
    let mut result = [([0; 4], ""); MODIFIER_IDS.len()];
    let mut index = 0;
    while index < MODIFIER_IDS.len() {
        let bytes = MODIFIER_IDS[index].0.as_bytes();
        let mut words = [0_u32; 4];
        let mut offset = 0;
        let mut digit = 0;
        while offset < bytes.len() {
            let byte = bytes[offset];
            if byte != b'-' {
                let nibble = if byte <= b'9' {
                    byte - b'0'
                } else {
                    byte - b'a' + 10
                };
                words[digit / 8] = (words[digit / 8] << 4) | nibble as u32;
                digit += 1;
            }
            offset += 1;
        }
        result[index] = (
            [
                words[0] as i32,
                words[1] as i32,
                words[2] as i32,
                words[3] as i32,
            ],
            MODIFIER_IDS[index].1,
        );
        index += 1;
    }
    result
};
pub(super) const MODIFIER_NAMES: &[(&str, &str)] = &[
    ("Random spawn bonus", "minecraft:random_spawn_bonus"),
    (
        "Random zombie-spawn bonus",
        "minecraft:zombie_random_spawn_bonus",
    ),
    ("Leader zombie bonus", "minecraft:leader_zombie_bonus"),
    (
        "Zombie reinforcement callee charge",
        "minecraft:reinforcement_callee_charge",
    ),
    (
        "Zombie reinforcement caller charge",
        "minecraft:reinforcement_caller_charge",
    ),
];
pub(super) const DYES: &[&str] = &[
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];
pub(super) const MAP_NAMES: &[&str] = &[
    "filled_map.buried_treasure",
    "filled_map.explorer_jungle",
    "filled_map.explorer_swamp",
    "filled_map.mansion",
    "filled_map.monument",
    "filled_map.trial_chambers",
    "filled_map.village_desert",
    "filled_map.village_plains",
    "filled_map.village_savanna",
    "filled_map.village_snowy",
    "filled_map.village_taiga",
];
pub(super) const ADDITIONAL_TOOLTIP: &[&str] = &[
    "minecraft:banner_patterns",
    "minecraft:bees",
    "minecraft:block_entity_data",
    "minecraft:block_state",
    "minecraft:bundle_contents",
    "minecraft:charged_projectiles",
    "minecraft:container",
    "minecraft:container_loot",
    "minecraft:firework_explosion",
    "minecraft:fireworks",
    "minecraft:instrument",
    "minecraft:map_id",
    "minecraft:painting/variant",
    "minecraft:pot_decorations",
    "minecraft:potion_contents",
    "minecraft:tropical_fish/pattern",
    "minecraft:written_book_content",
];
