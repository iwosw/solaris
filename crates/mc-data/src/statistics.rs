//! Vanilla 26.3 statistics. Registry order is derived from local `javap`
//! of `Stats` and `BuiltInRegistries`; custom hot counters never allocate.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The vanilla `stat_type` registry, in registration/wire order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u32)]
pub enum StatType {
    Mined,
    Crafted,
    Used,
    Broken,
    PickedUp,
    Dropped,
    Killed,
    KilledBy,
    Custom,
}

impl StatType {
    pub const ALL: [Self; 9] = [
        Self::Mined,
        Self::Crafted,
        Self::Used,
        Self::Broken,
        Self::PickedUp,
        Self::Dropped,
        Self::Killed,
        Self::KilledBy,
        Self::Custom,
    ];

    #[must_use]
    pub const fn id(self) -> u32 {
        self as u32
    }

    #[must_use]
    pub fn from_id(id: u32) -> Option<Self> {
        Self::ALL.get(id as usize).copied()
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mined => "minecraft:mined",
            Self::Crafted => "minecraft:crafted",
            Self::Used => "minecraft:used",
            Self::Broken => "minecraft:broken",
            Self::PickedUp => "minecraft:picked_up",
            Self::Dropped => "minecraft:dropped",
            Self::Killed => "minecraft:killed",
            Self::KilledBy => "minecraft:killed_by",
            Self::Custom => "minecraft:custom",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// The complete vanilla `custom_stat` registry in registration/wire order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u32)]
pub enum CustomStat {
    LeaveGame,
    PlayTime,
    TotalWorldTime,
    TimeSinceDeath,
    TimeSinceRest,
    SneakTime,
    WalkOneCm,
    CrouchOneCm,
    SprintOneCm,
    WalkOnWaterOneCm,
    FallOneCm,
    ClimbOneCm,
    FlyOneCm,
    WalkUnderWaterOneCm,
    MinecartOneCm,
    BoatOneCm,
    PigOneCm,
    HappyGhastOneCm,
    HorseOneCm,
    AviateOneCm,
    SwimOneCm,
    StriderOneCm,
    NautilusOneCm,
    Jump,
    Drop,
    DamageDealt,
    DamageDealtAbsorbed,
    DamageDealtResisted,
    DamageTaken,
    DamageBlockedByShield,
    DamageAbsorbed,
    DamageResisted,
    Deaths,
    MobKills,
    AnimalsBred,
    PlayerKills,
    FishCaught,
    TalkedToVillager,
    TradedWithVillager,
    EatCakeSlice,
    FillCauldron,
    UseCauldron,
    CleanArmor,
    CleanBanner,
    CleanShulkerBox,
    InteractWithBrewingStand,
    InteractWithBeacon,
    InspectDropper,
    InspectHopper,
    InspectDispenser,
    PlayNoteBlock,
    TuneNoteBlock,
    PotFlower,
    TriggerTrappedChest,
    OpenEnderChest,
    EnchantItem,
    PlayRecord,
    InteractWithFurnace,
    InteractWithCraftingTable,
    OpenChest,
    SleepInBed,
    SleepInStrawBed,
    OpenShulkerBox,
    OpenBarrel,
    InteractWithBlastFurnace,
    InteractWithSmoker,
    InteractWithLectern,
    InteractWithCampfire,
    InteractWithCartographyTable,
    InteractWithLoom,
    InteractWithStonecutter,
    BellRing,
    RaidTrigger,
    RaidWin,
    InteractWithAnvil,
    InteractWithGrindstone,
    TargetHit,
    InteractWithSmithingTable,
}

impl CustomStat {
    pub const COUNT: usize = 78;
    pub const ALL: [Self; Self::COUNT] = [
        Self::LeaveGame,
        Self::PlayTime,
        Self::TotalWorldTime,
        Self::TimeSinceDeath,
        Self::TimeSinceRest,
        Self::SneakTime,
        Self::WalkOneCm,
        Self::CrouchOneCm,
        Self::SprintOneCm,
        Self::WalkOnWaterOneCm,
        Self::FallOneCm,
        Self::ClimbOneCm,
        Self::FlyOneCm,
        Self::WalkUnderWaterOneCm,
        Self::MinecartOneCm,
        Self::BoatOneCm,
        Self::PigOneCm,
        Self::HappyGhastOneCm,
        Self::HorseOneCm,
        Self::AviateOneCm,
        Self::SwimOneCm,
        Self::StriderOneCm,
        Self::NautilusOneCm,
        Self::Jump,
        Self::Drop,
        Self::DamageDealt,
        Self::DamageDealtAbsorbed,
        Self::DamageDealtResisted,
        Self::DamageTaken,
        Self::DamageBlockedByShield,
        Self::DamageAbsorbed,
        Self::DamageResisted,
        Self::Deaths,
        Self::MobKills,
        Self::AnimalsBred,
        Self::PlayerKills,
        Self::FishCaught,
        Self::TalkedToVillager,
        Self::TradedWithVillager,
        Self::EatCakeSlice,
        Self::FillCauldron,
        Self::UseCauldron,
        Self::CleanArmor,
        Self::CleanBanner,
        Self::CleanShulkerBox,
        Self::InteractWithBrewingStand,
        Self::InteractWithBeacon,
        Self::InspectDropper,
        Self::InspectHopper,
        Self::InspectDispenser,
        Self::PlayNoteBlock,
        Self::TuneNoteBlock,
        Self::PotFlower,
        Self::TriggerTrappedChest,
        Self::OpenEnderChest,
        Self::EnchantItem,
        Self::PlayRecord,
        Self::InteractWithFurnace,
        Self::InteractWithCraftingTable,
        Self::OpenChest,
        Self::SleepInBed,
        Self::SleepInStrawBed,
        Self::OpenShulkerBox,
        Self::OpenBarrel,
        Self::InteractWithBlastFurnace,
        Self::InteractWithSmoker,
        Self::InteractWithLectern,
        Self::InteractWithCampfire,
        Self::InteractWithCartographyTable,
        Self::InteractWithLoom,
        Self::InteractWithStonecutter,
        Self::BellRing,
        Self::RaidTrigger,
        Self::RaidWin,
        Self::InteractWithAnvil,
        Self::InteractWithGrindstone,
        Self::TargetHit,
        Self::InteractWithSmithingTable,
    ];

    #[must_use]
    pub const fn id(self) -> u32 {
        self as u32
    }

    #[must_use]
    pub fn from_id(id: u32) -> Option<Self> {
        Self::ALL.get(id as usize).copied()
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::LeaveGame => "minecraft:leave_game",
            Self::PlayTime => "minecraft:play_time",
            Self::TotalWorldTime => "minecraft:total_world_time",
            Self::TimeSinceDeath => "minecraft:time_since_death",
            Self::TimeSinceRest => "minecraft:time_since_rest",
            Self::SneakTime => "minecraft:sneak_time",
            Self::WalkOneCm => "minecraft:walk_one_cm",
            Self::CrouchOneCm => "minecraft:crouch_one_cm",
            Self::SprintOneCm => "minecraft:sprint_one_cm",
            Self::WalkOnWaterOneCm => "minecraft:walk_on_water_one_cm",
            Self::FallOneCm => "minecraft:fall_one_cm",
            Self::ClimbOneCm => "minecraft:climb_one_cm",
            Self::FlyOneCm => "minecraft:fly_one_cm",
            Self::WalkUnderWaterOneCm => "minecraft:walk_under_water_one_cm",
            Self::MinecartOneCm => "minecraft:minecart_one_cm",
            Self::BoatOneCm => "minecraft:boat_one_cm",
            Self::PigOneCm => "minecraft:pig_one_cm",
            Self::HappyGhastOneCm => "minecraft:happy_ghast_one_cm",
            Self::HorseOneCm => "minecraft:horse_one_cm",
            Self::AviateOneCm => "minecraft:aviate_one_cm",
            Self::SwimOneCm => "minecraft:swim_one_cm",
            Self::StriderOneCm => "minecraft:strider_one_cm",
            Self::NautilusOneCm => "minecraft:nautilus_one_cm",
            Self::Jump => "minecraft:jump",
            Self::Drop => "minecraft:drop",
            Self::DamageDealt => "minecraft:damage_dealt",
            Self::DamageDealtAbsorbed => "minecraft:damage_dealt_absorbed",
            Self::DamageDealtResisted => "minecraft:damage_dealt_resisted",
            Self::DamageTaken => "minecraft:damage_taken",
            Self::DamageBlockedByShield => "minecraft:damage_blocked_by_shield",
            Self::DamageAbsorbed => "minecraft:damage_absorbed",
            Self::DamageResisted => "minecraft:damage_resisted",
            Self::Deaths => "minecraft:deaths",
            Self::MobKills => "minecraft:mob_kills",
            Self::AnimalsBred => "minecraft:animals_bred",
            Self::PlayerKills => "minecraft:player_kills",
            Self::FishCaught => "minecraft:fish_caught",
            Self::TalkedToVillager => "minecraft:talked_to_villager",
            Self::TradedWithVillager => "minecraft:traded_with_villager",
            Self::EatCakeSlice => "minecraft:eat_cake_slice",
            Self::FillCauldron => "minecraft:fill_cauldron",
            Self::UseCauldron => "minecraft:use_cauldron",
            Self::CleanArmor => "minecraft:clean_armor",
            Self::CleanBanner => "minecraft:clean_banner",
            Self::CleanShulkerBox => "minecraft:clean_shulker_box",
            Self::InteractWithBrewingStand => "minecraft:interact_with_brewingstand",
            Self::InteractWithBeacon => "minecraft:interact_with_beacon",
            Self::InspectDropper => "minecraft:inspect_dropper",
            Self::InspectHopper => "minecraft:inspect_hopper",
            Self::InspectDispenser => "minecraft:inspect_dispenser",
            Self::PlayNoteBlock => "minecraft:play_noteblock",
            Self::TuneNoteBlock => "minecraft:tune_noteblock",
            Self::PotFlower => "minecraft:pot_flower",
            Self::TriggerTrappedChest => "minecraft:trigger_trapped_chest",
            Self::OpenEnderChest => "minecraft:open_enderchest",
            Self::EnchantItem => "minecraft:enchant_item",
            Self::PlayRecord => "minecraft:play_record",
            Self::InteractWithFurnace => "minecraft:interact_with_furnace",
            Self::InteractWithCraftingTable => "minecraft:interact_with_crafting_table",
            Self::OpenChest => "minecraft:open_chest",
            Self::SleepInBed => "minecraft:sleep_in_bed",
            Self::SleepInStrawBed => "minecraft:sleep_in_straw_bed",
            Self::OpenShulkerBox => "minecraft:open_shulker_box",
            Self::OpenBarrel => "minecraft:open_barrel",
            Self::InteractWithBlastFurnace => "minecraft:interact_with_blast_furnace",
            Self::InteractWithSmoker => "minecraft:interact_with_smoker",
            Self::InteractWithLectern => "minecraft:interact_with_lectern",
            Self::InteractWithCampfire => "minecraft:interact_with_campfire",
            Self::InteractWithCartographyTable => "minecraft:interact_with_cartography_table",
            Self::InteractWithLoom => "minecraft:interact_with_loom",
            Self::InteractWithStonecutter => "minecraft:interact_with_stonecutter",
            Self::BellRing => "minecraft:bell_ring",
            Self::RaidTrigger => "minecraft:raid_trigger",
            Self::RaidWin => "minecraft:raid_win",
            Self::InteractWithAnvil => "minecraft:interact_with_anvil",
            Self::InteractWithGrindstone => "minecraft:interact_with_grindstone",
            Self::TargetHit => "minecraft:target_hit",
            Self::InteractWithSmithingTable => "minecraft:interact_with_smithing_table",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stat| stat.name() == name)
    }
}
/// Lazy timer anchor. Rates are bit flags in `TIMED_STATS` order; values live
/// in the fixed custom array. Only resets and activity transitions change this.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatisticsTimers {
    pub base_tick: u64,
    pub rates: u8,
}

const TIMED_STATS: [CustomStat; 5] = [
    CustomStat::PlayTime,
    CustomStat::TotalWorldTime,
    CustomStat::TimeSinceDeath,
    CustomStat::TimeSinceRest,
    CustomStat::SneakTime,
];

/// Registry release that persisted `sparse` ids and `custom` slots index.
pub const STATISTICS_RELEASE: &str = "26.3";

/// Durable counters plus the original JSON envelope. Keeping the envelope
/// preserves unknown root fields, categories, stat names and values, including
/// `DataVersion`; only recognized counters are refreshed on vanilla export.
/// Records written before the 26.3 bump carry no release and are migrated on
/// load (see [`StoredStatistics`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "StoredStatistics")]
pub struct PlayerStatistics {
    #[serde(serialize_with = "serialize_custom")]
    custom: [i32; CustomStat::COUNT],
    custom_present: [u64; 2],
    sparse: BTreeMap<StatType, BTreeMap<u32, i32>>,
    retained: Value,
    #[serde(default)]
    timers: StatisticsTimers,
    /// Always [`STATISTICS_RELEASE`] in memory; written so loads can tell.
    registry_release: CurrentRelease,
    /// The current simulation tick is a read cursor, not a durable mutation.
    #[serde(skip)]
    current_tick: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct CurrentRelease;

impl Serialize for CurrentRelease {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(STATISTICS_RELEASE)
    }
}

impl<'de> Deserialize<'de> for CurrentRelease {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let release = String::deserialize(deserializer)?;
        if release == STATISTICS_RELEASE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "statistics indexed by unsupported registry release {release:?}"
            )))
        }
    }
}

/// The persisted shape. A missing `registry_release` marks a 26.1.2 record:
/// its `sparse` ids name 26.1.2 registry entries and its `custom` array lacks
/// `sleep_in_straw_bed`.
#[derive(Deserialize)]
struct StoredStatistics {
    custom: Vec<i32>,
    custom_present: [u64; 2],
    sparse: BTreeMap<StatType, BTreeMap<u32, i32>>,
    retained: Value,
    #[serde(default)]
    timers: StatisticsTimers,
    #[serde(default)]
    registry_release: Option<CurrentRelease>,
}

impl TryFrom<StoredStatistics> for PlayerStatistics {
    type Error = String;

    fn try_from(stored: StoredStatistics) -> Result<Self, Self::Error> {
        let StoredStatistics {
            mut custom,
            mut custom_present,
            mut sparse,
            retained,
            timers,
            registry_release,
        } = stored;
        if registry_release.is_none() {
            // 26.3 inserted `sleep_in_straw_bed` mid-registry: shift the
            // counters and their presence bits from that slot up by one.
            let inserted = CustomStat::SleepInStrawBed as usize;
            if custom.len() != CustomStat::COUNT - 1 {
                return Err(format!(
                    "expected {} pre-26.3 custom counters, got {}",
                    CustomStat::COUNT - 1,
                    custom.len()
                ));
            }
            custom.insert(inserted, 0);
            let mut present = [0_u64; 2];
            for old in 0..CustomStat::COUNT - 1 {
                if custom_present[old / 64] & (1 << (old % 64)) != 0 {
                    let new = if old >= inserted { old + 1 } else { old };
                    present[new / 64] |= 1 << (new % 64);
                }
            }
            custom_present = present;
            sparse = sparse
                .into_iter()
                .map(|(kind, counters)| {
                    let counters = counters
                        .into_iter()
                        .filter_map(|(id, value)| {
                            crate::legacy_26_1_2::current_statistic_id(kind, id)
                                .map(|id| (id, value))
                        })
                        .collect();
                    (kind, counters)
                })
                .collect();
        }
        let custom = custom.try_into().map_err(|values: Vec<i32>| {
            format!(
                "expected {} custom counters, got {}",
                CustomStat::COUNT,
                values.len()
            )
        })?;
        Ok(Self {
            custom,
            custom_present,
            sparse,
            retained,
            timers,
            registry_release: CurrentRelease,
            current_tick: 0,
        })
    }
}

impl PartialEq for PlayerStatistics {
    fn eq(&self, other: &Self) -> bool {
        self.custom == other.custom
            && self.custom_present == other.custom_present
            && self.sparse == other.sparse
            && self.retained == other.retained
            && self.timers == other.timers
    }
}

impl Default for PlayerStatistics {
    fn default() -> Self {
        Self {
            custom: [0; CustomStat::COUNT],
            custom_present: [0; 2],
            sparse: BTreeMap::new(),
            retained: Value::Null,
            timers: StatisticsTimers::default(),
            registry_release: CurrentRelease,
            current_tick: 0,
        }
    }
}

impl PlayerStatistics {
    #[must_use]
    pub fn get_custom(&self, stat: CustomStat) -> i32 {
        let base = self.custom[stat as usize];
        if TIMED_STATS
            .iter()
            .position(|timer| *timer == stat)
            .is_some_and(|index| self.timers.rates & (1 << index) != 0)
        {
            let elapsed = self.current_tick.saturating_sub(self.timers.base_tick);
            return (i128::from(base) + i128::from(elapsed))
                .clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
        }
        base
    }

    pub fn set_custom(&mut self, stat: CustomStat, value: i32) {
        if TIMED_STATS.contains(&stat) {
            self.rebase_timers();
        }
        let index = stat as usize;
        self.custom[index] = value;
        self.custom_present[index / 64] |= 1_u64 << (index % 64);
    }

    pub fn increment_custom(&mut self, stat: CustomStat, amount: i32) {
        self.set_custom(stat, self.get_custom(stat).saturating_add(amount));
    }

    /// Move the transient read cursor; idle ticking changes no durable fields.
    pub fn advance_tick(&mut self, tick: u64) {
        self.current_tick = tick;
    }

    /// Connected play/world time, living death time, awake rest and crouch time.
    /// Calling with unchanged flags is allocation-free and changes no postimage.
    pub fn set_active(&mut self, connected: bool, alive: bool, sleeping: bool, sneaking: bool) {
        let rates = if connected {
            0b00011
                | if alive { 0b00100 } else { 0 }
                | if !sleeping { 0b01000 } else { 0 }
                | if sneaking { 0b10000 } else { 0 }
        } else {
            0
        };
        if self.timers.rates == rates {
            return;
        }
        self.rebase_timers();
        self.timers.rates = rates;
        for (index, stat) in TIMED_STATS.into_iter().enumerate() {
            if rates & (1 << index) != 0 {
                let index = stat as usize;
                self.custom_present[index / 64] |= 1_u64 << (index % 64);
            }
        }
    }

    fn rebase_timers(&mut self) {
        for stat in TIMED_STATS {
            self.custom[stat as usize] = self.get_custom(stat);
        }
        self.timers.base_tick = self.current_tick;
    }

    /// Snapshot at a simulation tick without changing the live lazy anchor.
    #[must_use]
    pub fn at_tick(&self, tick: u64) -> Self {
        let mut snapshot = self.clone();
        snapshot.advance_tick(tick);
        snapshot.rebase_timers();
        snapshot
    }

    /// Offline players stop at the final recovered tick, not wall-clock time.
    pub fn freeze_at_tick(&mut self, tick: u64) {
        self.advance_tick(tick);
        self.set_active(false, false, false, false);
    }

    #[must_use]
    pub fn timers(&self) -> StatisticsTimers {
        self.timers
    }

    /// Growth inputs for a memory estimate: the number of sparse counters and
    /// the preserved vanilla JSON envelope. No allocation, no serialization.
    #[must_use]
    pub fn heap_parts(&self) -> (usize, &Value) {
        (
            self.sparse.values().map(BTreeMap::len).sum(),
            &self.retained,
        )
    }

    pub fn apply_timers(&mut self, timers: StatisticsTimers) {
        self.timers = timers;
    }

    /// Changed absolute base counters. Timer progression alone has no delta.
    #[must_use]
    pub fn changes_since(&self, previous: &Self) -> Vec<(u32, u32, i32)> {
        let mut changes = Vec::new();
        for stat in CustomStat::ALL {
            let index = stat as usize;
            let bit = 1_u64 << (index % 64);
            if self.custom_present[index / 64] & bit != 0
                && (previous.custom_present[index / 64] & bit == 0
                    || self.custom[index] != previous.custom[index])
            {
                changes.push((StatType::Custom.id(), stat.id(), self.custom[index]));
            }
        }
        for (&kind, counters) in &self.sparse {
            for (&id, &value) in counters {
                if previous
                    .sparse
                    .get(&kind)
                    .and_then(|counters| counters.get(&id))
                    != Some(&value)
                {
                    changes.push((kind.id(), id, value));
                }
            }
        }
        changes
    }

    /// Apply a validated WAL delta without interpreting base values as resets.
    pub fn apply_changes(&mut self, changes: &[(u32, u32, i32)]) {
        for &(kind, id, value) in changes {
            let Some(kind) = StatType::from_id(kind) else {
                continue;
            };
            if kind == StatType::Custom {
                if let Some(stat) = CustomStat::from_id(id) {
                    let index = stat as usize;
                    self.custom[index] = value;
                    self.custom_present[index / 64] |= 1_u64 << (index % 64);
                }
            } else {
                self.sparse.entry(kind).or_default().insert(id, value);
            }
        }
    }

    /// `stat_id` is a block/item/entity registry id, not a block-state id.
    pub fn increment(&mut self, kind: StatType, stat_id: u32, amount: i32) {
        if kind == StatType::Custom {
            if let Some(stat) = CustomStat::from_id(stat_id) {
                self.increment_custom(stat, amount);
            }
            return;
        }
        let value = self
            .sparse
            .entry(kind)
            .or_default()
            .entry(stat_id)
            .or_default();
        *value = value.saturating_add(amount);
    }

    #[must_use]
    pub fn get(&self, kind: StatType, stat_id: u32) -> i32 {
        if kind == StatType::Custom {
            return CustomStat::from_id(stat_id).map_or(0, |stat| self.get_custom(stat));
        }
        self.sparse
            .get(&kind)
            .and_then(|stats| stats.get(&stat_id))
            .copied()
            .unwrap_or(0)
    }

    /// All nonzero absolute counters for AwardStats; WAL also retains zeros.
    #[must_use]
    pub fn wire_entries(&self) -> Vec<(i32, i32, i32)> {
        let sparse_len: usize = self.sparse.values().map(BTreeMap::len).sum();
        let mut entries = Vec::with_capacity(sparse_len + CustomStat::COUNT);
        for (&kind, stats) in &self.sparse {
            for (&id, &value) in stats {
                if let Ok(id) = i32::try_from(id)
                    && value != 0
                {
                    entries.push((kind.id() as i32, id, value));
                }
            }
        }
        for stat in CustomStat::ALL {
            if self.get_custom(stat) != 0
                && self.custom_present[stat as usize / 64] & (1_u64 << (stat as usize % 64)) != 0
            {
                entries.push((
                    StatType::Custom.id() as i32,
                    stat.id() as i32,
                    self.get_custom(stat),
                ));
            }
        }
        entries
    }

    /// Project known names using the caller's local block/item/entity registries.
    /// Unrecognized categories, names and non-i32 values remain in the envelope.
    pub fn from_json(
        value: Value,
        mut resolve: impl FnMut(StatType, &str) -> Option<u32>,
    ) -> Result<Self, String> {
        let root = value
            .as_object()
            .ok_or("statistics root must be an object")?;
        let mut result = Self::default();
        if let Some(stats) = root.get("stats") {
            let stats = stats
                .as_object()
                .ok_or("statistics stats must be an object")?;
            for (category, counters) in stats {
                let Some(kind) = StatType::from_name(category) else {
                    continue;
                };
                let Some(counters) = counters.as_object() else {
                    continue;
                };
                for (name, counter) in counters {
                    let Some(counter) = counter.as_i64().and_then(|v| i32::try_from(v).ok()) else {
                        continue;
                    };
                    if kind == StatType::Custom {
                        if let Some(stat) = CustomStat::from_name(name) {
                            result.set_custom(stat, counter);
                        }
                    } else if let Some(id) = resolve(kind, name) {
                        result.sparse.entry(kind).or_default().insert(id, counter);
                    }
                }
            }
        }
        result.retained = value;
        Ok(result)
    }

    /// Refresh owned counters without dropping any other JSON content.
    pub fn to_json(
        &self,
        mut name_of: impl FnMut(StatType, u32) -> Option<String>,
    ) -> Result<Value, String> {
        let mut value = if self.retained.is_null() {
            serde_json::json!({"DataVersion": 4790, "stats": {}})
        } else {
            self.retained.clone()
        };
        let root = value
            .as_object_mut()
            .ok_or("statistics root must be an object")?;
        let has_counters = self.custom_present != [0; 2] || !self.sparse.is_empty();
        if !root.contains_key("stats") && !has_counters {
            return Ok(value);
        }
        let stats = root
            .entry("stats")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or("statistics stats must be an object")?;
        for (&kind, counters) in &self.sparse {
            let category = stats
                .entry(kind.name())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .ok_or_else(|| format!("{} must be an object", kind.name()))?;
            for (&id, &counter) in counters {
                let name = name_of(kind, id)
                    .ok_or_else(|| format!("unresolved {} id {id}", kind.name()))?;
                category.insert(name, Value::from(counter));
            }
        }
        if self.custom_present != [0; 2] {
            let category = stats
                .entry(StatType::Custom.name())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .ok_or("minecraft:custom must be an object")?;
            for stat in CustomStat::ALL {
                if self.custom_present[stat as usize / 64] & (1_u64 << (stat as usize % 64)) != 0 {
                    category.insert(stat.name().into(), Value::from(self.get_custom(stat)));
                }
            }
        }
        Ok(value)
    }
}

fn serialize_custom<S: serde::Serializer>(
    value: &[i32; CustomStat::COUNT],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.as_slice().serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_vanilla_registry_and_saturating_counters() {
        assert_eq!(StatType::ALL.len(), 9);
        assert_eq!(CustomStat::ALL.len(), 78);
        for (id, kind) in StatType::ALL.into_iter().enumerate() {
            assert_eq!(kind.id(), id as u32);
            assert_eq!(StatType::from_name(kind.name()), Some(kind));
        }
        for (id, stat) in CustomStat::ALL.into_iter().enumerate() {
            assert_eq!(stat.id(), id as u32);
            assert_eq!(CustomStat::from_name(stat.name()), Some(stat));
        }
        assert_eq!(CustomStat::HappyGhastOneCm.id(), 17);
        assert_eq!(CustomStat::NautilusOneCm.id(), 22);
        assert_eq!(CustomStat::SleepInStrawBed.id(), 61);
        assert_eq!(CustomStat::InteractWithSmithingTable.id(), 77);
        let mut stats = PlayerStatistics::default();
        stats.set_custom(CustomStat::PlayTime, i32::MAX);
        stats.increment_custom(CustomStat::PlayTime, 1);
        stats.set_custom(CustomStat::TimeSinceRest, 0);
        assert_eq!(stats.get_custom(CustomStat::PlayTime), i32::MAX);
        assert!(!stats.wire_entries().contains(&(8, 4, 0)));
        assert!(
            stats
                .changes_since(&PlayerStatistics::default())
                .contains(&(8, 4, 0))
        );
        let decoded: PlayerStatistics =
            serde_json::from_slice(&serde_json::to_vec(&stats).unwrap()).unwrap();
        assert_eq!(decoded, stats);
    }

    #[test]
    fn pre_26_3_records_migrate_custom_slots_and_registry_ids() {
        // 26.1.2 layout: 77 counters, `open_shulker_box` at slot 61, item
        // stats keyed by 26.1.2 item ids.
        let diamond = crate::Identifier::parse("minecraft:diamond").unwrap();
        let legacy_diamond = (0..)
            .find(|id| crate::legacy_26_1_2::item_name(*id) == Some(&diamond))
            .unwrap();
        let mut custom = vec![0; CustomStat::COUNT - 1];
        custom[60] = 3; // sleep_in_bed
        custom[61] = 7; // open_shulker_box
        let legacy = serde_json::json!({
            "custom": custom,
            "custom_present": [(1_u64 << 60) | (1_u64 << 61), 0],
            "sparse": {"Used": {legacy_diamond.to_string(): 5}},
            "retained": null,
        });
        let stats: PlayerStatistics = serde_json::from_value(legacy).unwrap();
        assert_eq!(stats.get_custom(CustomStat::SleepInBed), 3);
        assert_eq!(stats.get_custom(CustomStat::SleepInStrawBed), 0);
        assert_eq!(stats.get_custom(CustomStat::OpenShulkerBox), 7);
        let current_diamond = crate::items::solaris_required_items()
            .id_of(&diamond)
            .unwrap();
        assert_ne!(current_diamond, legacy_diamond);
        assert_eq!(stats.get(StatType::Used, current_diamond), 5);
        let wire = stats.wire_entries();
        assert!(wire.contains(&(8, CustomStat::OpenShulkerBox.id() as i32, 7)));
        assert!(
            !wire
                .iter()
                .any(|entry| entry.1 == CustomStat::SleepInStrawBed.id() as i32 && entry.0 == 8)
        );

        // Current records round-trip unchanged and name their release.
        let encoded = serde_json::to_value(&stats).unwrap();
        assert_eq!(encoded["registry_release"], STATISTICS_RELEASE);
        let decoded: PlayerStatistics = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, stats);
    }

    #[test]
    fn json_preserves_unknown_names_categories_values_and_envelope() {
        let input = serde_json::json!({"DataVersion": 4000, "future": {"array": [1, null, "x"]},
            "stats": {"future:category": {"future:name": {"value": 4}},
                "minecraft:custom": {"minecraft:jump": 2, "future:stat": 100000000000_u64},
                "minecraft:mined": {"minecraft:stone": 3, "future:block": 7},
                "minecraft:crafted": {"minecraft:stone": 4},
                "minecraft:used": {"minecraft:stone": 5},
                "minecraft:broken": {"minecraft:stone": 6},
                "minecraft:picked_up": {"minecraft:stone": 7},
                "minecraft:dropped": {"minecraft:stone": 8},
                "minecraft:killed": {"minecraft:cow": 9},
                "minecraft:killed_by": {"minecraft:cow": 10}}});
        let resolve = |kind, name: &str| match (kind, name) {
            (StatType::Killed | StatType::KilledBy, "minecraft:cow") => Some(2),
            (_, "minecraft:stone") => Some(1),
            _ => None,
        };
        let name = |kind, id| match (kind, id) {
            (StatType::Killed | StatType::KilledBy, 2) => Some("minecraft:cow".into()),
            (_, 1) => Some("minecraft:stone".into()),
            _ => None,
        };
        let mut stats = PlayerStatistics::from_json(input.clone(), resolve).unwrap();
        assert_eq!(stats.to_json(name).unwrap(), input);
        assert_eq!(stats.wire_entries().len(), 9);
        stats.increment_custom(CustomStat::Jump, 1);
        stats.increment(StatType::Mined, 1, 1);
        let mut expected = input;
        expected["stats"]["minecraft:custom"]["minecraft:jump"] = Value::from(3);
        expected["stats"]["minecraft:mined"]["minecraft:stone"] = Value::from(4);
        assert_eq!(stats.to_json(name).unwrap(), expected);
    }
}

#[cfg(test)]
mod timer_tests {
    use super::*;

    #[test]
    fn lazy_timers_are_exact_without_idle_postimage_changes() {
        let mut statistics = PlayerStatistics::default();
        statistics.advance_tick(10);
        statistics.set_active(true, true, false, false);
        let anchor = statistics.clone();
        let serialized_anchor = serde_json::to_vec(&anchor).unwrap();
        statistics.advance_tick(210);
        assert_eq!(statistics, anchor);
        assert_eq!(serde_json::to_vec(&statistics).unwrap(), serialized_anchor);
        assert!(statistics.changes_since(&anchor).is_empty());
        for stat in [
            CustomStat::PlayTime,
            CustomStat::TotalWorldTime,
            CustomStat::TimeSinceDeath,
            CustomStat::TimeSinceRest,
        ] {
            assert_eq!(statistics.get_custom(stat), 200);
        }
        assert_eq!(
            statistics.at_tick(310).get_custom(CustomStat::PlayTime),
            300
        );
        assert_eq!(statistics.get_custom(CustomStat::PlayTime), 200);
        statistics.set_custom(CustomStat::TimeSinceDeath, 0);
        statistics.set_active(true, false, false, true);
        statistics.advance_tick(211);
        assert_eq!(statistics.get_custom(CustomStat::TimeSinceDeath), 0);
        assert_eq!(statistics.get_custom(CustomStat::TimeSinceRest), 201);
        assert_eq!(statistics.get_custom(CustomStat::SneakTime), 1);
        statistics.set_custom(CustomStat::TimeSinceRest, 0);
        statistics.set_active(true, true, true, false);
        statistics.advance_tick(220);
        assert_eq!(statistics.get_custom(CustomStat::TimeSinceRest), 0);
        assert_eq!(statistics.get_custom(CustomStat::TimeSinceDeath), 9);
        statistics.freeze_at_tick(230);
        let stopped = statistics.get_custom(CustomStat::PlayTime);
        statistics.advance_tick(999);
        assert_eq!(statistics.get_custom(CustomStat::PlayTime), stopped);
    }

    #[test]
    fn compact_changes_preserve_zero_resets_and_lazy_anchor_idempotently() {
        let mut base = PlayerStatistics::default();
        base.advance_tick(1);
        base.set_active(true, true, false, false);
        base.set_custom(CustomStat::Jump, 5);
        let mut latest = base.clone();
        latest.advance_tick(20);
        latest.set_custom(CustomStat::Jump, 0);
        latest.set_custom(CustomStat::TimeSinceRest, 0);
        latest.increment(StatType::Used, 7, 3);
        let changes = latest.changes_since(&base);
        assert!(changes.contains(&(8, CustomStat::Jump.id(), 0)));
        base.apply_changes(&changes);
        base.apply_timers(latest.timers());
        base.advance_tick(20);
        assert_eq!(base, latest);
        base.apply_changes(&changes);
        base.apply_timers(latest.timers());
        assert_eq!(base, latest);
        assert!(!latest.wire_entries().iter().any(|entry| entry.2 == 0));
        let mut decoded: PlayerStatistics =
            serde_json::from_slice(&serde_json::to_vec(&latest).unwrap()).unwrap();
        decoded.freeze_at_tick(30);
        assert_eq!(decoded.get_custom(CustomStat::TimeSinceRest), 10);
        assert_eq!(decoded.get_custom(CustomStat::PlayTime), 29);
    }
}
