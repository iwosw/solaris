use super::*;

#[test]
fn core_action_generator_is_seeded_and_shrinkable() {
    let first = CoreActionGenerator::generate(0x51, 8);
    let second = CoreActionGenerator::generate(0x51, 8);
    let different = CoreActionGenerator::generate(0x52, 8);

    assert_eq!(first, second);
    assert_ne!(first, different);
    assert_eq!(shrink_action_prefix(&first, 3), &first[..3]);
    assert_eq!(shrink_action_prefix(&first, 99), &first[..]);
}

#[test]
fn observation_diff_reports_fact_mismatch_and_points_at_phase() {
    let mut expected = ObservationSet::new("vanilla", "break-dirt");
    expected.push(ObservationFact::PacketSeen { id: 0x23 });
    expected.push(ObservationFact::BlockState {
        x: 0,
        y: 64,
        z: 0,
        state_id: 0,
    });

    let mut actual = ObservationSet::new("solaris", "break-dirt");
    actual.push(ObservationFact::PacketSeen { id: 0x23 });
    actual.push(ObservationFact::BlockState {
        x: 0,
        y: 64,
        z: 0,
        state_id: 1,
    });

    let diff = diff_observations(&expected, &actual);
    assert_eq!(diff.phase, "break-dirt");
    assert_eq!(diff.missing_from_actual.len(), 1);
    assert_eq!(diff.unexpected_in_actual.len(), 1);
}

#[test]
fn sequence_normalization_preserves_order_and_multiplicity() {
    let mut left = ObservationSet::new("vanilla", "spawn");
    left.push(ObservationFact::PacketSeen { id: 2 });
    left.push(ObservationFact::PacketSeen { id: 1 });
    left.push(ObservationFact::PacketSeen { id: 2 });

    let normalized = left.clone().normalize_sequence();
    assert_eq!(normalized.facts(), left.facts());

    let mut right = ObservationSet::new("solaris", "spawn");
    right.push(ObservationFact::PacketSeen { id: 2 });
    right.push(ObservationFact::PacketSeen { id: 2 });
    right.push(ObservationFact::PacketSeen { id: 1 });

    let diff = diff_observations(&left, &right);
    assert_eq!(diff.missing_from_actual, left.facts()[1..]);
    assert_eq!(diff.unexpected_in_actual, right.facts()[1..]);
}

#[test]
fn set_normalization_discards_order_and_multiplicity() {
    let mut observations = ObservationSet::new("vanilla", "stable-world-state");
    observations.push(ObservationFact::PacketSeen { id: 2 });
    observations.push(ObservationFact::PacketSeen { id: 1 });
    observations.push(ObservationFact::PacketSeen { id: 2 });

    assert_eq!(
        observations.normalize_set().facts(),
        &[
            ObservationFact::PacketSeen { id: 1 },
            ObservationFact::PacketSeen { id: 2 }
        ]
    );
}
