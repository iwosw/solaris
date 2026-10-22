//! Offline prune/compact: rectangle arithmetic, dry run, regeneration, lease refusal.

use super::*;
use crate::pregenerate::{PregenerateProgress, PregenerateRequest, pregenerate};

fn world_dir(slug: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{slug}-{}", uuid::Uuid::new_v4()))
}

fn config(dir: &std::path::Path, seed: i64) -> CoreConfig {
    let mut config = CoreConfig::new(dir.to_path_buf(), seed);
    config.checkpoint_every_ticks = u64::MAX;
    config
}

fn no_progress(_: &PregenerateProgress) {}

fn prune_request(keep_radius: u32, dry_run: bool) -> PruneRequest {
    PruneRequest {
        keep_radius,
        center: Some([0, 0]),
        max_inhabited_ticks: 0,
        dry_run,
    }
}

#[test]
fn rectangle_covers_the_radius_in_chunks_including_negative_blocks() {
    assert_eq!(rectangle([0, 0], 16), ([-1, -1], [1, 1]));
    assert_eq!(rectangle([-1024, 2048], 256), ([-80, 112], [-48, 144]));
    assert_eq!(rectangle([5, -5], 1), ([0, -1], [0, -1]));
}

#[test]
fn prune_forgets_far_pregenerated_chunks_and_they_regenerate_on_demand() {
    let dir = world_dir("mc-core-prune");
    let config = config(&dir, 42);
    let request = PregenerateRequest::new(64);
    let first = pregenerate(&config, &request, &mut no_progress).expect("pregenerate");
    assert_eq!(first.chunks, 81);

    let dry = prune_world(&config, &prune_request(16, true)).expect("dry run");
    assert_eq!(dry.keep_chunk_min, [-1, -1]);
    assert_eq!(dry.keep_chunk_max, [1, 1]);
    assert_eq!(dry.chunks.examined, 81);
    assert_eq!(dry.chunks.pruned, 72);
    assert_eq!(dry.chunks.kept_inside_rectangle, 9);
    assert_eq!(
        dry.bytes_before, dry.bytes_after,
        "a dry run writes nothing"
    );

    let report = prune_world(&config, &prune_request(16, false)).expect("prune");
    assert_eq!(report.chunks, dry.chunks);
    assert!(
        report.bytes_after < report.bytes_before,
        "{} -> {}",
        report.bytes_before,
        report.bytes_after
    );

    // The world still opens, keeps the inner chunks and regenerates the rest.
    let rerun = pregenerate(&config, &request, &mut no_progress).expect("rerun");
    assert_eq!(rerun.already_stored, 9);
    assert_eq!(rerun.generated, 72);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn compact_world_keeps_every_chunk_and_never_grows_the_file() {
    let dir = world_dir("mc-core-compact");
    let config = config(&dir, 7);
    let request = PregenerateRequest::new(32);
    let first = pregenerate(&config, &request, &mut no_progress).expect("pregenerate");
    let report = compact_world(&config).expect("compact");
    assert_eq!(report.rewritten_rows, 0, "new rows are already packed");
    assert!(report.bytes_after <= report.bytes_before);
    let rerun = pregenerate(&config, &request, &mut no_progress).expect("rerun");
    assert_eq!(rerun.already_stored, first.chunks);
    assert_eq!(rerun.generated, 0);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn maintenance_refuses_a_missing_world_and_a_leased_one() {
    let missing = world_dir("mc-core-prune-missing");
    let error = prune_world(&config(&missing, 1), &prune_request(16, true))
        .expect_err("missing world")
        .to_string();
    assert!(error.contains("no native checkpoint"), "{error}");
    assert!(!missing.exists(), "must not create the world");
    assert!(compact_world(&config(&missing, 1)).is_err());

    let dir = world_dir("mc-core-prune-leased");
    let config = config(&dir, 3);
    Core::open(config.clone()).expect("fresh world").shutdown();
    let world = std::fs::File::open(&config.world_dir).unwrap();
    world.try_lock().expect("test holds the world lease");
    let error = compact_world(&config).expect_err("leased").to_string();
    assert!(error.contains("already leased"), "{error}");
    world.unlock().unwrap();
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn prune_refuses_a_negative_inhabited_limit() {
    let mut request = prune_request(16, true);
    request.max_inhabited_ticks = -1;
    assert!(prune_world(&config(&world_dir("mc-core-prune-neg"), 1), &request).is_err());
}
