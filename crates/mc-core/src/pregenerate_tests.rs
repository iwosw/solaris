//! Offline pregeneration: rectangle coverage, resumability, lease refusal.

use super::*;

fn world_dir(slug: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{slug}-{}", uuid::Uuid::new_v4()))
}

fn config(dir: &std::path::Path, seed: i64) -> CoreConfig {
    let mut config = CoreConfig::new(dir.to_path_buf(), seed);
    config.checkpoint_every_ticks = u64::MAX;
    config
}

fn fresh_world(seed: i64) -> (std::path::PathBuf, CoreConfig) {
    let dir = world_dir("mc-core-pregenerate");
    let config = config(&dir, seed);
    (dir, config)
}

fn stored_positions(config: &CoreConfig) -> HashSet<ChunkPos> {
    let store = NativeStore::open(
        &config.world_dir.join("native/state.redb"),
        crate::native_checkpoint::native_identity(config),
    )
    .expect("store opens after pregeneration");
    let mut positions = HashSet::new();
    store
        .visit_chunk_positions(|dimension, pos| {
            if dimension == crate::native_checkpoint::dimension_id(Dimension::Overworld) {
                positions.insert(pos);
            }
            Ok(())
        })
        .unwrap();
    positions
}

fn no_progress(_: &PregenerateProgress) {}

#[test]
fn pregenerate_stores_the_rectangle_then_a_rerun_skips_every_stored_chunk() {
    let (dir, config) = fresh_world(42);
    let request = PregenerateRequest {
        radius: 32,
        center: Some([0, 0]),
    };
    let report = pregenerate(&config, &request, &mut no_progress).expect("pregenerate");
    assert_eq!(report.center, [0, 0]);
    assert_eq!(report.radius, 32);
    assert_eq!(report.chunk_min, [-2, -2]);
    assert_eq!(report.chunk_max, [2, 2]);
    assert_eq!(report.chunks, 25);
    assert_eq!(
        report.generated + report.already_stored,
        report.chunks,
        "every rectangle chunk is either generated now or already stored"
    );
    assert!(report.generated > 0, "a fresh world stores new chunks");
    let stored = stored_positions(&config);
    for z in -2..=2 {
        for x in -2..=2 {
            assert!(
                stored.contains(&ChunkPos { x, z }),
                "chunk ({x},{z}) must be persisted"
            );
        }
    }

    // Rerun: the same rectangle is fully covered, so nothing regenerates.
    let rerun = pregenerate(&config, &request, &mut no_progress).expect("rerun");
    assert_eq!(rerun.already_stored, 25);
    assert_eq!(rerun.generated, 0, "rerun must skip existing chunks");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn pregenerate_resumes_a_partial_rectangle_instead_of_regenerating_it() {
    let (dir, config) = fresh_world(7);
    let inner = PregenerateRequest {
        radius: 16,
        center: None,
    };
    let outer = PregenerateRequest {
        radius: 32,
        center: inner.center,
    };
    let first = pregenerate(&config, &inner, &mut no_progress).expect("inner");
    assert_eq!(first.chunks, 9);
    assert!(first.already_stored + first.generated == 9);

    let grown = pregenerate(&config, &outer, &mut no_progress).expect("grown");
    assert_eq!(grown.chunks, 25);
    assert_eq!(
        grown.already_stored, first.chunks,
        "the inner rectangle is reused, not regenerated"
    );
    assert_eq!(grown.generated, 25 - first.chunks);
    assert!(stored_positions(&config).len() >= 25);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn pregenerate_reports_progress_until_the_last_chunk() {
    let (dir, config) = fresh_world(11);
    let mut samples = Vec::new();
    let report = pregenerate(
        &config,
        &PregenerateRequest {
            radius: 24,
            center: Some([100, -100]),
        },
        &mut |progress| samples.push(*progress),
    )
    .unwrap();
    assert!(!samples.is_empty());
    let last = *samples.last().expect("final sample");
    assert_eq!(last.chunks, report.chunks);
    assert_eq!(last.generated, report.generated);
    assert_eq!(last.already_stored, report.already_stored);
    assert_eq!(report.chunk_min, [4, -8]);
    assert_eq!(report.chunk_max, [7, -5]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn pregenerate_refuses_a_zero_radius_without_creating_the_world() {
    let dir = world_dir("mc-core-pregenerate-zero");
    let config = config(&dir, 3);
    let error = pregenerate(&config, &PregenerateRequest::new(0), &mut no_progress)
        .expect_err("zero radius")
        .to_string();
    assert!(error.contains("at least one block"), "{error}");
    assert!(
        !config.world_dir.exists(),
        "refusal happens before any open"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn pregenerate_refuses_a_world_that_a_running_server_holds() {
    let (dir, config) = fresh_world(42);
    Core::open(config.clone()).expect("fresh world").shutdown();
    let world = std::fs::File::open(&config.world_dir).unwrap();
    world.try_lock().expect("test holds the world lease");
    let error = pregenerate(&config, &PregenerateRequest::new(16), &mut no_progress)
        .expect_err("leased world")
        .to_string();
    world.unlock().unwrap();
    assert!(error.contains("leased"), "{error}");
    std::fs::remove_dir_all(dir).ok();
}
