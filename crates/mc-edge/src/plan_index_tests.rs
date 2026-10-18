//! S1/S3 packet and scratch-capacity invariants.
use super::*;

#[test]
fn sorted_square_query_matches_full_scan_without_changing_entity_order() {
    let mut rows = Vec::new();
    for x in [-50, -4, -1, 0, 1, 4, 50] {
        for z in [-50, -4, -1, 0, 1, 4, 50] {
            rows.push(entity(
                rows.len() as i32 + 10,
                [f64::from(x * 16), 64.0, f64::from(z * 16)],
            ));
        }
    }
    let snapshot = WorldSnapshot {
        entities: rows.into(),
        ..Default::default()
    };
    let mut visible = Vec::new();
    for center in [(-4, -4), (0, 0), (4, 4), (50, 50)] {
        for radius in [0, 1, 4, 8] {
            let expected: Vec<_> = snapshot
                .entities
                .iter()
                .filter(|view| {
                    let (x, z) = entity_chunk(view.position);
                    (x - center.0).abs() <= radius && (z - center.1).abs() <= radius
                })
                .map(|view| view.id)
                .collect();
            scratch::visible_entity_ids(&snapshot, center, radius, None, &mut visible);
            assert_eq!(visible, expected);
        }
    }
}

#[test]
fn indexed_one_shots_keep_commit_order_including_moved_and_removed_subjects() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let first = entity(10, [8.0, 64.0, 8.0]);
    let second = entity(11, [9.0, 64.0, 8.0]);
    plan_viewer(
        &output(
            vec![],
            vec![first.clone(), second.clone()],
            ChangeSet::default(),
        ),
        &mut viewer,
        4,
        &cache,
    );
    let mut moved = first;
    moved.position[0] = 8000.0;
    let output = output(
        vec![],
        vec![moved],
        ChangeSet {
            explosions: [48.0, -24.0, 8000.0, 16.0]
                .into_iter()
                .map(|x| mc_core::boundary::ExplosionEvent {
                    center: [x, 64.0, 8.0],
                    radius: 4.0,
                    blocks: 0,
                    knockback: vec![],
                })
                .collect(),
            entity_events: vec![
                BoundaryEvent::Status {
                    entity: EntityId(10),
                    event: 2,
                },
                BoundaryEvent::Death {
                    entity: EntityId(11),
                },
                BoundaryEvent::Status {
                    entity: EntityId(1),
                    event: 3,
                },
                BoundaryEvent::Status {
                    entity: EntityId(999),
                    event: 4,
                },
            ],
            ..Default::default()
        },
    );
    let mut expected = Vec::new();
    for explosion in output
        .changes
        .explosions
        .iter()
        .filter(|e| e.center[0] != 8000.0)
    {
        push_best_effort(
            &mut expected,
            viewer.connection,
            &explode_packet(explosion, viewer.entity_id),
        );
    }
    for event in &output.changes.entity_events[..3] {
        push_entity_event(&mut expected, viewer.connection, *event);
    }
    let packets = plan_viewer(&output, &mut viewer, 4, &cache);
    let ids: HashSet<_> = expected.iter().map(|p| p.id).collect();
    assert_eq!(
        packets
            .iter()
            .filter(|p| ids.contains(&p.id))
            .map(|p| &p.data)
            .collect::<Vec<_>>(),
        expected.iter().map(|p| &p.data).collect::<Vec<_>>()
    );
}

#[test]
fn unchanged_view_reuses_window_and_planning_scratch() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let output = output(
        vec![],
        vec![entity(10, [8.0, 64.0, 8.0])],
        ChangeSet::default(),
    );
    plan_viewer(&output, &mut viewer, 4, &cache);
    let order = viewer.window.order.as_ptr();
    let want_capacity = viewer.window.want.capacity();
    let visible = viewer.scratch.visible.as_ptr();
    let spawn_capacity = viewer.scratch.spawns.capacity();
    let packets = plan_viewer(&output, &mut viewer, 4, &cache);
    assert!(!packets.iter().any(|p| {
        [
            AddEntity::ID,
            ClientboundSetEntityData::ID,
            MoveEntityPos::ID,
            RemoveEntities::ID,
        ]
        .contains(&p.id)
    }));
    assert_eq!(viewer.window.order.as_ptr(), order);
    assert_eq!(viewer.window.want.capacity(), want_capacity);
    assert_eq!(viewer.scratch.visible.as_ptr(), visible);
    assert_eq!(viewer.scratch.spawns.capacity(), spawn_capacity);
}

#[test]
fn zero_credit_does_not_allocate_spawn_candidates_and_resumes_nearest_first() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let output = output(
        vec![],
        vec![
            entity(10, [8.0, 100.0, 8.0]),
            entity(11, [8.0, 64.0, 11.0]),
            entity(12, [8.0, 64.0, 9.0]),
            entity(13, [8.0, 64.0, 10.0]),
        ],
        ChangeSet::default(),
    );
    viewer.border_baseline = Some(output.snapshot.border);
    viewer.world_credit = 0;
    let packets = plan_viewer(&output, &mut viewer, 4, &cache);
    assert!(!packets.iter().any(|packet| packet.id == AddEntity::ID));
    assert_eq!(
        viewer.scratch.spawns.capacity(),
        0,
        "a full world queue must not materialize the unadmitted population"
    );
    viewer.world_credit = 4; // AddEntity + metadata for each of the two nearest.
    let packets = plan_viewer(&output, &mut viewer, 4, &cache);
    let spawned: Vec<_> = packets
        .iter()
        .filter(|packet| packet.id == AddEntity::ID)
        .map(|packet| {
            AddEntity::decode(&mut &body_of(packet)[..])
                .unwrap()
                .entity_id
        })
        .collect();
    assert_eq!(spawned, [12, 13]);
    viewer.world_credit = 4;
    let packets = plan_viewer(&output, &mut viewer, 4, &cache);
    let spawned: Vec<_> = packets
        .iter()
        .filter(|packet| packet.id == AddEntity::ID)
        .map(|packet| {
            AddEntity::decode(&mut &body_of(packet)[..])
                .unwrap()
                .entity_id
        })
        .collect();
    assert_eq!(spawned, [11, 10], "vertical distance still orders spawns");
}

#[test]
fn repeated_snapshot_stamp_reconciles_type_changes_and_credit_debt() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let mut mob = entity(10, [8.0, 64.0, 8.0]);
    let initial = output(vec![], vec![mob.clone()], ChangeSet::default());
    plan_viewer(&initial, &mut viewer, 4, &cache);
    mob.type_id = 30;
    let changed = output(vec![], vec![mob.clone()], ChangeSet::default());
    viewer.world_credit = 0;
    assert!(
        !plan_viewer(&changed, &mut viewer, 4, &cache)
            .iter()
            .any(|p| p.id == RemoveEntities::ID)
    );
    viewer.world_credit = 512;
    let packets = plan_viewer(&changed, &mut viewer, 4, &cache);
    let removal = packets
        .iter()
        .position(|p| p.id == RemoveEntities::ID)
        .unwrap();
    let spawn = packets.iter().position(|p| p.id == AddEntity::ID).unwrap();
    assert!(removal < spawn);
    assert_eq!(
        AddEntity::decode(&mut &body_of(&packets[spawn])[..])
            .unwrap()
            .entity_type_id,
        30
    );
    // Repeated tick number is normal after queue pressure: visibility stamps
    // belong to the plan, not merely the producer's numeric tick.
    let gone = output(vec![], vec![], ChangeSet::default());
    let packets = plan_viewer(&gone, &mut viewer, 4, &cache);
    let removal = packets.iter().find(|p| p.id == RemoveEntities::ID).unwrap();
    assert_eq!(
        RemoveEntities::decode(&mut &body_of(removal)[..])
            .unwrap()
            .entity_ids,
        vec![10]
    );
}

#[test]
fn multi_removal_has_canonical_wire_order_independent_of_tracking_container() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let initial = output(
        vec![],
        [40, 10, 30, 20]
            .into_iter()
            .map(|id| entity(id, [8.0, 64.0, 8.0]))
            .collect(),
        ChangeSet::default(),
    );
    plan_viewer(&initial, &mut viewer, 4, &cache);
    let packets = plan_viewer(
        &output(vec![], vec![], ChangeSet::default()),
        &mut viewer,
        4,
        &cache,
    );
    let removal = packets.iter().find(|p| p.id == RemoveEntities::ID).unwrap();
    let mut expected = Vec::new();
    push_best_effort(
        &mut expected,
        viewer.connection,
        &RemoveEntities {
            entity_ids: vec![10, 20, 30, 40],
        },
    );
    assert_eq!(removal.data, expected[0].data);
}

#[test]
fn wrapped_plan_stamp_does_not_retain_an_absent_entity() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    plan_viewer(
        &output(
            vec![],
            vec![entity(10, viewer.position)],
            ChangeSet::default(),
        ),
        &mut viewer,
        4,
        &cache,
    );
    viewer.plan_stamp = u64::MAX;
    let packets = plan_viewer(
        &output(vec![], vec![], ChangeSet::default()),
        &mut viewer,
        4,
        &cache,
    );
    assert!(packets.iter().any(|packet| packet.id == RemoveEntities::ID));
    assert!(!viewer.tracked_entities.contains_key(&EntityId(10)));
}
