//! Physical finite-volume grid. Partial outer cells retain their actual area.
use serde::{Deserialize, Serialize};

use super::definition::WorldDefinition;
use super::prepared::PrepareError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
    pub min_x_m: f64,
    pub min_z_m: f64,
    pub width_m: f64,
    pub depth_m: f64,
    pub step_m: f64,
}

impl Grid {
    pub fn from_definition(
        definition: &WorldDefinition,
        step_m: f64,
    ) -> Result<Self, PrepareError> {
        let d = definition.input();
        let grid = Self {
            columns: (((d.domain.max_x_exclusive - d.domain.min_x) as f64
                * d.horizontal_meters_per_block)
                / step_m)
                .ceil() as usize,
            rows: (((d.domain.max_z_exclusive - d.domain.min_z) as f64
                * d.horizontal_meters_per_block)
                / step_m)
                .ceil() as usize,
            min_x_m: d.domain.min_x as f64 * d.horizontal_meters_per_block,
            min_z_m: d.domain.min_z as f64 * d.horizontal_meters_per_block,
            width_m: (d.domain.max_x_exclusive - d.domain.min_x) as f64
                * d.horizontal_meters_per_block,
            depth_m: (d.domain.max_z_exclusive - d.domain.min_z) as f64
                * d.horizontal_meters_per_block,
            step_m,
        };
        grid.validate()?;
        Ok(grid)
    }

    pub fn validate(&self) -> Result<(), PrepareError> {
        if ![
            self.min_x_m,
            self.min_z_m,
            self.width_m,
            self.depth_m,
            self.step_m,
        ]
        .into_iter()
        .all(f64::is_finite)
            || self.width_m <= 0.0
            || self.depth_m <= 0.0
            || self.step_m <= 0.0
            || self.columns == 0
            || self.rows == 0
            || (self.width_m / self.step_m).ceil() as usize != self.columns
            || (self.depth_m / self.step_m).ceil() as usize != self.rows
            || self.columns.checked_mul(self.rows).is_none()
        {
            return Err(PrepareError::Invalid("invalid physical grid"));
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.columns * self.rows
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn center(&self, index: usize) -> (f64, f64) {
        let x = index % self.columns;
        let z = index / self.columns;
        (
            self.min_x_m + x as f64 * self.step_m + self.cell_width(x) * 0.5,
            self.min_z_m + z as f64 * self.step_m + self.cell_depth(z) * 0.5,
        )
    }
    fn cell_width(&self, x: usize) -> f64 {
        self.step_m.min(self.width_m - x as f64 * self.step_m)
    }
    fn cell_depth(&self, z: usize) -> f64 {
        self.step_m.min(self.depth_m - z as f64 * self.step_m)
    }
    pub fn area(&self, index: usize) -> f64 {
        self.cell_width(index % self.columns) * self.cell_depth(index / self.columns)
    }
    pub fn boundary(&self, index: usize) -> bool {
        let x = index % self.columns;
        let z = index / self.columns;
        x == 0 || z == 0 || x + 1 == self.columns || z + 1 == self.rows
    }
    pub fn neighbours(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        let x = index % self.columns;
        let z = index / self.columns;
        [
            x.checked_sub(1).map(|v| z * self.columns + v),
            (x + 1 < self.columns).then_some(index + 1),
            z.checked_sub(1).map(|v| v * self.columns + x),
            (z + 1 < self.rows).then_some(index + self.columns),
        ]
        .into_iter()
        .flatten()
    }
    pub fn index_at(&self, x_m: f64, z_m: f64) -> usize {
        let x = ((x_m - self.min_x_m) / self.step_m)
            .floor()
            .clamp(0.0, (self.columns - 1) as f64) as usize;
        let z = ((z_m - self.min_z_m) / self.step_m)
            .floor()
            .clamp(0.0, (self.rows - 1) as f64) as usize;
        z * self.columns + x
    }
}
