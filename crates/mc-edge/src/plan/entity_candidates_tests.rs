//! Fused candidate oracle: `collect` matches the canonical
//! `visible_entity_ids` + `entity_trackable` pass and fills spawn/boss buffers.
use super::*;
use crate::plan::entity_candidates::collect;
use mc_core::boundary::{ChangeSet, EntityId};

fn type_id_of(name: &str) -> u32 {
    mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}

/// Canonical pre-fusion pass: held-square ids, own-before-admission, trackable filter.
fn canonical_visible(
    snapshot: &WorldSnapshot,
    viewer: &ViewerState,
    view_distance: i32,
) -> Vec<EntityId> {
    let mut expected = Vec::new();
    scratch::visible_entity_ids(
        snapshot,
        entity_chunk(viewer.position),
        view_distance,
        Some(viewer),
        &mut expected,
    );
    expected.retain(|&id| {
        entity_trackable(
            snapshot.entities.get(id).expect("visible entity"),
            viewer,
            view_distance,
        )
    });
    expected
}

#[test]
fn fused_visible_matches_canonical_for_held_unheld_and_negative_chunks() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    for chunk in [(0, 0), (1, 0), (-1, -1), (2, 0)] {
        viewer.loaded_chunks.insert(chunk, 0);
    }
    let view_distance = 2;
    // Own row sits in unheld (0, 1): appended last by admission, not the walk.
    let rows = vec![
        entity(1, [8.0, 64.0, 24.0]),
        entity(10, [8.0, 64.0, 8.0]),     // held (0, 0)
        entity(11, [-8.0, 64.0, -8.0]),   // held negative (-1, -1)
        entity(12, [24.0, 64.0, 8.0]),    // held (1, 0)
        entity(13, [8.0, 64.0, 40.0]),    // unheld (0, 2)
        entity(14, [45.0, 64.0, 8.0]),    // held (2, 0) but out of tracking range
        entity(15, [-40.0, 64.0, -40.0]), // outside the vd square
    ];
    let out = output(vec![], rows, ChangeSet::default());
    let expected = canonical_visible(&out.snapshot, &viewer, view_distance);
    let mut scratch = PlanScratch::default();
    collect(&out.snapshot, &mut viewer, view_distance, &mut scratch, 100);
    assert_eq!(scratch.visible, expected);
    assert!(scratch.visible.contains(&EntityId(10)));
    assert!(scratch.visible.contains(&EntityId(11)));
    assert!(scratch.visible.contains(&EntityId(12)));
    assert!(!scratch.visible.contains(&EntityId(13)));
    assert!(!scratch.visible.contains(&EntityId(14)));
    assert!(!scratch.visible.contains(&EntityId(15)));
    assert_eq!(scratch.visible.last(), Some(&EntityId(1)));
}

#[test]
fn known_same_type_marks_seen_while_wrong_type_becomes_spawn_candidate() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let same = entity(10, [9.0, 64.0, 8.0]);
    let mut stale_row = entity(11, [10.0, 64.0, 8.0]);
    stale_row.type_id = 30;
    viewer
        .tracked_entities
        .insert(EntityId(10), TrackedEntity::new(&same, 0));
    viewer
        .tracked_entities
        .insert(EntityId(11), TrackedEntity::new(&stale_row, 0));
    let out = output(
        vec![],
        vec![
            entity(1, [8.0, 64.0, 8.0]),
            entity(10, [9.0, 64.0, 8.0]),
            entity(11, [10.0, 64.0, 8.0]),
            entity(12, [11.0, 64.0, 8.0]),
        ],
        ChangeSet::default(),
    );
    let mut scratch = PlanScratch::default();
    let before = viewer.plan_stamp;
    collect(&out.snapshot, &mut viewer, 4, &mut scratch, 100);
    assert_eq!(viewer.plan_stamp, before + 1);
    assert_eq!(
        viewer
            .tracked_entities
            .get(&EntityId(10))
            .expect("known row")
            .seen,
        viewer.plan_stamp
    );
    assert_ne!(
        viewer
            .tracked_entities
            .get(&EntityId(11))
            .expect("stale row")
            .seen,
        viewer.plan_stamp
    );
    let spawn_ids: Vec<_> = scratch.spawns.iter().map(|(id, _)| *id).collect();
    assert!(!spawn_ids.contains(&EntityId(10)));
    assert!(spawn_ids.contains(&EntityId(11)));
    assert!(spawn_ids.contains(&EntityId(12)));
    assert!(!spawn_ids.contains(&EntityId(1)));
    let dist12 = scratch
        .spawns
        .iter()
        .find(|(id, _)| *id == EntityId(12))
        .expect("fresh row")
        .1;
    assert_eq!(dist12, 9.0);
}

#[test]
fn zero_spawn_credit_keeps_visible_but_leaves_spawns_empty() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let out = output(
        vec![],
        vec![entity(1, [8.0, 64.0, 8.0]), entity(10, [9.0, 64.0, 8.0])],
        ChangeSet::default(),
    );
    let mut scratch = PlanScratch::default();
    collect(&out.snapshot, &mut viewer, 4, &mut scratch, 0);
    assert_eq!(scratch.visible, vec![EntityId(1), EntityId(10)]);
    assert!(scratch.spawns.is_empty());
    assert_eq!(scratch.spawns.capacity(), 0);
}

#[test]
fn type_replacement_waits_when_removal_consumes_the_last_credit() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let current = entity(10, [9.0, 64.0, 8.0]);
    let mut previous = current.clone();
    previous.type_id = type_id_of("minecraft:cow");
    viewer
        .tracked_entities
        .insert(current.id, TrackedEntity::new(&previous, 0));
    let out = output(vec![], vec![current.clone()], ChangeSet::default());
    let mut scratch = PlanScratch::default();
    let mut packets = Vec::new();
    let mut credit = 1;
    plan_entities(
        &out.snapshot,
        &mut viewer,
        4,
        &mut scratch,
        &TickPlanIndex::new(&out),
        &mut packets,
        &mut credit,
    );
    assert_eq!(credit, 0);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].id, RemoveEntities::ID);
    assert!(!viewer.tracked_entities.contains_key(&current.id));
    assert!(scratch.spawns.is_empty());

    packets.clear();
    credit = 16;
    let index = TickPlanIndex::with_previous(&out, viewer.previous_entities());
    plan_entities(
        &out.snapshot,
        &mut viewer,
        4,
        &mut scratch,
        &index,
        &mut packets,
        &mut credit,
    );
    assert!(packets.iter().any(|packet| packet.id == AddEntity::ID));
    assert_eq!(
        viewer.tracked_entities[&current.id].type_id,
        current.type_id
    );
}

#[test]
fn bosses_collect_only_wither_and_dragon_without_health_filter() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let mut wither = entity(10, [9.0, 64.0, 8.0]);
    wither.type_id = type_id_of("minecraft:wither");
    let mut dragon = entity(11, [10.0, 64.0, 8.0]);
    dragon.type_id = type_id_of("minecraft:ender_dragon");
    let mob = entity(12, [11.0, 64.0, 8.0]);
    let mut far_wither = entity(13, [9.0, 64.0, 8.0]);
    far_wither.type_id = type_id_of("minecraft:wither");
    far_wither.position = [9.0, 64.0, 200.0]; // unheld chunk: never walked
    let out = output(
        vec![],
        vec![entity(1, [8.0, 64.0, 8.0]), wither, dragon, mob, far_wither],
        ChangeSet::default(),
    );
    let mut scratch = PlanScratch::default();
    collect(&out.snapshot, &mut viewer, 4, &mut scratch, 100);
    // Default appearance health is 0.0: no health filter here, BossBaseline owns it.
    assert_eq!(scratch.bosses, vec![EntityId(10), EntityId(11)]);
}
