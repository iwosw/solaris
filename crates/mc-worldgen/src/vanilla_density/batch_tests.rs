use super::*;

#[test]
fn batched_terrain_matches_scalar_bits_at_boundaries() {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    for seed in [712816, 5617830, 42] {
        let runtime = DensityRuntime::new(seed, &root).unwrap();
        let field = runtime.field("final_density").unwrap();
        for (x, z) in [
            (0, 0),
            (-17, -9),
            (-30_000_000, 29_999_984),
            (16_777_216, -16_777_216),
        ] {
            let mut batch = DensitySampler::for_chunk(&runtime, x, z);
            let mut scalar = DensitySampler::for_chunk(&runtime, x, z);
            for y in [-64, -57, -1, 0, 7, 8, 63, 64, 127, 255, 319] {
                for dz in 0..8 {
                    let positions = std::array::from_fn(|lane| [x + lane as i32, y, z + dz]);
                    let actual = batch.sample_terrain_batch(field, positions);
                    for (lane, [x, y, z]) in positions.into_iter().enumerate() {
                        let expected = scalar.sample_terrain(field, x, y, z);
                        assert_eq!(
                            actual[lane].to_bits(),
                            expected.to_bits(),
                            "seed={seed} ({x},{y},{z})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn batched_branches_preserve_zero_and_lazy_nan_semantics() {
    let runtime = DensityRuntime {
        nodes: vec![
            Node::Constant(-0.0),
            Node::Constant(f64::NAN),
            Node::Binary(Binary::Mul, 0, 1),
            Node::Gradient(0, 8, -1.0, 1.0),
            Node::Range(3, -2.0, 0.0, 2, 0),
            Node::Binary(Binary::Min, 4, 0),
            Node::Binary(Binary::Max, 4, 0),
            Node::Mapped(Mapped::Square, 3),
            Node::Range(3, -2.0, 0.0, 7, 0),
            Node::Range(3, 0.0, 2.0, 7, 0),
            Node::Binary(Binary::Add, 8, 9),
            Node::Binary(Binary::Add, 10, 7),
            Node::Marker(Marker::FlatCache, 3),
            Node::Binary(Binary::Add, 3, 12),
            Node::Binary(Binary::Add, 13, 3),
        ],
        noises: vec![],
        blended: vec![],
        fields: HashMap::new(),
        min_y: 0,
        height: 16,
        sea_level: 0,
        cell_width: 4,
        cell_height: 8,
    };
    let mut batch = DensitySampler::for_chunk(&runtime, 0, 0);
    let mut scalar = DensitySampler::for_chunk(&runtime, 0, 0);
    // 11 revisits a node with disjoint masks; 14 revisits it after a flat-cache
    // coordinate transform has populated that node under another generation.
    for field in [2, 4, 5, 6, 11, 14] {
        let positions = std::array::from_fn(|lane| [lane as i32, lane as i32, 0]);
        let actual = batch.sample_terrain_batch(field, positions);
        for (lane, [x, y, z]) in positions.into_iter().enumerate() {
            assert_eq!(
                actual[lane].to_bits(),
                scalar.sample_terrain(field, x, y, z).to_bits()
            );
        }
    }
}

#[test]
fn full_chunk_batch_matches_scalar_density_bits() {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    for (seed, min_x, min_z) in [(712816, -16, -16), (5617830, 0, 0), (42, 192, -448)] {
        let runtime = std::sync::Arc::new(DensityRuntime::new(seed, &root).unwrap());
        let generator =
            crate::vanilla_density::VanillaDensity::from_runtime(runtime.clone()).unwrap();
        let mut actual = vec![0.0; runtime.height as usize * 256];
        generator.fill_density_chunk(min_x, min_z, &mut actual);
        let mut scalar = DensitySampler::for_chunk(&runtime, min_x, min_z);
        let field = runtime.field("final_density").unwrap();
        for y in runtime.min_y..runtime.min_y + runtime.height {
            for z in 0..16 {
                for x in 0..16 {
                    let index = (y - runtime.min_y) as usize * 256 + z as usize * 16 + x as usize;
                    let expected = scalar.sample_terrain(field, min_x + x, y, min_z + z);
                    assert_eq!(
                        actual[index].to_bits(),
                        expected.to_bits(),
                        "seed={seed} ({x},{y},{z})"
                    );
                }
            }
        }
    }
}

#[test]
fn interpolation_cell_reuse_preserves_sparse_cross_cell_batches() {
    for (width, height) in [(1, 2), (4, 8), (8, 4)] {
        let runtime = DensityRuntime {
            nodes: vec![
                Node::Gradient(-64, 320, -1.0, 1.0),
                Node::Mapped(Mapped::Cube, 0),
                Node::Marker(Marker::Interpolated, 1),
            ],
            noises: vec![],
            blended: vec![],
            fields: HashMap::new(),
            min_y: -64,
            height: 384,
            sea_level: 63,
            cell_width: width,
            cell_height: height,
        };
        let mut batch = DensitySampler::for_chunk(&runtime, -16, -16);
        batch.batch_values.resize(runtime.nodes.len(), BatchMemo::default());
        let mut scalar = DensitySampler::for_chunk(&runtime, -16, -16);
        for mask in [1, 0b10101010, 0b11011011, 255] {
            for y in [-64, -57, -1, 0, 7, 8, 63, 319] {
                let positions = std::array::from_fn(|lane| Position {
                    x: -17 + lane as i32,
                    y: y + (lane / 3) as i32,
                    z: -9 + (lane / 2) as i32,
                    mode: Mode::Terrain,
                });
                let actual = batch.eval_new_batch(2, &positions, mask);
                for (lane, p) in positions.into_iter().enumerate() {
                    if mask & (1 << lane) != 0 {
                        assert_eq!(
                            actual[lane].to_bits(),
                            scalar.sample_terrain(2, p.x, p.y, p.z).to_bits(),
                            "width={width} height={height} mask={mask} lane={lane}",
                        );
                    }
                }
            }
        }
    }
}
