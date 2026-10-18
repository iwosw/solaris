//! Generic terrain-generation contributions from component packages.
use mc_script::PluginWorldgenOreProfile;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskWorldgen {
    #[serde(default)]
    pub ore_profile: Option<DiskWorldgenOreProfile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskWorldgenOreProfile {
    RealisticDeposits,
}

#[derive(Debug, Clone, Default)]
pub struct PackageWorldgen {
    ore_profile: Option<PluginWorldgenOreProfile>,
}

impl PackageWorldgen {
    #[must_use]
    pub const fn ore_profile(&self) -> Option<PluginWorldgenOreProfile> {
        self.ore_profile
    }
}

pub fn materialize_worldgen(worldgen: Option<DiskWorldgen>) -> Result<PackageWorldgen, String> {
    let Some(worldgen) = worldgen else {
        return Ok(PackageWorldgen::default());
    };
    let profile = worldgen
        .ore_profile
        .ok_or_else(|| "worldgen must declare ore_profile".to_owned())?;
    Ok(PackageWorldgen {
        ore_profile: Some(match profile {
            DiskWorldgenOreProfile::RealisticDeposits => {
                PluginWorldgenOreProfile::RealisticDeposits
            }
        }),
    })
}
