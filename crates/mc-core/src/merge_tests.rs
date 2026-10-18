//! Merge rule tests (assignment §5): single-winner pickup, dual-attacker
//! damage, break-vs-place determinism. Sibling `*_tests.rs` per repo rule.

use crate::boundary::{ConnectionId, EntityId};
use crate::intent::{Footprint, Intent, IntentKind, footprints_disjoint};

fn intent(actor: i32, seq: u32, kind: IntentKind, footprint: Footprint) -> Intent {
    Intent {
        actor: EntityId(actor),
        seq,
        kind,
        footprint,
    }
}

/// Two claimants on one item: exactly one gets it (footprints overlap, merge
/// order decides, loser rejects whole).
#[test]
fn two_claimants_on_one_item_exactly_one_wins() {
    let item = EntityId(100);
    let a = intent(
        1,
        0,
        IntentKind::PickupItem { item, count: 64 },
        Footprint {
            entity_reads: vec![EntityId(1)],
            item_take: Some(item),
            ..Default::default()
        },
    );
    let b = intent(
        2,
        0,
        IntentKind::PickupItem { item, count: 64 },
        Footprint {
            entity_reads: vec![EntityId(2)],
            item_take: Some(item),
            ..Default::default()
        },
    );
    assert!(!footprints_disjoint(&a.footprint, &b.footprint));
    // Merge order is fixed: lower actor id first.
    let mut intents = vec![b.clone(), a.clone()];
    crate::intent::sort_intents(&mut intents);
    assert_eq!(intents[0].actor, EntityId(1));
    assert_eq!(intents[1].actor, EntityId(2));
    // Simulate merge: first validates, consumes the item; second rejects.
    let mut consumed = false;
    let mut winners = Vec::new();
    for intent in &intents {
        let footprint = &intent.footprint;
        let valid = footprint.item_take.is_none() || !consumed;
        if valid {
            consumed = true;
            winners.push(intent.actor);
        }
    }
    assert_eq!(winners, vec![EntityId(1)]);
}

/// Two attackers on one target both apply in stable order (damage sums).
#[test]
fn two_attackers_both_apply_in_stable_order() {
    let target = EntityId(50);
    let mk = |actor: i32| {
        intent(
            actor,
            0,
            IntentKind::DamageEntity {
                target,
                amount: 3.0,
                kind: crate::survival::DamageKind::Melee,
                source: None,
            },
            Footprint {
                entity_reads: vec![EntityId(actor)],
                entity_writes: vec![target],
                ..Default::default()
            },
        )
    };
    let (a, b) = (mk(2), mk(1));
    // Same target write: footprints overlap (serial merge), both validate
    // because entity writes do not invalidate each other.
    assert!(!footprints_disjoint(&a.footprint, &b.footprint));
    let mut intents = vec![a, b];
    crate::intent::sort_intents(&mut intents);
    assert_eq!(intents[0].actor, EntityId(1));
    assert_eq!(intents[1].actor, EntityId(2));
    let total: f32 = intents
        .iter()
        .map(|intent| match intent.kind {
            IntentKind::DamageEntity { amount, .. } => amount,
            _ => 0.0,
        })
        .sum();
    assert_eq!(total, 6.0);
}

/// Block break vs place on the same position resolves deterministically:
/// both footprints expect a precondition; only the first in merge order
/// validates, the loser rejects whole.
#[test]
fn break_vs_place_same_position_is_deterministic() {
    use crate::boundary::BlockPos;
    let pos = BlockPos { x: 1, y: 64, z: 1 };
    let break_intent = intent(
        1,
        0,
        IntentKind::BreakBlock { pos },
        Footprint {
            block_expect: vec![(pos, 7)],
            block_write: vec![(pos, 0)],
            ..Default::default()
        },
    );
    let place_intent = intent(
        2,
        0,
        IntentKind::PlaceBlock { pos, state: 9 },
        Footprint {
            block_expect: vec![(pos, 7)],
            block_write: vec![(pos, 9)],
            ..Default::default()
        },
    );
    assert!(!footprints_disjoint(
        &break_intent.footprint,
        &place_intent.footprint
    ));
    // Forward and reverse submission orders converge to the same winner.
    for order in [
        vec![break_intent.clone(), place_intent.clone()],
        vec![place_intent.clone(), break_intent.clone()],
    ] {
        let mut intents = order;
        crate::intent::sort_intents(&mut intents);
        assert_eq!(intents[0].actor, EntityId(1));
        // Simulate: first write wins the position, second sees a changed
        // block_expect and rejects.
        let mut written: Option<(BlockPos, u32)> = None;
        let mut applied = Vec::new();
        for intent in &intents {
            let expects_ok = intent
                .footprint
                .block_expect
                .iter()
                .all(|(pos, _)| written.is_none_or(|(wpos, _)| wpos != *pos));
            if expects_ok {
                for (wpos, wstate) in &intent.footprint.block_write {
                    written = Some((*wpos, *wstate));
                }
                applied.push(intent.actor);
            }
        }
        assert_eq!(applied, vec![EntityId(1)]);
    }
    let _ = ConnectionId(0);
}
