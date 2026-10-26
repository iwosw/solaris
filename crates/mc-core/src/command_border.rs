//! Tick-owned world borders: vanilla 26.1.2 `WorldBorder` semantics
//! (pinned server-26.1.2.jar SHA256
//! 4723380bd2a0a0206719b50f2e390383afdaf82b0a76a0d573baf788e6aa3e86).
//! `size` is the current extent; a timed `set`/`add` installs a
//! `MovingBorderExtent` (from size, target size, remaining ticks) that the
//! native tick advances once per tick. Damage/warning fields ride the same
//! struct so checkpoints and the init packet observe one owner.
use crate::commands::{CommandError, Coord, coord};
use serde::{Deserialize, Serialize};

pub const MAX_SIZE: f64 = 59_999_968.0;
/// Vanilla `WorldBorder` constructor defaults (`WorldBorder.javap`: ctor
/// stores damagePerBlock 0.2, safeZone 5.0, warningTime 15, warningBlocks 5).
pub const DEFAULT_DAMAGE_PER_BLOCK: f64 = 0.2;
pub const DEFAULT_SAFE_ZONE: f64 = 5.0;
pub const DEFAULT_WARNING_TIME: i32 = 15;
pub const DEFAULT_WARNING_BLOCKS: i32 = 5;
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Border {
    pub center_x: f64,
    pub center_z: f64,
    pub size: f64,
    /// In-flight interpolation target; equals `size` when settled.
    pub lerp_target: f64,
    /// Remaining interpolation ticks; zero when settled.
    pub lerp_remaining: i64,
    /// Vanilla `damagePerBlock` (`worldborder damage amount`).
    pub damage_per_block: f64,
    /// Vanilla `safeZone` (`worldborder damage buffer`).
    pub safe_zone: f64,
    /// Vanilla `warningTime` in seconds (`worldborder warning time`).
    pub warning_time: i32,
    /// Vanilla `warningBlocks` (`worldborder warning distance`).
    pub warning_blocks: i32,
}
impl Default for Border {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_z: 0.0,
            size: MAX_SIZE,
            lerp_target: MAX_SIZE,
            lerp_remaining: 0,
            damage_per_block: DEFAULT_DAMAGE_PER_BLOCK,
            safe_zone: DEFAULT_SAFE_ZONE,
            warning_time: DEFAULT_WARNING_TIME,
            warning_blocks: DEFAULT_WARNING_BLOCKS,
        }
    }
}
impl Border {
    pub fn validate(self) -> Result<(), crate::config::CoreError> {
        for (name, value) in [
            ("center_x", self.center_x),
            ("center_z", self.center_z),
            ("size", self.size),
            ("lerp_target", self.lerp_target),
            ("damage_per_block", self.damage_per_block),
            ("safe_zone", self.safe_zone),
        ] {
            if !value.is_finite() {
                return Err(crate::config::CoreError::Storage(format!(
                    "worldborder.{name}: non-finite number"
                )));
            }
        }
        if self.center_x.abs() > 30_000_000.0 || self.center_z.abs() > 30_000_000.0 {
            return Err(crate::config::CoreError::Storage(
                "worldborder.center: outside +/-30000000".into(),
            ));
        }
        if !(1.0..=MAX_SIZE).contains(&self.size) {
            return Err(crate::config::CoreError::Storage(
                "worldborder.size: outside 1..59999968".into(),
            ));
        }
        if !(1.0..=MAX_SIZE).contains(&self.lerp_target) {
            return Err(crate::config::CoreError::Storage(
                "worldborder.lerp_target: outside 1..59999968".into(),
            ));
        }
        if self.lerp_remaining < 0 {
            return Err(crate::config::CoreError::Storage(
                "worldborder.lerp_remaining: negative".into(),
            ));
        }
        if self.damage_per_block < 0.0 {
            return Err(crate::config::CoreError::Storage(
                "worldborder.damage_per_block: negative".into(),
            ));
        }
        if self.safe_zone < 0.0 {
            return Err(crate::config::CoreError::Storage(
                "worldborder.safe_zone: negative".into(),
            ));
        }
        if self.warning_time < 0 || self.warning_blocks < 0 {
            return Err(crate::config::CoreError::Storage(
                "worldborder.warning: negative".into(),
            ));
        }
        Ok(())
    }
    /// Advance one native tick of an in-flight interpolation, matching
    /// vanilla `MovingBorderExtent` linear progress (`size +=
    /// (target - size) / remaining; remaining -= 1`, settling exact).
    pub fn advance_tick(&mut self) {
        if self.lerp_remaining <= 0 {
            self.size = self.lerp_target;
            return;
        }
        self.lerp_remaining -= 1;
        if self.lerp_remaining == 0 {
            self.size = self.lerp_target;
        } else {
            self.size += (self.lerp_target - self.size) / (self.lerp_remaining + 1) as f64;
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderCommand {
    Center([Coord; 2]),
    Set { size: f64, time: i64 },
    Add { delta: f64, time: i64 },
    DamageBuffer(f64),
    DamageAmount(f64),
    WarningDistance(i32),
    WarningTime(i32),
    Get,
}
/// Vanilla `TimeArgument` units (TimeArgument.javap static init:
/// d=24000, s=20, t=1, bare=1); minimum 0 for the border time argument
/// (`TimeArgument.time(0)` at WorldBorderCommand.javap registration).
pub fn parse_time(raw: &str) -> Option<i64> {
    let (number, unit) = if let Some(prefix) = raw.strip_suffix('d') {
        (prefix, 24_000_i64)
    } else if let Some(prefix) = raw.strip_suffix('s') {
        (prefix, 20_i64)
    } else if let Some(prefix) = raw.strip_suffix('t') {
        (prefix, 1_i64)
    } else {
        (raw, 1_i64)
    };
    let value: f64 = number.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let scaled = (value * unit as f64).round();
    (scaled < i64::MAX as f64).then_some(scaled as i64)
}
pub fn parse(args: &[&str]) -> Result<BorderCommand, CommandError> {
    let invalid = || {
        CommandError::Usage(
            "worldborder <center x z|set size [time]|add size [time]|damage buffer|damage amount|warning distance|warning time|get>",
        )
    };
    let number = |raw: &str| {
        raw.parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or_else(invalid)
    };
    match args {
        ["get"] => Ok(BorderCommand::Get),
        ["center", x, z] => {
            let x = coord(x).filter(|c| !c.local).ok_or_else(invalid)?;
            let z = coord(z).filter(|c| !c.local).ok_or_else(invalid)?;
            Ok(BorderCommand::Center([x, z]))
        }
        ["damage", "buffer", value] => {
            let value = number(value)?;
            if value < 0.0 {
                return Err(invalid());
            }
            Ok(BorderCommand::DamageBuffer(value))
        }
        ["damage", "amount", value] => {
            let value = number(value)?;
            if !(0.0..=5.0).contains(&value) {
                return Err(invalid());
            }
            Ok(BorderCommand::DamageAmount(value))
        }
        ["warning", "distance", value] => {
            let value = value.parse::<i32>().ok().filter(|v| *v >= 0).ok_or_else(invalid)?;
            Ok(BorderCommand::WarningDistance(value))
        }
        ["warning", "time", value] => {
            let value = value.parse::<i32>().ok().filter(|v| *v >= 0).ok_or_else(invalid)?;
            Ok(BorderCommand::WarningTime(value))
        }
        [mode @ ("set" | "add"), size] => {
            let size = number(size)?;
            if *mode == "set" {
                if !(1.0..=MAX_SIZE).contains(&size) {
                    return Err(invalid());
                }
                Ok(BorderCommand::Set { size, time: 0 })
            } else {
                Ok(BorderCommand::Add { delta: size, time: 0 })
            }
        }
        [mode @ ("set" | "add"), size, time] => {
            let size = number(size)?;
            let time = parse_time(time).ok_or_else(invalid)?;
            if *mode == "set" {
                if !(1.0..=MAX_SIZE).contains(&size) {
                    return Err(invalid());
                }
                Ok(BorderCommand::Set { size, time })
            } else {
                Ok(BorderCommand::Add { delta: size, time })
            }
        }
        _ => Err(invalid()),
    }
}
pub(crate) fn execute(border: &mut Border, command: BorderCommand, origin: [f64; 3]) -> String {
    match command {
        BorderCommand::Get => return format!("The world border is {} blocks wide", border.size),
        BorderCommand::Center([x, z]) => {
            border.center_x = (x.value + if x.relative { origin[0] } else { 0.0 })
                .clamp(-30_000_000.0, 30_000_000.0);
            border.center_z = (z.value + if z.relative { origin[2] } else { 0.0 })
                .clamp(-30_000_000.0, 30_000_000.0);
        }
        // Vanilla `setSize`: time>0 starts `lerpSizeBetween(current,
        // target, gameTime, gameTime + time)`; time==0 installs instantly
        // (`WorldBorderCommand.setSize` bytecode branches on lload time).
        // `add` targets current size + delta with the in-flight remaining
        // time added (`lambda$register$1`: getSize + distance, getLerpTime
        // + time).
        BorderCommand::Set { size, time } => {
            if time > 0 && size != border.size {
                border.lerp_target = size;
                border.lerp_remaining = time;
            } else {
                border.size = size;
                border.lerp_target = size;
                border.lerp_remaining = 0;
            }
        }
        BorderCommand::Add { delta, time } => {
            let target = (border.size + delta).clamp(1.0, MAX_SIZE);
            let remaining = if time > 0 {
                border.lerp_remaining.saturating_add(time)
            } else {
                0
            };
            if remaining > 0 && target != border.size {
                border.lerp_target = target;
                border.lerp_remaining = remaining;
            } else {
                border.size = target;
                border.lerp_target = target;
                border.lerp_remaining = 0;
            }
        }
        BorderCommand::DamageBuffer(value) => border.safe_zone = value,
        BorderCommand::DamageAmount(value) => border.damage_per_block = value,
        BorderCommand::WarningDistance(value) => border.warning_blocks = value,
        BorderCommand::WarningTime(value) => border.warning_time = value,
    }
    format!("Set the world border to {} blocks wide", border.size)
}

#[cfg(test)]
#[path = "command_border_tests.rs"]
mod tests;
