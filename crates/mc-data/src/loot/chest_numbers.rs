use super::ChestRng;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Number {
    Constant(f64),
    Uniform { min: f64, max: f64 },
    Binomial { n: u32, p: f64 },
}
impl Number {
    pub(super) fn compile(value: &Value, low: f64, high: f64) -> Result<Self, String> {
        let number = |value: &Value| {
            value
                .as_f64()
                .filter(|v| v.is_finite() && *v >= low && *v <= high)
                .ok_or_else(|| format!("number provider expects {low}..={high}, found {value}"))
        };
        if value.is_number() {
            return Ok(Self::Constant(number(value)?));
        }
        match value["type"].as_str() {
            Some("minecraft:constant") => Ok(Self::Constant(number(&value["value"])?)),
            Some("minecraft:uniform") => {
                let min = number(&value["min"])?;
                let max = number(&value["max"])?;
                if min > max {
                    return Err("unordered uniform provider".into());
                }
                Ok(Self::Uniform { min, max })
            }
            Some("minecraft:binomial") => {
                let n = number(&value["n"])?;
                let p = value["p"]
                    .as_f64()
                    .filter(|p| (0.0..=1.0).contains(p))
                    .ok_or("binomial probability must be in 0..=1")?;
                if n < 0.0 || n.fract() != 0.0 || low > 0.0 {
                    return Err(
                        "binomial trials must be nonnegative, integral and admit zero".into(),
                    );
                }
                Ok(Self::Binomial { n: n as u32, p })
            }
            other => Err(format!("unsupported number provider {other:?}")),
        }
    }
    pub(super) fn min(&self) -> f64 {
        match self {
            Self::Constant(v) => *v,
            Self::Uniform { min, .. } => *min,
            Self::Binomial { .. } => 0.0,
        }
    }
    pub(super) fn max(&self) -> f64 {
        match self {
            Self::Constant(v) => *v,
            Self::Uniform { max, .. } => *max,
            Self::Binomial { n, .. } => f64::from(*n),
        }
    }
    pub(super) fn int(&self, rng: &mut ChestRng) -> i32 {
        match self {
            Self::Constant(v) => v.floor() as i32,
            Self::Uniform { min, max } => {
                let low = min.floor() as i32;
                let high = max.floor() as i32;
                low + rng.next_bounded((i64::from(high) - i64::from(low) + 1) as u64) as i32
            }
            Self::Binomial { n, p } => (0..*n).filter(|_| rng.fraction() < *p).count() as i32,
        }
    }
    pub(super) fn float(&self, rng: &mut ChestRng) -> f64 {
        match self {
            Self::Constant(v) => *v,
            Self::Uniform { min, max } => min + rng.fraction() * (max - min),
            Self::Binomial { .. } => f64::from(self.int(rng)),
        }
    }
}
