//! Player survival (ADR 0013): health, hunger, regeneration, starvation, air,
//! fall damage, damage reduction and experience. Pure transitions over owned
//! state; the merge step applies them in its fixed order.

use mc_data::ItemStack;
use mc_data::armor::{ArmorStats, armor_reduced_damage, protection_reduced_damage};
use mc_data::food::FoodEntry;

use crate::inventory::ItemRules;

pub const MAX_HEALTH: f32 = 20.0;
pub const MAX_FOOD: i32 = 20;
pub const MAX_AIR: i32 = 300;
pub const BLOCK_BREAK_EXHAUSTION: f32 = 0.005;
pub const ATTACK_EXHAUSTION: f32 = 0.1;
pub const SPRINT_EXHAUSTION_PER_METER: f32 = 0.1;
pub const SWIM_EXHAUSTION_PER_METER: f32 = 0.01;
pub const JUMP_EXHAUSTION: f32 = 0.05;
pub const SPRINT_JUMP_EXHAUSTION: f32 = 0.2;
/// Exhaustion of one hit from a damage type that has it (vanilla
/// `damage_type` `exhaustion` field).
const HIT_EXHAUSTION: f32 = 0.1;
const EXHAUSTION_STEP: f32 = 4.0;
const SATURATED_REGEN_TICKS: u32 = 10;
const HEALTH_TICK_PERIOD: u32 = 80;
const SAFE_FALL_DISTANCE: f64 = 3.0;
const DROWNING_AIR: i32 = -20;
const AIR_RECOVERY_PER_TICK: i32 = 4;
const DROWNING_DAMAGE: f32 = 2.0;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    Peaceful,
    #[default]
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    /// Protocol id (`ClientboundChangeDifficulty`).
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Vanilla `Player.hurtServer` difficulty scaling of mob damage.
    pub fn scale_mob_damage(self, amount: f32) -> f32 {
        match self {
            Self::Peaceful => 0.0,
            Self::Easy => (amount / 2.0 + 1.0).min(amount),
            Self::Normal => amount,
            Self::Hard => amount * 1.5,
        }
    }

    /// Starvation stops at this health (Hard kills).
    fn starvation_floor(self) -> Option<f32> {
        match self {
            Self::Peaceful | Self::Easy => Some(10.0),
            Self::Normal => Some(1.0),
            Self::Hard => None,
        }
    }
}

/// Damage sources that change how damage is reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageKind {
    Generic,
    Melee,
    Thorns,
    Projectile,
    /// Projectile and fire damage tags both apply to fireballs.
    Fireball,
    /// Fire-tagged lightning damage still passes through armor.
    Lightning,
    Explosion,
    /// Standing in a fire block.
    InFire,
    /// Burning.
    OnFire,
    Lava,
    Suffocation,
    Cactus,
    SweetBerryBush,
    HotFloor,
    Campfire,
    Fall,
    Drowning,
    Starvation,
    Poison,
    Wither,
    Magic,
    /// Out of world and `/kill`: no reduction at all.
    Void,
}

impl DamageKind {
    /// Shared vanilla damage identity for wire publication and advancement tags.
    pub(crate) fn type_name(
        self,
        source: Option<crate::intent::DamageSource>,
        player_attack: bool,
    ) -> &'static str {
        use crate::intent::DamageSource;
        match (self, source) {
            (Self::Melee, Some(DamageSource::Mace { attack, .. }))
                if attack.fall_distance > 0.0 =>
            {
                "minecraft:mace_smash"
            }
            (Self::Melee, _) if player_attack => "minecraft:player_attack",
            (Self::Melee, _) => "minecraft:mob_attack",
            (Self::Projectile, Some(DamageSource::Projectile { direct_type_id, .. })) => {
                match mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(
                    direct_type_id,
                )
                .map(|contract| contract.name)
                {
                    Some("minecraft:trident") => "minecraft:trident",
                    Some("minecraft:arrow" | "minecraft:spectral_arrow") => "minecraft:arrow",
                    _ => "minecraft:thrown",
                }
            }
            (Self::Fireball, _) => "minecraft:fireball",
            (Self::Lightning, _) => "minecraft:lightning_bolt",
            (Self::Explosion, _) => "minecraft:explosion",
            (Self::InFire, _) => "minecraft:in_fire",
            (Self::OnFire, _) => "minecraft:on_fire",
            (Self::Lava, _) => "minecraft:lava",
            (Self::Suffocation, _) => "minecraft:in_wall",
            (Self::Cactus, _) => "minecraft:cactus",
            (Self::SweetBerryBush, _) => "minecraft:sweet_berry_bush",
            (Self::HotFloor, _) => "minecraft:hot_floor",
            (Self::Campfire, _) => "minecraft:campfire",
            (Self::Fall, _) => "minecraft:fall",
            (Self::Drowning, _) => "minecraft:drown",
            (Self::Starvation, _) => "minecraft:starve",
            (Self::Wither, _) => "minecraft:wither",
            (Self::Poison | Self::Magic, _) => "minecraft:magic",
            (Self::Void, _) => "minecraft:out_of_world",
            (Self::Thorns, _) => "minecraft:thorns",
            _ => "minecraft:generic",
        }
    }

    pub(crate) fn type_id(
        self,
        source: Option<crate::intent::DamageSource>,
        player_attack: bool,
    ) -> i32 {
        static IDS: std::sync::LazyLock<std::collections::HashMap<String, i32>> =
            std::sync::LazyLock::new(|| {
                (0..)
                    .map_while(|id| {
                        mc_data::required_registry_entry("damage_type", id)
                            .map(|name| (name.as_str().to_owned(), id as i32))
                    })
                    .collect()
            });
        IDS[self.type_name(source, player_attack)]
    }

    /// Vanilla `bypasses_armor` damage types among these kinds.
    pub(crate) fn bypasses_armor(self) -> bool {
        matches!(
            self,
            Self::Generic
                | Self::OnFire
                | Self::Fall
                | Self::Drowning
                | Self::Starvation
                | Self::Void
                | Self::Poison
                | Self::Wither
                | Self::Magic
                | Self::Suffocation
        )
    }

    /// Vanilla `damage_type` exhaustion, charged once per hit.
    fn exhaustion(self) -> f32 {
        match self {
            Self::Melee
            | Self::Thorns
            | Self::Projectile
            | Self::Fireball
            | Self::Explosion
            | Self::InFire
            | Self::Lava
            | Self::Cactus
            | Self::SweetBerryBush
            | Self::HotFloor
            | Self::Campfire => HIT_EXHAUSTION,
            Self::Generic
            | Self::OnFire
            | Self::Lightning
            | Self::Fall
            | Self::Drowning
            | Self::Starvation
            | Self::Poison
            | Self::Wither
            | Self::Magic
            | Self::Suffocation
            | Self::Void => 0.0,
        }
    }
}

/// A player's survival state.
#[derive(Debug, Clone, PartialEq)]
pub struct Vitals {
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub exhaustion: f32,
    pub air: i32,
    pub experience: Experience,
    regen_timer: u32,
}

impl Default for Vitals {
    fn default() -> Self {
        Self {
            health: MAX_HEALTH,
            food: MAX_FOOD,
            saturation: 5.0,
            exhaustion: 0.0,
            air: MAX_AIR,
            experience: Experience::default(),
            regen_timer: 0,
        }
    }
}

/// What one survival tick did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VitalsTick {
    /// Health, food or saturation changed: send `Health`.
    pub health_changed: bool,
    /// Air supply changed: publish the air entity data.
    pub air_changed: bool,
    /// Damage to apply through [`reduce_damage`] and [`Vitals::hurt`].
    pub damage: Vec<(DamageKind, f32)>,
}

impl Vitals {
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.health <= 0.0
    }

    /// One server tick. `can_drown` is false outside survival/adventure.
    pub fn tick(
        &mut self,
        difficulty: Difficulty,
        eye_in_water: bool,
        can_drown: bool,
        consume_air: bool,
        natural_regeneration: bool,
    ) -> VitalsTick {
        let mut out = VitalsTick::default();
        if self.is_dead() {
            return out;
        }
        let before = (self.health, self.food, self.saturation);
        if let Some(amount) = self.tick_regen(difficulty, natural_regeneration) {
            out.damage.push((DamageKind::Starvation, amount));
        }
        out.health_changed = before != (self.health, self.food, self.saturation);

        let air = self.air;
        if eye_in_water && can_drown {
            if consume_air {
                self.air -= 1;
                if self.air <= DROWNING_AIR {
                    self.air = 0;
                    out.damage.push((DamageKind::Drowning, DROWNING_DAMAGE));
                }
            }
        } else if self.air < MAX_AIR {
            self.air = (self.air + AIR_RECOVERY_PER_TICK).min(MAX_AIR);
        }
        out.air_changed = air != self.air;
        out
    }

    fn tick_regen(&mut self, difficulty: Difficulty, natural_regeneration: bool) -> Option<f32> {
        if self.food == 0
            && difficulty
                .starvation_floor()
                .is_some_and(|floor| self.health <= floor)
        {
            self.regen_timer = 0;
            return None;
        }
        if natural_regeneration
            && self.saturation > 0.0
            && self.food >= MAX_FOOD
            && self.health < MAX_HEALTH
        {
            self.regen_timer += 1;
            if self.regen_timer >= SATURATED_REGEN_TICKS {
                self.regen_timer = 0;
                let spent = self.saturation.min(6.0);
                self.heal(spent / 6.0);
                self.add_exhaustion(spent);
            }
            return None;
        }
        if natural_regeneration && self.food >= 18 && self.health < MAX_HEALTH {
            self.regen_timer += 1;
            if self.regen_timer >= HEALTH_TICK_PERIOD {
                self.regen_timer = 0;
                self.heal(1.0);
                self.add_exhaustion(6.0);
            }
            return None;
        }
        if self.food == 0 {
            self.regen_timer += 1;
            if self.regen_timer >= HEALTH_TICK_PERIOD {
                self.regen_timer = 0;
                return Some(1.0);
            }
            return None;
        }
        self.regen_timer = 0;
        None
    }

    pub fn heal(&mut self, amount: f32) {
        self.health = (self.health + amount.max(0.0)).clamp(0.0, MAX_HEALTH);
    }

    /// Applies already reduced damage of `kind`; returns true when this
    /// killed.
    pub fn hurt(&mut self, kind: DamageKind, amount: f32) -> bool {
        let amount = amount.max(0.0);
        if self.is_dead() || amount == 0.0 {
            return false;
        }
        self.health = (self.health - amount).clamp(0.0, MAX_HEALTH);
        self.add_exhaustion(kind.exhaustion());
        self.is_dead()
    }

    /// Vanilla `FoodData.addExhaustion`: every 4 points cost one saturation
    /// point, or one food point once saturation is gone.
    pub fn add_exhaustion(&mut self, amount: f32) {
        if !amount.is_finite() || amount <= 0.0 {
            return;
        }
        self.exhaustion += amount;
        while self.exhaustion >= EXHAUSTION_STEP {
            self.exhaustion -= EXHAUSTION_STEP;
            if self.saturation > 0.0 {
                self.saturation = (self.saturation - 1.0).max(0.0);
            } else {
                self.food = (self.food - 1).max(0);
            }
        }
    }

    #[must_use]
    pub fn can_eat(&self, food: &FoodEntry) -> bool {
        !self.is_dead() && (food.can_always_eat || self.food < MAX_FOOD)
    }

    /// Finishes eating `food`.
    pub fn eat(&mut self, food: &FoodEntry) {
        self.food = (self.food + food.food).clamp(0, MAX_FOOD);
        let added = if food.saturation.is_finite() {
            food.saturation.max(0.0)
        } else {
            0.0
        };
        self.saturation = (self.saturation + added).min(self.food as f32);
    }

    /// Respawn state: full vitals, experience lost.
    pub fn respawn(&mut self) {
        *self = Self::default();
    }
}

/// Vanilla experience level, progress within the level, and total points.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Experience {
    pub level: i32,
    pub progress: f32,
    pub total: i32,
}

impl Experience {
    /// Points needed to go from `level` to `level + 1`.
    #[must_use]
    pub fn points_for_level(level: i32) -> i32 {
        if level >= 30 {
            (level - 30).saturating_mul(9).saturating_add(112)
        } else if level >= 15 {
            37 + (level - 15) * 5
        } else {
            7 + level * 2
        }
    }

    /// Vanilla `Player.giveExperiencePoints`; negative points walk levels
    /// down and stop at zero.
    pub fn add_points(&mut self, points: i32) {
        self.progress += points as f32 / Self::points_for_level(self.level) as f32;
        self.total = self.total.saturating_add(points).max(0);
        while self.progress < 0.0 {
            let spill = self.progress * Self::points_for_level(self.level) as f32;
            if self.level > 0 {
                self.add_levels(-1);
                self.progress = 1.0 + spill / Self::points_for_level(self.level) as f32;
            } else {
                self.add_levels(-1);
                self.progress = 0.0;
            }
        }
        while self.progress >= 1.0 {
            self.progress = (self.progress - 1.0) * Self::points_for_level(self.level) as f32;
            self.level += 1;
            self.progress /= Self::points_for_level(self.level) as f32;
        }
    }

    /// Vanilla `Player.giveExperienceLevels`: below zero resets everything.
    pub fn add_levels(&mut self, levels: i32) {
        self.level = self.level.saturating_add(levels);
        if self.level < 0 {
            *self = Self::default();
        }
    }

    /// Points dropped as orbs on death.
    #[must_use]
    pub fn death_drop(&self) -> i32 {
        self.level.saturating_mul(7).min(100)
    }
}

/// One damage point per block fallen beyond three.
#[must_use]
pub fn fall_damage(fall_distance: f64) -> f32 {
    if !fall_distance.is_finite() || fall_distance <= SAFE_FALL_DISTANCE {
        return 0.0;
    }
    (fall_distance - SAFE_FALL_DISTANCE).ceil() as f32
}

/// Equipment slot names of the four armor slots, in `reduce_damage` order.
const ARMOR_SLOTS: [&str; 4] = ["feet", "legs", "chest", "head"];

/// Damage after armor (unless the kind bypasses it) and after the
/// enchantment protection factor of the worn armor. `armor` is the four armor
/// slots (feet, legs, chest, head). Armor values come from each stack's item
/// facts and count only in the slot the item equips to.
#[must_use]
pub fn reduce_damage(
    rules: &ItemRules<'_>,
    armor: &[ItemStack],
    amount: f32,
    kind: DamageKind,
) -> f32 {
    reduce_damage_with_breach(rules, armor, amount, kind, 0)
}

pub(crate) fn reduce_damage_with_breach(
    rules: &ItemRules<'_>,
    armor: &[ItemStack],
    amount: f32,
    kind: DamageKind,
    breach: u32,
) -> f32 {
    reduce_damage_impl(rules, armor, amount, kind, breach, false)
}

pub(crate) fn reduce_damage_after_physical(
    rules: &ItemRules<'_>,
    armor: &[ItemStack],
    amount: f32,
    kind: DamageKind,
    breach: u32,
) -> f32 {
    reduce_damage_impl(rules, armor, amount, kind, breach, true)
}

fn reduce_damage_impl(
    rules: &ItemRules<'_>,
    armor: &[ItemStack],
    amount: f32,
    kind: DamageKind,
    breach: u32,
    physical: bool,
) -> f32 {
    if kind == DamageKind::Void {
        return amount.max(0.0);
    }
    let mut stats = ArmorStats {
        armor: 0.0,
        toughness: 0.0,
    };
    let mut protection = 0;
    for (stack, slot) in armor.iter().zip(ARMOR_SLOTS) {
        if stack.is_empty() {
            continue;
        }
        if let Some(facts) = rules
            .facts
            .facts_for_stack(stack, rules.items)
            .filter(|facts| {
                !physical
                    || facts
                        .physical_profile
                        .as_ref()
                        .is_none_or(|profile| profile.armor.is_none())
            })
            .and_then(|facts| facts.armor.as_ref())
            .filter(|facts| facts.slot == slot)
        {
            stats.armor += facts.armor;
            stats.toughness += facts.toughness;
        }
        protection += enchantment_level(stack, "minecraft:protection");
        protection += match kind {
            DamageKind::InFire
            | DamageKind::OnFire
            | DamageKind::Lava
            | DamageKind::Lightning
            | DamageKind::HotFloor
            | DamageKind::Campfire => 2 * enchantment_level(stack, "minecraft:fire_protection"),
            DamageKind::Fall => 3 * enchantment_level(stack, "minecraft:feather_falling"),
            DamageKind::Explosion => 2 * enchantment_level(stack, "minecraft:blast_protection"),
            DamageKind::Fireball => {
                2 * (enchantment_level(stack, "minecraft:fire_protection")
                    + enchantment_level(stack, "minecraft:projectile_protection"))
            }
            DamageKind::Projectile => {
                2 * enchantment_level(stack, "minecraft:projectile_protection")
            }
            _ => 0,
        };
    }
    let after_armor = if kind.bypasses_armor() {
        amount.max(0.0)
    } else {
        crate::enchantments::breach_damage(amount, armor_reduced_damage(amount, stats), breach)
    };
    protection_reduced_damage(after_armor, protection.min(20))
}

fn enchantment_level(stack: &ItemStack, id: &str) -> i32 {
    stack
        .enchantments
        .iter()
        .filter(|enchantment| enchantment.id.as_str() == id)
        .map(|enchantment| enchantment.level.max(0))
        .max()
        .unwrap_or(0)
}

/// Durability each worn armor piece loses from `incoming` damage (vanilla:
/// a quarter of the damage, at least one), unless the kind bypasses armor.
#[must_use]
pub fn armor_wear(incoming: f32, kind: DamageKind) -> i32 {
    if kind.bypasses_armor() || !incoming.is_finite() || incoming <= 0.0 {
        return 0;
    }
    ((incoming / 4.0).floor() as i32).max(1)
}

#[cfg(test)]
#[path = "survival_tests.rs"]
mod tests;
