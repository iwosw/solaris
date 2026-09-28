use super::{REGION_SIZE_CHUNKS, RegionEpoch, RegionKey, RegionOwnership, RegionOwnershipError};
use crate::Vec3;

#[test]
fn region_key_uses_euclidean_chunk_boundaries() {
    assert_eq!(REGION_SIZE_CHUNKS, 8);
    assert_eq!(RegionKey::from_chunk(0, 0), RegionKey::new(0, 0));
    assert_eq!(RegionKey::from_chunk(7, 7), RegionKey::new(0, 0));
    assert_eq!(RegionKey::from_chunk(8, 8), RegionKey::new(1, 1));
    assert_eq!(RegionKey::from_chunk(-1, -1), RegionKey::new(-1, -1));
    assert_eq!(RegionKey::from_chunk(-8, -8), RegionKey::new(-1, -1));
    assert_eq!(RegionKey::from_chunk(-9, -9), RegionKey::new(-2, -2));

    assert_eq!(
        RegionKey::from_position(Vec3::new(127.999, 64.0, 127.999)),
        Some(RegionKey::new(0, 0))
    );
    assert_eq!(
        RegionKey::from_position(Vec3::new(128.0, 64.0, 128.0)),
        Some(RegionKey::new(1, 1))
    );
    assert_eq!(
        RegionKey::from_position(Vec3::new(-128.0, 64.0, -128.0)),
        Some(RegionKey::new(-1, -1))
    );
    assert_eq!(
        RegionKey::from_position(Vec3::new(-128.001, 64.0, -128.001)),
        Some(RegionKey::new(-2, -2))
    );
    assert_eq!(
        RegionKey::from_position(Vec3::new(f64::NAN, 64.0, 0.0)),
        None
    );
}

#[test]
fn region_reassignment_is_fenced_by_phase_and_epoch() {
    let key = RegionKey::new(2, -3);
    let mut ownership = RegionOwnership::new();
    let first = ownership.assign(key, 0).expect("initial owner");
    assert_eq!(first.epoch, RegionEpoch::INITIAL);
    assert!(ownership.validate(first));

    let phase = ownership.begin_phase().expect("begin phase");
    assert_eq!(
        ownership.reassign(first, 1),
        Err(RegionOwnershipError::PhaseActive)
    );
    ownership
        .acknowledge_lane(phase, 0)
        .expect("lane 0 completes phase");
    ownership.finish_phase(phase).expect("finish phase");

    let second = ownership.reassign(first, 1).expect("next owner");
    assert_eq!(second.key, key);
    assert_eq!(second.lane, 1);
    assert_eq!(second.epoch, first.epoch.next().expect("next epoch"));
    assert!(!ownership.validate(first));
    assert!(ownership.validate(second));
    assert_eq!(
        ownership.reassign(first, 2),
        Err(RegionOwnershipError::StaleEpoch)
    );
}

#[test]
fn region_phase_transitions_reject_duplicate_events() {
    let mut ownership = RegionOwnership::new();

    let first = ownership.begin_phase().expect("first begin");
    assert_eq!(
        ownership.begin_phase(),
        Err(RegionOwnershipError::PhaseActive)
    );
    ownership.finish_phase(first).expect("first finish");
    assert_eq!(
        ownership.finish_phase(first),
        Err(RegionOwnershipError::PhaseInactive)
    );

    let lease = ownership
        .assign(RegionKey::new(0, 0), 0)
        .expect("owner between phases");
    let second = ownership.begin_phase().expect("second begin");
    assert_eq!(
        ownership.finish_phase(first),
        Err(RegionOwnershipError::StalePhase)
    );
    assert_eq!(
        ownership.reassign(lease, 1),
        Err(RegionOwnershipError::PhaseActive)
    );
    assert_eq!(
        ownership.finish_phase(second),
        Err(RegionOwnershipError::PhaseIncomplete)
    );
    ownership
        .acknowledge_lane(second, 0)
        .expect("lane 0 completes second phase");
    assert_eq!(
        ownership.acknowledge_lane(second, 0),
        Err(RegionOwnershipError::DuplicateLaneCompletion)
    );
    ownership.finish_phase(second).expect("second finish");
}

#[test]
fn regional_phase_waits_only_for_selected_lanes() {
    let mut ownership = RegionOwnership::new();
    ownership
        .assign(RegionKey::new(0, 0), 0)
        .expect("west owner");
    ownership
        .assign(RegionKey::new(1, 0), 1)
        .expect("east owner");

    let phase = ownership
        .begin_phase_for_lanes(std::collections::BTreeSet::from([0]))
        .expect("west-only phase");

    assert_eq!(
        ownership.validate_lane(phase, 1),
        Err(RegionOwnershipError::LaneCompleted)
    );
    ownership
        .acknowledge_lane(phase, 0)
        .expect("selected lane completes");
    ownership
        .finish_phase(phase)
        .expect("idle east lane is not part of the barrier");
}

#[test]
fn region_leases_are_exposed_in_key_order() {
    let mut ownership = RegionOwnership::new();
    let east = ownership
        .assign(RegionKey::new(3, 0), 0)
        .expect("east owner");
    let west = ownership
        .assign(RegionKey::new(-2, 0), 1)
        .expect("west owner");

    assert_eq!(ownership.lease(east.key), Some(east));
    assert_eq!(ownership.lease(RegionKey::new(0, 0)), None);
    assert_eq!(ownership.leases().collect::<Vec<_>>(), vec![west, east]);
}
