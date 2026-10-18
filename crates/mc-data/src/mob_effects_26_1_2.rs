//! Mob-effect registry identities from the local vanilla 26.1.2 registry report.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u32)]
pub enum MobEffect {
    Speed = 0,
    Slowness = 1,
    Haste = 2,
    MiningFatigue = 3,
    Strength = 4,
    InstantHealth = 5,
    InstantDamage = 6,
    JumpBoost = 7,
    Nausea = 8,
    Regeneration = 9,
    Resistance = 10,
    FireResistance = 11,
    WaterBreathing = 12,
    Invisibility = 13,
    Blindness = 14,
    NightVision = 15,
    Hunger = 16,
    Weakness = 17,
    Poison = 18,
    Wither = 19,
    HealthBoost = 20,
    Absorption = 21,
    Saturation = 22,
    Glowing = 23,
    Levitation = 24,
    Luck = 25,
    Unluck = 26,
    SlowFalling = 27,
    ConduitPower = 28,
    DolphinsGrace = 29,
    BadOmen = 30,
    HeroOfTheVillage = 31,
    Darkness = 32,
    TrialOmen = 33,
    RaidOmen = 34,
    WindCharged = 35,
    Weaving = 36,
    Oozing = 37,
    Infested = 38,
    BreathOfTheNautilus = 39,
}

impl MobEffect {
    pub const ALL: [Self; 40] = [
        Self::Speed,
        Self::Slowness,
        Self::Haste,
        Self::MiningFatigue,
        Self::Strength,
        Self::InstantHealth,
        Self::InstantDamage,
        Self::JumpBoost,
        Self::Nausea,
        Self::Regeneration,
        Self::Resistance,
        Self::FireResistance,
        Self::WaterBreathing,
        Self::Invisibility,
        Self::Blindness,
        Self::NightVision,
        Self::Hunger,
        Self::Weakness,
        Self::Poison,
        Self::Wither,
        Self::HealthBoost,
        Self::Absorption,
        Self::Saturation,
        Self::Glowing,
        Self::Levitation,
        Self::Luck,
        Self::Unluck,
        Self::SlowFalling,
        Self::ConduitPower,
        Self::DolphinsGrace,
        Self::BadOmen,
        Self::HeroOfTheVillage,
        Self::Darkness,
        Self::TrialOmen,
        Self::RaidOmen,
        Self::WindCharged,
        Self::Weaving,
        Self::Oozing,
        Self::Infested,
        Self::BreathOfTheNautilus,
    ];
    pub fn color(self) -> i32 {
        const COLORS: [i32; 40] = [
            3402751, 9154528, 14270531, 4866583, 16762624, 16262179, 11101546, 16646020, 5578058,
            13458603, 9520880, 16750848, 10017472, 16185078, 2039587, 12779366, 5797459, 4738376,
            8889187, 7561558, 16284963, 2445989, 16262179, 9740385, 13565951, 5882118, 12624973,
            15978425, 1950417, 8954814, 745784, 4521796, 2696993, 1484454, 14565464, 12438015,
            7891290, 10092451, 9214860, 65518,
        ];
        COLORS[self as usize]
    }
    pub fn instant(self) -> bool {
        matches!(
            self,
            Self::InstantHealth | Self::InstantDamage | Self::Saturation
        )
    }
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.strip_prefix("minecraft:").unwrap_or(name);
        Self::ALL.into_iter().find(|id| id.name() == name)
    }
    pub fn name(self) -> &'static str {
        name_for_id(self as u32)
            .expect("registered effect")
            .trim_start_matches("minecraft:")
    }
}

#[must_use]
pub fn name_for_id(id: u32) -> Option<&'static str> {
    const NAMES: [&str; 40] = [
        "minecraft:speed",
        "minecraft:slowness",
        "minecraft:haste",
        "minecraft:mining_fatigue",
        "minecraft:strength",
        "minecraft:instant_health",
        "minecraft:instant_damage",
        "minecraft:jump_boost",
        "minecraft:nausea",
        "minecraft:regeneration",
        "minecraft:resistance",
        "minecraft:fire_resistance",
        "minecraft:water_breathing",
        "minecraft:invisibility",
        "minecraft:blindness",
        "minecraft:night_vision",
        "minecraft:hunger",
        "minecraft:weakness",
        "minecraft:poison",
        "minecraft:wither",
        "minecraft:health_boost",
        "minecraft:absorption",
        "minecraft:saturation",
        "minecraft:glowing",
        "minecraft:levitation",
        "minecraft:luck",
        "minecraft:unluck",
        "minecraft:slow_falling",
        "minecraft:conduit_power",
        "minecraft:dolphins_grace",
        "minecraft:bad_omen",
        "minecraft:hero_of_the_village",
        "minecraft:darkness",
        "minecraft:trial_omen",
        "minecraft:raid_omen",
        "minecraft:wind_charged",
        "minecraft:weaving",
        "minecraft:oozing",
        "minecraft:infested",
        "minecraft:breath_of_the_nautilus",
    ];
    NAMES.get(id as usize).copied()
}
