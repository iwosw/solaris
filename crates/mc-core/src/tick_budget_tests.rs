use super::*;

fn charge(budget: &mut Budget, dimension: Dimension, consumer: Consumer, kind: Kind) -> bool {
    budget.try_charge(dimension, consumer, kind, 1)
}

#[test]
fn every_dimension_and_owner_spends_the_same_allowance() {
    let mut budget = Budget::default();
    budget.reset(7);
    let mut spent = 0;
    for dimension in Dimension::ALL {
        for consumer in [Consumer::Allocator, Consumer::Brain] {
            while charge(&mut budget, dimension, consumer, Kind::Decision) {
                spent += 1;
            }
            budget.finish(dimension, consumer);
        }
    }
    assert_eq!(spent, DECISIONS_PER_TICK);
    budget.reset(7);
    assert_eq!(budget.remaining(Kind::Decision), 0);
    budget.reset(8);
    assert_eq!(budget.remaining(Kind::Decision), DECISIONS_PER_TICK);
}

#[test]
fn refused_charges_are_atomic_and_node_budget_is_independent() {
    let mut budget = Budget::default();
    budget.reset(0);
    assert!(!budget.try_charge(
        Dimension::Overworld,
        Consumer::Allocator,
        Kind::Decision,
        usize::MAX,
    ));
    assert_eq!(budget.remaining(Kind::Decision), DECISIONS_PER_TICK);
    for dimension in Dimension::ALL {
        for consumer in [Consumer::Allocator, Consumer::Brain] {
            while charge(&mut budget, dimension, consumer, Kind::PathNode) {}
            budget.finish(dimension, consumer);
        }
    }
    assert_eq!(budget.remaining(Kind::PathNode), 0);
    assert!(charge(
        &mut budget,
        Dimension::Overworld,
        Consumer::Brain,
        Kind::Decision
    ));
    budget.reset(0);
    assert_eq!(budget.remaining(Kind::PathNode), 0);
    budget.reset(1);
    assert_eq!(budget.remaining(Kind::PathNode), PATH_NODES_PER_TICK);
}

#[test]
fn overworld_allocator_backlog_cannot_starve_nether_brains() {
    let mut budget = Budget::default();
    for tick in 0..8 {
        budget.reset(tick);
        for _ in 0..DECISIONS_PER_TICK * 8 {
            charge(
                &mut budget,
                Dimension::Overworld,
                Consumer::Allocator,
                Kind::Decision,
            );
        }
        assert!(
            charge(
                &mut budget,
                Dimension::Nether,
                Consumer::Brain,
                Kind::Decision
            ),
            "a later Nether brain must receive admission every tick"
        );
        budget.finish(Dimension::Overworld, Consumer::Allocator);
        budget.finish(Dimension::Nether, Consumer::Brain);
    }
}

#[test]
fn allocator_backlog_cannot_zero_brain_guarantees_in_the_same_dimension() {
    let mut budget = Budget::default();
    for tick in 0..8 {
        budget.reset(tick);
        for kind in [Kind::Decision, Kind::PathNode] {
            while charge(&mut budget, Dimension::Overworld, Consumer::Allocator, kind) {}
            assert!(charge(
                &mut budget,
                Dimension::Overworld,
                Consumer::Brain,
                kind
            ));
        }
    }
}

#[test]
fn demand_weights_dimension_shares_without_erasing_small_dimensions() {
    let mut budget = Budget::default();
    budget.reset(0);
    for _ in 0..1000 {
        charge(
            &mut budget,
            Dimension::Overworld,
            Consumer::Allocator,
            Kind::Decision,
        );
    }
    charge(
        &mut budget,
        Dimension::Nether,
        Consumer::Brain,
        Kind::Decision,
    );
    budget.reset(1);
    let share = |dimension: usize| {
        budget.shared[dimension][0]
            + budget.reserved[dimension]
                .iter()
                .map(|owner| owner[0])
                .sum::<usize>()
    };
    assert!(share(0) > share(1));
    assert!(share(1) >= 2);
    assert_eq!(share(2), 0);
    assert_eq!(budget.remaining(Kind::Decision), DECISIONS_PER_TICK);
}

#[test]
fn unused_guarantees_return_to_common_and_finish_is_idempotent() {
    let mut budget = Budget::default();
    budget.reset(0);
    while charge(
        &mut budget,
        Dimension::Overworld,
        Consumer::Allocator,
        Kind::Decision,
    ) {}
    assert!(!charge(
        &mut budget,
        Dimension::Overworld,
        Consumer::Allocator,
        Kind::Decision
    ));
    let remaining = budget.remaining(Kind::Decision);
    budget.finish(Dimension::Nether, Consumer::Allocator);
    budget.finish(Dimension::Nether, Consumer::Allocator);
    assert_eq!(budget.remaining(Kind::Decision), remaining);
    assert!(charge(
        &mut budget,
        Dimension::Overworld,
        Consumer::Allocator,
        Kind::Decision
    ));
    assert!(charge(
        &mut budget,
        Dimension::Nether,
        Consumer::Brain,
        Kind::Decision
    ));
}

#[test]
fn known_backlog_is_not_lost_at_first_denial_or_double_counted() {
    let mut budget = Budget::default();
    budget.reset(0);
    for _ in 0..2 {
        budget.note_demand(
            Dimension::Overworld,
            Consumer::Brain,
            Kind::Decision,
            10_000,
        );
    }
    assert!(charge(
        &mut budget,
        Dimension::Overworld,
        Consumer::Brain,
        Kind::Decision
    ));
    budget.note_demand(Dimension::Nether, Consumer::Brain, Kind::Decision, 10_000);
    budget.reset(1);
    assert_eq!(
        budget.reserved[0][Consumer::Brain.index()][0],
        budget.reserved[1][Consumer::Brain.index()][0]
    );
    assert_eq!(budget.shared[0][0], budget.shared[1][0]);
    assert!(
        budget.allowance(Dimension::Overworld, Consumer::Brain, Kind::Decision)
            < budget.remaining(Kind::Decision)
    );
}

#[test]
fn newly_populated_nether_receives_brain_admission_after_overworld_only_history() {
    let mut budget = Budget::default();
    for tick in 0..10 {
        budget.reset(tick);
        budget.note_demand(
            Dimension::Overworld,
            Consumer::Allocator,
            Kind::Decision,
            1_000_000,
        );
        while charge(
            &mut budget,
            Dimension::Overworld,
            Consumer::Allocator,
            Kind::Decision,
        ) {}
    }
    assert_eq!(
        budget.allowance(Dimension::Nether, Consumer::Brain, Kind::Decision),
        0
    );
    // A newly loaded Nether reports its REAL backlog even though this tick's
    // learned share is zero; it does not announce only admitted work.
    budget.note_demand(Dimension::Nether, Consumer::Brain, Kind::Decision, 100);
    budget.reset(10);
    while charge(
        &mut budget,
        Dimension::Overworld,
        Consumer::Allocator,
        Kind::Decision,
    ) {}
    assert!(charge(
        &mut budget,
        Dimension::Nether,
        Consumer::Brain,
        Kind::Decision
    ));
}
