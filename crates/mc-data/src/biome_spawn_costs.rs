//! Exact vanilla charge/energy-budget facts shared by full and embedded biomes.
use super::{BiomeDataError, Identifier};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiomeSpawnCost {
    pub charge: f64,
    pub energy_budget: f64,
}

pub(super) type Costs = BTreeMap<Identifier, BTreeMap<Identifier, BiomeSpawnCost>>;

pub(super) fn parse(
    biome: &Identifier,
    raw: BTreeMap<String, BiomeSpawnCost>,
) -> Result<BTreeMap<Identifier, BiomeSpawnCost>, BiomeDataError> {
    raw.into_iter()
        .map(|(name, cost)| {
            let entity = Identifier::parse(&name)
                .map_err(|_| BiomeDataError::InvalidIdentifier(name.clone()))?;
            if !cost.charge.is_finite()
                || cost.charge < 0.0
                || !cost.energy_budget.is_finite()
                || cost.energy_budget < 0.0
            {
                return Err(BiomeDataError::InvalidSpawnCost {
                    biome: biome.clone(),
                    entity,
                    charge: cost.charge,
                    energy_budget: cost.energy_budget,
                });
            }
            Ok((entity, cost))
        })
        .collect()
}

pub(super) fn embedded() -> Costs {
    let raw: BTreeMap<String, BTreeMap<String, BiomeSpawnCost>> =
        serde_json::from_str(include_str!("../data/required_biome_spawn_costs.json"))
            .expect("embedded biome spawn costs decode");
    raw.into_iter()
        .map(|(name, costs)| {
            let biome = Identifier::parse(name).expect("embedded cost biome id");
            let costs = parse(&biome, costs).expect("embedded biome spawn costs are valid");
            (biome, costs)
        })
        .collect()
}
