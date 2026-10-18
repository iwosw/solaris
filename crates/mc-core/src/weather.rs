//! Tick-owned weather targets, retained client fades and durable world clock.
//!
//! The old core alternates clear/rain naturally; thunder is a command target.
//! It has no natural lightning scheduler. Keep those boundaries explicit.

use crate::boundary::PlayerChange;
use crate::commands::Weather;
use crate::config::CoreError;
use crate::tick::TickState;

const LEVEL_STEP_PER_TICK: f32 = 0.01;

/// Absolute clock/weather post-image. Float bits preserve mid-fade restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClimateImage {
    pub day_time: i64,
    pub weather_kind: u8,
    pub remaining_ticks: i64,
    pub rain_level_bits: u32,
    pub thunder_level_bits: u32,
    pub advance_time: bool,
    pub advance_weather: bool,
}

impl ClimateImage {
    pub(crate) fn capture(state: &TickState) -> Self {
        Self {
            day_time: state.day_time,
            weather_kind: match state.weather.0 {
                Weather::Clear => 0,
                Weather::Rain => 1,
                Weather::Thunder => 2,
            },
            remaining_ticks: state.weather.1,
            rain_level_bits: state.rain_level.to_bits(),
            thunder_level_bits: state.thunder_level.to_bits(),
            advance_time: crate::command_rules::boolean(state, "advance_time"),
            advance_weather: crate::command_rules::boolean(state, "advance_weather"),
        }
    }

    pub(crate) fn restore(self, state: &mut TickState) -> Result<(), CoreError> {
        let kind = match self.weather_kind {
            0 => Weather::Clear,
            1 => Weather::Rain,
            2 => Weather::Thunder,
            _ => return Err(CoreError::Storage("invalid persisted weather kind".into())),
        };
        let rain_level = f32::from_bits(self.rain_level_bits);
        let thunder_level = f32::from_bits(self.thunder_level_bits);
        if self.remaining_ticks < 0 || !valid_level(rain_level) || !valid_level(thunder_level) {
            return Err(CoreError::Storage("invalid persisted weather state".into()));
        }
        state.day_time = self.day_time;
        state.weather = (kind, self.remaining_ticks);
        state.rain_level = rain_level;
        state.thunder_level = thunder_level;
        state
            .gamerules
            .insert("advance_time".into(), self.advance_time.to_string());
        state
            .gamerules
            .insert("advance_weather".into(), self.advance_weather.to_string());
        state.journaled_climate = Some(self);
        Ok(())
    }
}

fn valid_level(level: f32) -> bool {
    level.is_finite() && (0.0..=1.0).contains(&level)
}

/// A viewer's semantic weather baseline, captured before commands run.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct View {
    kind: Weather,
    rain_level_bits: u32,
    thunder_level_bits: u32,
}

pub(crate) fn view(state: &TickState) -> View {
    View {
        kind: state.weather.0,
        rain_level_bits: state.rain_level.to_bits(),
        thunder_level_bits: state.thunder_level.to_bits(),
    }
}

/// Commands replace the target and duration, never the retained fade levels.
pub(crate) fn set_command(state: &mut TickState, kind: Weather, duration: Option<i64>) {
    state.weather = (
        kind,
        duration
            .unwrap_or_else(|| default_duration(state, kind))
            .max(1),
    );
}

/// Existing command/natural-cycle duration distributions, sampled without allocation.
pub(crate) fn default_duration(state: &TickState, weather: Weather) -> i64 {
    let (minimum, maximum) = match weather {
        Weather::Clear => (12_000, 180_000),
        Weather::Rain => (12_000, 24_000),
        Weather::Thunder => (3_600, 15_600),
    };
    let mut rng = crate::natural_spawn::Rng(
        state.world.seed() as u64 ^ state.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15),
    );
    minimum + rng.below((maximum - minimum + 1) as u64) as i64
}

/// Advance only the duration clock. A frozen cycle still permits fading.
pub(crate) fn advance_cycle(state: &mut TickState) {
    if !crate::command_rules::boolean(state, "advance_weather") {
        return;
    }
    if state.weather.1 > 0 {
        state.weather.1 -= 1;
        if state.weather.1 > 0 {
            return;
        }
        state.weather.0 = if state.weather.0 == Weather::Clear {
            Weather::Rain
        } else {
            Weather::Clear
        };
    }
    state.weather.1 = default_duration(state, state.weather.0);
}

/// Fade after commands, the cycle and sleep select this tick's final target.
/// The edge owns threshold state events and exact GameEvent packet ordering.
pub(crate) fn fade_and_publish(state: &mut TickState, previous: View) {
    state.rain_level = step_level(state.rain_level, state.weather.0 != Weather::Clear);
    state.thunder_level = step_level(state.thunder_level, state.weather.0 == Weather::Thunder);
    if view(state) == previous {
        return;
    }
    let raining = state.weather.0 != Weather::Clear;
    let rain_level = state.rain_level;
    let thunder_level = state.thunder_level;
    // Temporarily move the index to permit bounded queue pushes without
    // allocating a recipient copy. push_player only mutates queue state.
    let connections = std::mem::take(&mut state.connections);
    for connection in connections.all_connections() {
        state.push_player(
            connection,
            PlayerChange::WeatherChanged {
                raining,
                rain_level,
                thunder_level,
            },
        );
    }
    state.connections = connections;
}

fn step_level(level: f32, increasing: bool) -> f32 {
    if increasing {
        (level + LEVEL_STEP_PER_TICK).min(1.0)
    } else {
        (level - LEVEL_STEP_PER_TICK).max(0.0)
    }
}

#[cfg(test)]
#[path = "weather_tests.rs"]
mod tests;
