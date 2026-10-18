//! Derived local spawn potential, rebuilt with the native population census.
use crate::entities::{EntityKind, EntitySlot};
use crate::tick::TickState;
use mc_data::biomes::BiomeSpawnCost;

#[derive(Debug, Default)]
pub(crate) struct Charges(Vec<([f64; 3], f64)>);

impl Charges {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    fn add(&mut self, position: [f64; 3], charge: f64) {
        if charge > 0.0 {
            self.0.push((position, charge));
        }
    }

    pub(crate) fn allows(&self, position: [f64; 3], cost: BiomeSpawnCost) -> bool {
        if cost.charge == 0.0 {
            return true;
        }
        let mut energy = 0.0;
        for &(other, charge) in &self.0 {
            let distance_squared = (0..3)
                .map(|axis| (position[axis] - other[axis]).powi(2))
                .sum::<f64>();
            if distance_squared == 0.0 {
                return false;
            }
            energy += cost.charge * charge / distance_squared.sqrt();
            if energy > cost.energy_budget {
                return false;
            }
        }
        true
    }
}

fn cost_at(state: &TickState, name: &str, position: [f64; 3]) -> Option<BiomeSpawnCost> {
    let biome = state.world.biome_at(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    )?;
    state.registry.biome_spawns.spawn_cost(biome, name)
}

pub(crate) fn observe(state: &mut TickState, slot: EntitySlot) {
    let body = state.entities.body(slot);
    let EntityKind::Mob { type_id } = body.kind else {
        return;
    };
    let position = state.entities.pos(slot);
    if let Some(cost) = state
        .registry
        .blueprint(type_id)
        .and_then(|blueprint| cost_at(state, blueprint.name, position))
    {
        state
            .spawn_census
            .dimension_charges
            .add(position, cost.charge);
    }
}

pub(crate) fn admitted(state: &TickState, name: &str, position: [f64; 3]) -> bool {
    cost_at(state, name, position)
        .is_none_or(|cost| state.spawn_census.dimension_charges.allows(position, cost))
}
