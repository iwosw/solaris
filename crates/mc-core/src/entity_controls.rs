//! Vanilla common entity flags retained in both native records and WAL images.
use crate::config::CoreError;
use mc_nbt::Tag;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    pub no_ai: bool,
    pub invulnerable: bool,
    pub fall_distance: f64,
}
impl Controls {
    /// Journal dirty comparison of a baseline (`self`) with the live row.
    /// `fall_distance` is a transient mid-air accumulator (see `mob_fall`):
    /// growth rides the next image emitted for another reason and
    /// checkpoints, but never forces one. The reset to zero on landing is
    /// journaled so replay cannot keep a stale non-zero distance.
    pub(crate) fn journal_eq(&self, live: &Self) -> bool {
        self.no_ai == live.no_ai
            && self.invulnerable == live.invulnerable
            && !(live.fall_distance == 0.0 && self.fall_distance != 0.0)
    }

    pub(crate) fn from_fields(fields: &[(String, Tag)]) -> Result<Self, CoreError> {
        let mut state = Self::default();
        for name in ["NoAI", "Invulnerable", "FallDistance"] {
            let mut found = fields.iter().filter(|(key, _)| key == name);
            let Some((_, value)) = found.next() else {
                continue;
            };
            if found.next().is_some() {
                return Err(CoreError::Storage(format!("{name}: duplicate field")));
            }
            match (name, value) {
                ("NoAI", Tag::Byte(value)) => state.no_ai = *value != 0,
                ("Invulnerable", Tag::Byte(value)) => state.invulnerable = *value != 0,
                ("FallDistance", Tag::Float(value)) if value.is_finite() => {
                    state.fall_distance = f64::from(*value)
                }
                ("FallDistance", _) => {
                    return Err(CoreError::Storage(
                        "FallDistance: expected finite float".into(),
                    ));
                }
                _ => return Err(CoreError::Storage(format!("{name}: expected byte"))),
            }
        }
        Ok(state)
    }
    pub(crate) fn to_fields(self) -> Vec<(String, Tag)> {
        vec![
            ("NoAI".into(), Tag::Byte(i8::from(self.no_ai))),
            (
                "Invulnerable".into(),
                Tag::Byte(i8::from(self.invulnerable)),
            ),
            ("FallDistance".into(), Tag::Float(self.fall_distance as f32)),
        ]
    }
}
