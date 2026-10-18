//! Tick-owned instant world borders, matching OLD's center/set/add/get subset.
use crate::commands::{CommandError, Coord, coord};
use serde::{Deserialize, Serialize};

pub const MAX_SIZE: f64 = 59_999_968.0;
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Border {
    pub center_x: f64,
    pub center_z: f64,
    pub size: f64,
}
impl Default for Border {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_z: 0.0,
            size: MAX_SIZE,
        }
    }
}
impl Border {
    pub fn validate(self) -> Result<(), crate::config::CoreError> {
        for (name, value) in [
            ("center_x", self.center_x),
            ("center_z", self.center_z),
            ("size", self.size),
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
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderCommand {
    Center([Coord; 2]),
    Set(f64),
    Add(f64),
    Get,
}
pub fn parse(args: &[&str]) -> Result<BorderCommand, CommandError> {
    let invalid =
        || CommandError::Usage("worldborder <center x z|set size [time]|add size [time]|get>");
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
        [mode @ ("set" | "add"), size] | [mode @ ("set" | "add"), size, "0"] => {
            let size = number(size)?;
            if *mode == "set" {
                if !(1.0..=MAX_SIZE).contains(&size) {
                    return Err(invalid());
                }
                Ok(BorderCommand::Set(size))
            } else {
                Ok(BorderCommand::Add(size))
            }
        }
        ["set" | "add", _, time] if time.parse::<u64>().is_ok_and(|n| n > 0) => Err(
            CommandError::Message("Timed world border transitions are not supported".into()),
        ),
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
        BorderCommand::Set(size) => border.size = size,
        BorderCommand::Add(delta) => border.size = (border.size + delta).clamp(1.0, MAX_SIZE),
    }
    format!("Set the world border to {} blocks wide", border.size)
}

#[cfg(test)]
#[path = "command_border_tests.rs"]
mod tests;
