//! Raw router samples along a column. Only proven Y-free subgraphs may reuse
//! another height's value; marker wrappers keep raw (transparent) semantics.
use super::*;

pub(crate) fn raw_y_free(runtime: &DensityRuntime) -> Vec<bool> {
    let mut memo = HashMap::new();
    let mut stack = Vec::new();
    (0..runtime.nodes.len())
        .map(|id| y_free_inner(runtime, id, &mut memo, &mut stack))
        .collect()
}

pub(crate) struct RawColumnSampler<'a> {
    runtime: &'a DensityRuntime,
    y_free: &'a [bool],
    values: Vec<Option<Cached>>,
}

impl<'a> RawColumnSampler<'a> {
    pub(crate) fn new(runtime: &'a DensityRuntime, y_free: &'a [bool]) -> Self {
        Self {
            runtime,
            y_free,
            values: vec![None; runtime.nodes.len()],
        }
    }

    pub(crate) fn sample_fields<const N: usize>(
        &mut self,
        fields: [Id; N],
        x: i32,
        y: i32,
        z: i32,
    ) -> [f32; N] {
        let position = Position {
            x,
            y,
            z,
            mode: Mode::Raw,
        };
        fields.map(|field| eval(self.runtime, self, field, position))
    }
}

impl Cache for RawColumnSampler<'_> {
    fn get(&self, id: Id, position: Position) -> Option<f32> {
        self.values[id]
            .filter(|cached| {
                cached.position.x == position.x
                    && cached.position.z == position.z
                    && cached.position.mode == position.mode
                    && (cached.position.y == position.y || self.y_free[id])
            })
            .map(|cached| cached.value)
    }

    fn put(&mut self, id: Id, position: Position, value: f32) {
        self.values[id] = Some(Cached { position, value });
    }

    fn cell(&self, _: Id, _: i32, _: i32, _: i32) -> Option<[f32; 8]> {
        None
    }

    fn put_cell(&mut self, _: Id, _: Cell) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vanilla_climate::noise::NoiseFactory;

    #[test]
    fn raw_column_cache_preserves_bits_and_only_reuses_y_free_nodes() {
        let runtime = DensityRuntime {
            nodes: vec![
                Node::Noise(0, 0.25, 0.0),
                Node::Gradient(-64, 320, -1.0, 1.0),
                Node::Binary(Binary::Add, 0, 1),
                Node::Marker(Marker::FlatCache, 2),
                Node::Noise(0, 0.25, 0.5),
                Node::Shift(0, Shift::A),
                Node::Shift(0, Shift::All),
                Node::Marker(Marker::Cache2D, 1),
            ],
            noises: vec![NoiseFactory::new(81).normal("minecraft:temperature", -3, &[1.0])],
            blended: vec![],
            fields: HashMap::new(),
            min_y: -64,
            height: 384,
            sea_level: 63,
            cell_width: 4,
            cell_height: 8,
        };
        let free = raw_y_free(&runtime);
        assert_eq!(free, [true, false, false, false, false, true, false, false]);
        let mut sampler = RawColumnSampler::new(&runtime, &free);
        let fields = std::array::from_fn::<_, 8, _>(|id| id);
        for (x, z) in [(-16, -32), (0, 0), (4, -4), (-16, -32)] {
            for y in [-64, -1, 0, 1, 64, 319, -64] {
                let expected = direct_fields(&runtime, fields, x, y, z);
                let actual = sampler.sample_fields(fields, x, y, z);
                assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
                let next_height = Position {
                    x,
                    y: y + 1,
                    z,
                    mode: Mode::Raw,
                };
                assert!(sampler.get(0, next_height).is_some());
                assert!(sampler.get(1, next_height).is_none());
                assert!(sampler.get(3, next_height).is_none());
                assert!(
                    sampler
                        .get(
                            0,
                            Position {
                                x: x + 4,
                                ..next_height
                            }
                        )
                        .is_none()
                );
            }
        }
    }
}
