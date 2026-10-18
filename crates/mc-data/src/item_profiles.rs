//! Immutable physical mechanics admitted with a canonical custom item definition.
//! Values are authored by providers; this module contains no combat policy.
use serde::{Deserialize, Serialize};

use crate::{Identifier, items::ItemRegistry};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalItemProfile {
    #[serde(default)]
    pub attacks: Vec<AttackProfile>,
    pub armor: Option<ArmorProfile>,
    pub stamina_capacity: f32,
    pub stamina_recovery_per_tick: f32,
    pub shield: Option<ShieldProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DamageKind {
    Cut,
    Pierce,
    Crush,
    Fire,
    Blast,
    Hook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttackMode {
    Melee,
    Projectile,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttackProfile {
    pub mode: AttackMode,
    pub damage_kind: DamageKind,
    pub base_damage_hp: f32,
    pub windup_ticks: u32,
    pub active_ticks: u32,
    pub recovery_ticks: u32,
    pub min_range_blocks: f64,
    pub reach_blocks: f64,
    pub stamina_cost: f32,
    pub shape: AttackShape,
    pub projectile: Option<ProjectileProfile>,
    pub consumption: Option<ItemConsumption>,
    pub brace: Option<BracePredicate>,
    pub charge: Option<ChargePredicate>,
    pub two_handed: bool,
    pub wear: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttackShape {
    Cone { half_angle_degrees: f64 },
    Thrust { radius_blocks: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemConsumption {
    pub item: Identifier,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BracePredicate {
    pub on_ground: bool,
    pub max_speed_blocks_per_tick: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChargePredicate {
    pub min_speed_blocks_per_tick: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectileProfile {
    pub carrier: Identifier,
    pub speed_blocks_per_tick: f64,
    pub gravity_blocks_per_tick_squared: f64,
    pub collision_radius_blocks: f64,
    pub lifetime_ticks: u32,
    pub recoverable: bool,
    pub impulse: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmorSlot {
    Head,
    Chest,
    Legs,
    Feet,
}
impl ArmorSlot {
    /// Canonical native endpoint order, deliberately not EntityEquipmentSlot ordinal.
    pub const fn equipment_index(self) -> usize {
        match self {
            Self::Head => 2,
            Self::Chest => 3,
            Self::Legs => 4,
            Self::Feet => 5,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Chest => "chest",
            Self::Legs => "legs",
            Self::Feet => "feet",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageRegion {
    Head,
    Torso,
    Arms,
    Legs,
    Feet,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmorCoverage {
    pub region: CoverageRegion,
    pub fraction: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DamageResistance {
    pub kind: DamageKind,
    pub fraction: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DegradationPoint {
    pub remaining_fraction: f32,
    pub resistance_multiplier: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmorProfile {
    pub slot: ArmorSlot,
    pub coverage: Vec<ArmorCoverage>,
    pub resistance: Vec<DamageResistance>,
    pub common_cap: f32,
    pub mass_units: f32,
    pub stamina_cost: f32,
    pub degradation: Vec<DegradationPoint>,
    pub wear_per_damage_hp: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShieldProfile {
    pub width_blocks: f64,
    pub height_blocks: f64,
    pub forward_offset_blocks: f64,
    pub bottom_offset_blocks: f64,
    pub activation_ticks: u32,
    pub manual: bool,
    pub deployable: bool,
    pub block_stamina_cost: f32,
    pub block_wear_per_damage_hp: f32,
    pub max_durability: u32,
    pub hook_disables_ticks: u32,
    pub impulse_limit: f64,
}

impl PhysicalItemProfile {
    pub fn validate(
        &self,
        items: &ItemRegistry,
        max_damage: Option<u32>,
        equip_slot: Option<&str>,
    ) -> Result<(), &'static str> {
        if !positive(self.stamina_capacity as f64)
            || !nonnegative(self.stamina_recovery_per_tick as f64)
        {
            return Err("invalid authored stamina mechanics");
        }
        if self.attacks.len() > 16
            || (self.attacks.is_empty() && self.armor.is_none() && self.shield.is_none())
        {
            return Err("empty or oversized physical profile");
        }
        for attack in &self.attacks {
            if !positive(attack.base_damage_hp as f64)
                || !nonnegative(attack.stamina_cost as f64)
                || !nonnegative(attack.min_range_blocks)
                || !positive(attack.reach_blocks)
                || attack.min_range_blocks >= attack.reach_blocks
                || attack.reach_blocks > 128.0
                || attack.active_ticks == 0
                || attack.windup_ticks > 72_000
                || attack.active_ticks > 72_000
                || attack.recovery_ticks > 72_000
                || (attack.wear > 0 && max_damage.is_none())
            {
                return Err("invalid attack mechanics");
            }
            match attack.shape {
                AttackShape::Cone { half_angle_degrees }
                    if positive(half_angle_degrees) && half_angle_degrees <= 180.0 => {}
                AttackShape::Thrust { radius_blocks }
                    if positive(radius_blocks) && radius_blocks <= attack.reach_blocks => {}
                _ => return Err("invalid attack shape"),
            }
            if (attack.mode == AttackMode::Projectile) != attack.projectile.is_some() {
                return Err("projectile mode requires exactly one projectile profile");
            }
            if let Some(projectile) = &attack.projectile
                && (!matches!(
                    projectile.carrier.as_str(),
                    "minecraft:arrow" | "minecraft:spectral_arrow"
                ) || !positive(projectile.speed_blocks_per_tick)
                    || projectile.speed_blocks_per_tick > 32.0
                    || !nonnegative(projectile.gravity_blocks_per_tick_squared)
                    || !positive(projectile.collision_radius_blocks)
                    || projectile.collision_radius_blocks > 4.0
                    || projectile.lifetime_ticks == 0
                    || projectile.lifetime_ticks > 72_000
                    || !nonnegative(projectile.impulse))
            {
                return Err("invalid native projectile profile");
            }
            if let Some(consumption) = &attack.consumption
                && (consumption.count == 0 || consumption.count > 64)
            {
                return Err("invalid attack consumption");
            }
            if attack
                .brace
                .is_some_and(|p| !nonnegative(p.max_speed_blocks_per_tick))
                || attack
                    .charge
                    .is_some_and(|p| !positive(p.min_speed_blocks_per_tick))
            {
                return Err("invalid brace or charge predicate");
            }
        }
        if let Some(armor) = &self.armor {
            if max_damage.is_none()
                || equip_slot != Some(armor.slot.name())
                || armor.coverage.is_empty()
                || armor.coverage.len() > 5
                || armor.resistance.is_empty()
                || armor.resistance.len() > 6
                || !fraction(armor.common_cap)
                || armor.common_cap >= 1.0
                || !nonnegative(armor.mass_units as f64)
                || !nonnegative(armor.stamina_cost as f64)
                || !nonnegative(armor.wear_per_damage_hp as f64)
                || armor.degradation.len() < 2
                || armor.degradation.len() > 16
            {
                return Err("invalid functional armor profile");
            }
            for (i, coverage) in armor.coverage.iter().enumerate() {
                if !fraction(coverage.fraction)
                    || armor.coverage[..i]
                        .iter()
                        .any(|p| p.region == coverage.region)
                {
                    return Err("invalid or duplicate armor coverage");
                }
            }
            for (i, resistance) in armor.resistance.iter().enumerate() {
                if !fraction(resistance.fraction)
                    || armor.resistance[..i]
                        .iter()
                        .any(|p| p.kind == resistance.kind)
                {
                    return Err("invalid or duplicate armor resistance");
                }
            }
            if armor
                .degradation
                .first()
                .is_none_or(|p| p.remaining_fraction != 0.0)
                || armor
                    .degradation
                    .last()
                    .is_none_or(|p| p.remaining_fraction != 1.0)
            {
                return Err("degradation must span broken to intact");
            }
            for (i, point) in armor.degradation.iter().enumerate() {
                if !fraction(point.remaining_fraction)
                    || !fraction(point.resistance_multiplier)
                    || (i > 0
                        && (point.remaining_fraction
                            <= armor.degradation[i - 1].remaining_fraction
                            || point.resistance_multiplier
                                < armor.degradation[i - 1].resistance_multiplier))
                {
                    return Err("invalid degradation curve");
                }
            }
        }
        if let Some(shield) = &self.shield
            && ((!shield.manual && !shield.deployable)
                || !positive(shield.width_blocks)
                || shield.width_blocks > 8.0
                || !positive(shield.height_blocks)
                || shield.height_blocks > 8.0
                || !nonnegative(shield.forward_offset_blocks)
                || shield.forward_offset_blocks > 4.0
                || !shield.bottom_offset_blocks.is_finite()
                || shield.bottom_offset_blocks.abs() > 4.0
                || shield.activation_ticks > 72_000
                || !nonnegative(shield.block_stamina_cost as f64)
                || !nonnegative(shield.block_wear_per_damage_hp as f64)
                || shield.max_durability == 0
                || Some(shield.max_durability) != max_damage
                || shield.hook_disables_ticks > 72_000
                || !nonnegative(shield.impulse_limit))
        {
            return Err("invalid physical shield profile");
        }
        let _ = items; // Cross-item closure is checked against the complete admitted table.
        Ok(())
    }
}
fn nonnegative(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}
fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}
fn fraction(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

impl AttackProfile {
    /// Actual pose/velocity, not policy hints, decides brace, charge and attack volume.
    pub fn contains(
        &self,
        origin: [f64; 3],
        yaw_degrees: f32,
        target: [f64; 3],
        velocity: [f64; 3],
        on_ground: bool,
    ) -> bool {
        let speed = velocity[0].hypot(velocity[2]);
        if self
            .brace
            .is_some_and(|p| (p.on_ground && !on_ground) || speed > p.max_speed_blocks_per_tick)
            || self
                .charge
                .is_some_and(|p| speed < p.min_speed_blocks_per_tick)
        {
            return false;
        }
        let delta = [
            target[0] - origin[0],
            target[1] - origin[1],
            target[2] - origin[2],
        ];
        let distance = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        if !distance.is_finite() || distance < self.min_range_blocks || distance > self.reach_blocks
        {
            return false;
        }
        let yaw = f64::from(yaw_degrees).to_radians();
        let forward = [-yaw.sin(), yaw.cos()];
        let longitudinal = delta[0] * forward[0] + delta[2] * forward[1];
        match self.shape {
            AttackShape::Cone { half_angle_degrees } => {
                distance == 0.0 || longitudinal / distance >= half_angle_degrees.to_radians().cos()
            }
            AttackShape::Thrust { radius_blocks } => {
                longitudinal >= 0.0
                    && (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]
                        - longitudinal * longitudinal)
                        .max(0.0)
                        <= radius_blocks * radius_blocks
            }
        }
    }
    pub fn active_at(&self, start_tick: u64, tick: u64) -> bool {
        let elapsed = tick.saturating_sub(start_tick);
        elapsed >= u64::from(self.windup_ticks)
            && elapsed < u64::from(self.windup_ticks) + u64::from(self.active_ticks)
    }
    pub fn ready_tick(&self, start_tick: u64) -> u64 {
        start_tick.saturating_add(
            u64::from(self.windup_ticks)
                + u64::from(self.active_ticks)
                + u64::from(self.recovery_ticks),
        )
    }
}

impl ArmorProfile {
    pub fn reduction(
        &self,
        region: CoverageRegion,
        kind: DamageKind,
        remaining_fraction: f32,
    ) -> f32 {
        let coverage = self
            .coverage
            .iter()
            .find(|p| p.region == region)
            .map_or(0.0, |p| p.fraction);
        let resistance = self
            .resistance
            .iter()
            .find(|p| p.kind == kind)
            .map_or(0.0, |p| p.fraction);
        let remaining = remaining_fraction.clamp(0.0, 1.0);
        let multiplier = self
            .degradation
            .windows(2)
            .find(|pair| remaining <= pair[1].remaining_fraction)
            .map_or(0.0, |pair| {
                let t = (remaining - pair[0].remaining_fraction)
                    / (pair[1].remaining_fraction - pair[0].remaining_fraction);
                pair[0].resistance_multiplier
                    + t * (pair[1].resistance_multiplier - pair[0].resistance_multiplier)
            });
        coverage * resistance * multiplier
    }
}

impl ShieldProfile {
    /// Segment-plane intersection with the real oriented shield rectangle. Rear and flank hits miss.
    pub fn intersects(
        &self,
        position: [f64; 3],
        yaw_degrees: f32,
        source: [f64; 3],
        impact: [f64; 3],
    ) -> bool {
        self.intersection_t(position, yaw_degrees, source, impact)
            .is_some()
    }

    pub fn intersection_t(
        &self,
        position: [f64; 3],
        yaw_degrees: f32,
        source: [f64; 3],
        impact: [f64; 3],
    ) -> Option<f64> {
        let yaw = f64::from(yaw_degrees).to_radians();
        let normal = [-yaw.sin(), 0.0, yaw.cos()];
        let center = [
            position[0] + normal[0] * self.forward_offset_blocks,
            position[1] + self.bottom_offset_blocks,
            position[2] + normal[2] * self.forward_offset_blocks,
        ];
        let start = (source[0] - center[0]) * normal[0] + (source[2] - center[2]) * normal[2];
        let end = (impact[0] - center[0]) * normal[0] + (impact[2] - center[2]) * normal[2];
        if !start.is_finite() || !end.is_finite() || start <= 0.0 || end > 0.0 {
            return None;
        }
        let t = start / (start - end);
        let point = [
            source[0] + (impact[0] - source[0]) * t,
            source[1] + (impact[1] - source[1]) * t,
            source[2] + (impact[2] - source[2]) * t,
        ];
        let side = (point[0] - center[0]) * normal[2] - (point[2] - center[2]) * normal[0];
        (side.abs() <= self.width_blocks * 0.5
            && point[1] >= center[1]
            && point[1] <= center[1] + self.height_blocks)
            .then_some(t)
    }

    pub fn intersects_swept(
        &self,
        position: [f64; 3],
        yaw_degrees: f32,
        source: [f64; 3],
        impact: [f64; 3],
        radius: f64,
    ) -> bool {
        self.intersection_t_swept(position, yaw_degrees, source, impact, radius)
            .is_some()
    }

    /// Earliest spherical contact with the real rectangle, including rounded
    /// edges and corners. Rear-originating and outward-moving sweeps miss.
    pub fn intersection_t_swept(
        &self,
        position: [f64; 3],
        yaw_degrees: f32,
        source: [f64; 3],
        impact: [f64; 3],
        radius: f64,
    ) -> Option<f64> {
        if !radius.is_finite()
            || radius < 0.0
            || !yaw_degrees.is_finite()
            || !position
                .into_iter()
                .chain(source)
                .chain(impact)
                .all(f64::is_finite)
        {
            return None;
        }
        if radius == 0.0 {
            return self.intersection_t(position, yaw_degrees, source, impact);
        }
        let yaw = f64::from(yaw_degrees).to_radians();
        let normal = [-yaw.sin(), 0.0, yaw.cos()];
        let center = [
            position[0] + normal[0] * self.forward_offset_blocks,
            position[1] + self.bottom_offset_blocks,
            position[2] + normal[2] * self.forward_offset_blocks,
        ];
        let local = |point: [f64; 3]| {
            [
                (point[0] - center[0]) * normal[0] + (point[2] - center[2]) * normal[2],
                (point[0] - center[0]) * normal[2] - (point[2] - center[2]) * normal[0],
                point[1] - center[1],
            ]
        };
        let start = local(source);
        let end = local(impact);
        if start[0] <= 0.0 || end[0] >= start[0] {
            return None;
        }
        let delta = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
        let half_width = self.width_blocks * 0.5;
        let mut cuts = [0.0; 6];
        cuts[1] = 1.0;
        let mut count = 2;
        for (axis, boundary) in [
            (1, -half_width),
            (1, half_width),
            (2, 0.0),
            (2, self.height_blocks),
        ] {
            if delta[axis] != 0.0 {
                let t = (boundary - start[axis]) / delta[axis];
                if t > 0.0 && t < 1.0 {
                    cuts[count] = t;
                    count += 1;
                }
            }
        }
        cuts[..count].sort_unstable_by(f64::total_cmp);
        for interval in cuts[..count].windows(2) {
            let low = interval[0];
            let high = interval[1];
            if high <= low {
                continue;
            }
            let middle = low + (high - low) * 0.5;
            let mut coefficients = [[start[0], delta[0]], [0.0; 2], [0.0; 2]];
            for (axis, min, max) in [(1, -half_width, half_width), (2, 0.0, self.height_blocks)] {
                let position = start[axis] + delta[axis] * middle;
                let boundary = if position < min {
                    Some(min)
                } else if position > max {
                    Some(max)
                } else {
                    None
                };
                if let Some(boundary) = boundary {
                    coefficients[axis] = [start[axis] - boundary, delta[axis]];
                }
            }
            let mut a = 0.0;
            let mut b = 0.0;
            let mut c = -radius * radius;
            for [origin, speed] in coefficients {
                a += speed * speed;
                b += 2.0 * origin * speed;
                c += origin * origin;
            }
            if (a * low + b) * low + c <= 0.0 {
                return Some(low);
            }
            if a == 0.0 {
                continue;
            }
            let discriminant = b * b - 4.0 * a * c;
            if discriminant < 0.0 {
                continue;
            }
            let q = -0.5 * (b + discriminant.sqrt().copysign(b));
            let root = if q == 0.0 {
                -b / (2.0 * a)
            } else {
                (q / a).min(c / q)
            };
            if root >= low && root <= high {
                return Some(root);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::ShieldProfile;

    #[test]
    fn native_shield_sweep_distinguishes_round_edges_from_square_corners() {
        let shield = ShieldProfile {
            width_blocks: 2.0,
            height_blocks: 2.0,
            forward_offset_blocks: 0.0,
            bottom_offset_blocks: 0.0,
            activation_ticks: 0,
            manual: true,
            deployable: true,
            block_stamina_cost: 1.0,
            block_wear_per_damage_hp: 1.0,
            max_durability: 50,
            hook_disables_ticks: 2,
            impulse_limit: 1.0,
        };
        let pose = [0.0, 0.0, 0.0];
        let edge = shield
            .intersection_t_swept(pose, 0.0, [1.2, 1.0, 2.0], [1.2, 1.0, -2.0], 0.25)
            .unwrap();
        assert!((edge - 0.4625).abs() < 1.0e-12);
        assert!(!shield.intersects_swept(pose, 0.0, [1.2, 1.0, 2.0], [1.2, 1.0, -2.0], 0.0));
        assert!(!shield.intersects_swept(pose, 0.0, [1.2, 2.2, 2.0], [1.2, 2.2, -2.0], 0.25));
        let corner = shield
            .intersection_t_swept(pose, 0.0, [1.1, 2.1, 2.0], [1.1, 2.1, -2.0], 0.25)
            .unwrap();
        assert!((corner - 0.4484611796797792).abs() < 1.0e-12);
        assert!(!shield.intersects_swept(pose, 0.0, [0.0, 1.0, -2.0], [0.0, 1.0, 2.0], 0.25));
        let front_contact = shield
            .intersection_t_swept(pose, 0.0, [0.0, 1.0, 2.0], [0.0, 1.0, 0.1], 0.25)
            .unwrap();
        assert!((front_contact - 0.9210526315789475).abs() < 1.0e-12);
    }
}
