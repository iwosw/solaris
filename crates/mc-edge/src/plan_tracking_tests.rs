//! Wire-level vanilla tracking contract regressions.
use super::*;

fn at_tick(mut output: TickOutput, tick: u64) -> TickOutput {
    output.tick = tick;
    let snapshot = Arc::get_mut(&mut output.snapshot).unwrap();
    snapshot.tick = tick;
    snapshot.day_time = 1000 + tick as i64;
    output
}

#[test]
fn cow_moves_and_head_turns_have_id_phased_vanilla_cadence() {
    let cache = ChunkPacketCache::new();
    let (pos, chunk) = chunk_at(0, 0);
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    let mut cows: Vec<_> = (10..13)
        .map(|id| {
            let mut cow = entity(id, [8.0, 64.0, 8.0]);
            cow.type_id = 30;
            cow
        })
        .collect();
    plan_viewer(
        &output(
            vec![(pos, chunk.clone())],
            cows.clone(),
            ChangeSet::default(),
        ),
        &mut viewer,
        4,
        &cache,
    );
    let mut received = [8.0; 3];
    for tick in 2..=10 {
        for cow in &mut cows {
            cow.position[0] += 0.25;
            cow.head_yaw += 5.0;
        }
        let output = at_tick(
            output(
                vec![(pos, chunk.clone())],
                cows.clone(),
                ChangeSet::default(),
            ),
            tick,
        );
        let packets = plan_viewer(&output, &mut viewer, 4, &cache);
        let moves: Vec<_> = packets
            .iter()
            .filter(|p| p.id == MoveEntityPos::ID)
            .map(|p| MoveEntityPos::decode(&mut &body_of(p)[..]).unwrap())
            .collect();
        let heads: Vec<_> = packets
            .iter()
            .filter(|p| p.id == RotateHead::ID)
            .map(|p| RotateHead::decode(&mut &body_of(p)[..]).unwrap())
            .collect();
        for cow in &cows {
            let due = (tick + cow.id.0 as u64).is_multiple_of(3);
            let movement = moves.iter().find(|p| p.entity_id == cow.id.0);
            assert_eq!(movement.is_some(), due, "tick={tick}, entity={:?}", cow.id);
            assert_eq!(heads.iter().any(|p| p.entity_id == cow.id.0), due);
            if let Some(movement) = movement {
                let received = &mut received[(cow.id.0 - 10) as usize];
                *received += f64::from(movement.delta_x) / 4096.0;
                assert_eq!(
                    *received, cow.position[0],
                    "delta uses last delivered baseline"
                );
            }
        }
    }
}

#[test]
fn cow_range_is_horizontal_inclusive_and_capped_by_view_distance() {
    let cache = ChunkPacketCache::new();
    for (vd, range) in [(4, 64.0), (12, 160.0)] {
        let mut viewer = viewer_at(1, [15.75, 64.0, -0.25]);
        let mut cow = entity(10, [15.75 + range, -100.0, -0.25 + range]);
        cow.type_id = 30;
        let pos = ChunkPos {
            x: (cow.position[0] / 16.0).floor() as i32,
            z: (cow.position[2] / 16.0).floor() as i32,
        };
        let (_, chunk) = chunk_at(pos.x, pos.z);
        let initial = output(
            vec![(pos, chunk.clone())],
            vec![cow.clone()],
            ChangeSet::default(),
        );
        viewer
            .loaded_chunks
            .insert((pos.x, pos.z), initial.snapshot.chunks[0].revision);
        let packets = plan_viewer(&initial, &mut viewer, vd, &cache);
        assert!(
            packets.iter().any(|p| p.id == AddEntity::ID),
            "inclusive square ignores height"
        );
        cow.position[0] += 0.001;
        viewer.world_credit = 0;
        let outside = output(
            vec![(pos, chunk.clone())],
            vec![cow.clone()],
            ChangeSet::default(),
        );
        let packets = plan_viewer(&outside, &mut viewer, vd, &cache);
        assert!(!packets.iter().any(|p| p.id == RemoveEntities::ID));
        viewer.world_credit = 512;
        let packets = plan_viewer(&outside, &mut viewer, vd, &cache);
        let removal = packets
            .iter()
            .find(|p| p.id == RemoveEntities::ID)
            .expect("range edge despawns without waiting for cadence");
        assert_eq!(
            RemoveEntities::decode(&mut &body_of(removal)[..])
                .unwrap()
                .entity_ids,
            vec![10]
        );
        cow.position[0] -= 0.001;
        let inside = output(vec![(pos, chunk)], vec![cow], ChangeSet::default());
        let packets = plan_viewer(&inside, &mut viewer, vd, &cache);
        assert!(packets.iter().any(|p| p.id == AddEntity::ID));
    }
}

#[test]
fn entity_waits_for_held_chunk_and_appearance_does_not_wait_for_cadence() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    let mut cow = entity(10, [8.0, 64.0, 8.0]);
    cow.type_id = 30;
    let initial = output(vec![], vec![cow.clone()], ChangeSet::default());
    let packets = plan_viewer(&initial, &mut viewer, 4, &cache);
    assert!(
        !packets.iter().any(|p| p.id == AddEntity::ID),
        "no floating entity before its chunk"
    );
    viewer.loaded_chunks.insert((0, 0), 0);
    plan_viewer(&initial, &mut viewer, 4, &cache);
    // Observe tick 2 unchanged, so tick 3 is off phase rather than a skipped due.
    plan_viewer(
        &at_tick(output(vec![], vec![cow.clone()], ChangeSet::default()), 2),
        &mut viewer,
        4,
        &cache,
    );
    Arc::make_mut(&mut cow.appearance).health = 3.0;
    cow.position[0] += 0.5;
    let changed = at_tick(output(vec![], vec![cow], ChangeSet::default()), 3); // 3+10 is not due.
    let packets = plan_viewer(&changed, &mut viewer, 4, &cache);
    assert!(packets.iter().any(|p| p.id == ClientboundSetEntityData::ID));
    assert!(!packets.iter().any(|p| p.id == MoveEntityPos::ID));
}

#[test]
fn contract_intervals_one_two_twenty_and_max_are_not_cow_special_cases() {
    let cache = ChunkPacketCache::new();
    for (type_id, interval) in [(15, 1), (155, 2), (6, 20), (3, i32::MAX as u64)] {
        let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
        viewer.loaded_chunks.insert((0, 0), 0);
        let mut tracked = entity(20, [8.0, 64.0, 8.0]);
        tracked.type_id = type_id;
        plan_viewer(
            &output(vec![], vec![tracked.clone()], ChangeSet::default()),
            &mut viewer,
            4,
            &cache,
        );
        for tick in 2..=21 {
            tracked.position[0] += 0.125;
            let moved = at_tick(
                output(vec![], vec![tracked.clone()], ChangeSet::default()),
                tick,
            );
            let packets = plan_viewer(&moved, &mut viewer, 4, &cache);
            assert_eq!(
                packets.iter().any(|p| p.id == MoveEntityPos::ID),
                (tick + 20).is_multiple_of(interval),
                "type={type_id}, tick={tick}"
            );
        }
    }
}

#[test]
fn cadence_preserves_credit_deferred_baseline_until_the_next_due_tick() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let mut cow = entity(10, [8.0, 64.0, 8.0]);
    cow.type_id = 30;
    plan_viewer(
        &output(vec![], vec![cow.clone()], ChangeSet::default()),
        &mut viewer,
        4,
        &cache,
    );
    cow.position[0] += 0.5;
    viewer.world_credit = 0;
    let deferred = at_tick(output(vec![], vec![cow.clone()], ChangeSet::default()), 2);
    assert!(
        !plan_viewer(&deferred, &mut viewer, 4, &cache)
            .iter()
            .any(|p| p.id == MoveEntityPos::ID)
    );
    viewer.world_credit = 512;
    let not_due = at_tick(output(vec![], vec![cow.clone()], ChangeSet::default()), 3);
    assert!(
        !plan_viewer(&not_due, &mut viewer, 4, &cache)
            .iter()
            .any(|p| p.id == MoveEntityPos::ID)
    );
    let due = at_tick(output(vec![], vec![cow], ChangeSet::default()), 5);
    let packets = plan_viewer(&due, &mut viewer, 4, &cache);
    let movement = packets.iter().find(|p| p.id == MoveEntityPos::ID).unwrap();
    assert_eq!(
        MoveEntityPos::decode(&mut &body_of(movement)[..])
            .unwrap()
            .delta_x,
        2048
    );
}

#[test]
fn skipped_due_snapshots_do_not_freeze_movement_or_head_turns() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let mut cow = entity(10, viewer.position);
    cow.type_id = 30;
    plan_viewer(
        &output(vec![], vec![cow.clone()], ChangeSet::default()),
        &mut viewer,
        4,
        &cache,
    );
    for tick in [3, 6, 9] {
        cow.position[0] += 0.5;
        cow.head_yaw += 15.0;
        let snapshot = at_tick(
            output(vec![], vec![cow.clone()], ChangeSet::default()),
            tick,
        );
        let packets = plan_viewer(&snapshot, &mut viewer, 4, &cache);
        let movement = packets
            .iter()
            .find(|packet| packet.id == MoveEntityPos::ID)
            .expect("crossed phase reconciles movement");
        assert_eq!(
            MoveEntityPos::decode(&mut &body_of(movement)[..])
                .unwrap()
                .delta_x,
            2048
        );
        assert!(packets.iter().any(|packet| packet.id == RotateHead::ID));
    }
}

#[test]
fn walking_inside_dense_chunk_matches_full_vanilla_range_oracle() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [0.125, 64.0, 7.75]);
    viewer.loaded_chunks.insert((0, 0), 0);
    viewer.loaded_chunks.insert((4, 0), 0);
    let entities = (0..2000)
        .map(|i| {
            entity(
                10 + i,
                [
                    0.125 + f64::from(i % 50) * 0.3125 + if i < 1000 { 0.0 } else { 64.0 },
                    64.0,
                    0.125 + f64::from((i % 1000) / 50) * 0.390625,
                ],
            )
        })
        .collect();
    let projection = output(vec![], entities, ChangeSet::default());
    viewer.world_credit = usize::MAX;
    let mut initial_count = 0;
    for step in 0..80 {
        viewer.position[0] = 0.125 + f64::from(step) * 0.125;
        let mut expected: Vec<_> = projection
            .snapshot
            .entities
            .iter()
            .filter(|entity| entity_trackable(entity, &viewer, 4))
            .map(|entity| entity.id)
            .collect();
        expected.sort_unstable();
        let packets = plan_viewer(&projection, &mut viewer, 4, &cache);
        let mut actual: Vec<_> = viewer.tracked_entities.keys().copied().collect();
        actual.sort_unstable();
        assert_eq!(actual, expected, "vanilla range oracle at step {step}");
        if step == 0 {
            initial_count = actual.len();
        }
        assert!(
            !packets
                .iter()
                .any(|packet| packet.id == EntityPositionSync::ID)
        );
    }
    assert!(
        viewer.tracked_entities.len() > initial_count,
        "walking within one chunk admits new rows at the vanilla range boundary"
    );
}

#[test]
fn incremental_wire_snapshots_match_full_packet_recompute_after_skips_and_removals() {
    use mc_core::entities::{ColdFields, EntityKind, EntityStore};
    let mut store = EntityStore::default();
    let spawn = |store: &mut EntityStore, id: i32| {
        store.spawn(
            EntityId(id),
            uuid::Uuid::from_u128(id as u128),
            EntityKind::Mob { type_id: 15 },
            ColdFields {
                pos: [f64::from(id % 60) + 0.25, 64.0, 0.25],
                vel: [0.0; 3],
                yaw: 0.0,
                pitch: 0.0,
                ground: true,
                health: 20.0,
                breed_cooldown: 0,
                pickup_delay: 0,
                item: None,
                appearance: Arc::default(),
                movement: mc_core::blueprint::MovementClass::Walker,
                half_width: 0.3,
                height: 1.8,
                speed: 0.1,
                attack_damage: 0.0,
                sight_range: 16.0,
                attack_reach: 2.0,
            },
        )
    };
    for id in 10..74 {
        spawn(&mut store, id);
    }
    let cache = ChunkPacketCache::new();
    let mut incremental = viewer_at(1, [0.0, 64.0, 0.0]);
    let mut oracle = viewer_at(1, [0.0, 64.0, 0.0]);
    for viewer in [&mut incremental, &mut oracle] {
        for x in 0..4 {
            viewer.loaded_chunks.insert((x, 0), 0);
        }
    }
    for tick in 1..=30 {
        if tick > 1 {
            let slot = store.slot_of_uuid(uuid::Uuid::from_u128(10)).unwrap();
            store.set_pos(slot, [f64::from(tick) * 0.125, 64.0, 0.25]);
            if tick == 8 {
                store.remove(store.slot_of_uuid(uuid::Uuid::from_u128(11)).unwrap());
            }
            if tick == 12 {
                spawn(&mut store, 99);
            }
            if tick == 16 {
                Arc::make_mut(&mut store.row_mut(slot).appearance).shared_flags = 0x20;
            }
        }
        let views = store.views();
        if tick == 20 {
            continue;
        }
        let mut current = at_tick(output(vec![], vec![], ChangeSet::default()), tick as u64);
        Arc::get_mut(&mut current.snapshot).unwrap().entities = views.clone();
        let rebuilt = at_tick(
            output(
                vec![],
                views.iter().cloned().collect(),
                ChangeSet::default(),
            ),
            tick as u64,
        );
        if tick == 23 {
            incremental.position[0] = 1.0;
            oracle.position[0] = 1.0;
        }
        incremental.world_credit = if tick == 7 || tick == 8 { 0 } else { 512 };
        oracle.world_credit = incremental.world_credit;
        let actual = plan_viewer(&current, &mut incremental, 4, &cache);
        let expected = plan_viewer(&rebuilt, &mut oracle, 4, &cache);
        assert_eq!(
            actual
                .iter()
                .map(|packet| (packet.id, &packet.data))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|packet| (packet.id, &packet.data))
                .collect::<Vec<_>>(),
            "tick {tick}"
        );
        if tick == 3 {
            assert_eq!(views.changed(), &[EntityId(10)]);
        }
    }
    assert!(!incremental.tracked_entities.contains_key(&EntityId(11)));
    assert!(incremental.tracked_entities.contains_key(&EntityId(99)));
}
