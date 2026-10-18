use super::*;

#[test]
fn views_preserve_signed_chunk_identity_order_and_appearance_sharing() {
    for count in [0, 7, 511, 512, 600] {
        let mut store = EntityStore::default();
        let coordinates = [i32::MIN / 16, -2, -1, 0, 1, 2, i32::MAX / 16];
        let mut expected = Vec::new();
        for slot in 0..count {
            let x = coordinates[slot % coordinates.len()];
            let z = coordinates[(slot / coordinates.len()) % coordinates.len()];
            let id = EntityId((count - slot) as i32);
            store.spawn(
                id,
                Uuid::from_u128(id.0 as u128),
                EntityKind::Mob { type_id: 7 },
                ColdFields {
                    pos: [f64::from(x) * 16.0, 64.0, f64::from(z) * 16.0],
                    vel: [0.0; 3],
                    yaw: 12.0,
                    pitch: 3.0,
                    ground: true,
                    health: 20.0,
                    breed_cooldown: 0,
                    pickup_delay: 0,
                    item: None,
                    appearance: Arc::new(EntityAppearance::default()),
                    movement: MovementClass::Walker,
                    half_width: 0.3,
                    height: 1.8,
                    speed: 0.1,
                    attack_damage: 1.0,
                    sight_range: 16.0,
                    attack_reach: 1.0,
                },
            );
            expected.push((x, z, id.0));
        }
        expected.sort_unstable();
        let views = store.views();
        assert_eq!(
            views.iter().map(|view| view.id.0).collect::<Vec<_>>(),
            expected.iter().map(|key| key.2).collect::<Vec<_>>()
        );
        for view in &views {
            let slot = store.slot_of(view.id).unwrap();
            assert!(Arc::ptr_eq(&view.appearance, &store.row(slot).appearance));
            let chunk = EntityStore::brain_chunk(view.position);
            assert!(views.in_chunk(chunk).any(|entry| entry.id == view.id));
        }
        assert_eq!(views.len(), count);
    }
}

fn wire_fields(position: [f64; 3]) -> ColdFields {
    ColdFields {
        pos: position,
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        ground: true,
        health: 20.0,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: Arc::new(EntityAppearance::default()),
        movement: MovementClass::Walker,
        half_width: 0.3,
        height: 1.8,
        speed: 0.1,
        attack_damage: 1.0,
        sight_range: 16.0,
        attack_reach: 1.0,
    }
}

#[test]
fn pickup_and_mob_lanes_follow_compaction_transfer_and_kind_changes() {
    let player = EntityKind::Player {
        connection: crate::boundary::ConnectionId(1),
    };
    let item = EntityKind::Item { type_id: 1 };
    let mob = EntityKind::Mob { type_id: 30 };
    let mut source = EntityStore::default();
    let mut destination = EntityStore::default();
    let assert_lanes = |store: &EntityStore| {
        let expected = |kind: EntityKind| {
            store
                .slots()
                .filter(|&slot| store.body(slot).kind == kind)
                .collect::<Vec<_>>()
        };
        assert_eq!(store.player_slots().collect::<Vec<_>>(), expected(player));
        assert_eq!(store.item_slots().collect::<Vec<_>>(), expected(item));
        assert_eq!(store.mob_slots(30).collect::<Vec<_>>(), expected(mob));
    };
    for (id, kind) in [mob, player, item, player, item, mob, item, player]
        .into_iter()
        .enumerate()
    {
        source.spawn(
            EntityId(id as i32 + 1),
            Uuid::from_u128(id as u128 + 1),
            kind,
            wire_fields([0.0, 64.0, 0.0]),
        );
    }
    assert_lanes(&source); // Spawn membership cannot rely on lazy motion flags.
    source.body_mut(EntitySlot(1)).alive = false;
    assert_lanes(&source); // Lanes include dead rows; consumers filter alive.
    source.remove(EntitySlot(0)); // Last player moves over a mob.
    assert_lanes(&source);
    source.remove(EntitySlot(1)); // Last item moves over a player.
    assert_lanes(&source);
    source.transfer_to(EntitySlot(0), &mut destination, [0.0; 3], [0.0; 2]);
    assert_lanes(&source);
    assert_lanes(&destination);
    source.transfer_to(EntitySlot(1), &mut destination, [0.0; 3], [0.0; 2]);
    assert_lanes(&source);
    assert_lanes(&destination);
    source.set_kind(EntitySlot(0), player);
    source.set_kind(EntitySlot(2), item);
    source.set_kind(EntitySlot(3), mob);
    assert_lanes(&source);
    destination.set_kind(EntitySlot(0), item);
    destination.transfer_to(EntitySlot(0), &mut source, [0.0; 3], [0.0; 2]);
    assert_lanes(&source);
    assert_lanes(&destination);
}

fn assert_wire_oracle(store: &EntityStore, actual: &crate::boundary::EntityViews) {
    let full: crate::boundary::EntityViews = store
        .slots()
        .map(|slot| store.wire_view(slot))
        .collect::<Vec<_>>()
        .into();
    assert_eq!(
        actual.iter().collect::<Vec<_>>(),
        full.iter().collect::<Vec<_>>()
    );
    for view in actual {
        let chunk = EntityStore::brain_chunk(view.position);
        assert_eq!(
            actual.in_chunk(chunk).map(|row| row.id).collect::<Vec<_>>(),
            full.in_chunk(chunk).map(|row| row.id).collect::<Vec<_>>()
        );
    }
}

#[test]
fn incremental_wire_views_touch_one_row_and_retain_immutable_versions() {
    let mut store = EntityStore::default();
    for id in 1..=1000 {
        store.spawn(
            EntityId(id),
            Uuid::from_u128(id as u128),
            EntityKind::Mob { type_id: 30 },
            wire_fields([f64::from(id % 64) + 0.25, 64.0, f64::from(id / 64) * 2.0]),
        );
    }
    // The journal can drain first: wire publication has independent consumer
    // custody of the same owner notification, not another detection scan.
    assert_eq!(store.take_touched().len(), 1000);
    let initial = store.views();
    assert_eq!(initial.changed().len(), 1000);
    assert_wire_oracle(&store, &initial);
    let quiet = store.views();
    assert_eq!(quiet.revision(), initial.revision());
    assert!(!store.wire_marked.iter().any(|marked| *marked));
    let id = EntityId(10);
    let slot = store.slot_of(id).unwrap();
    let before = initial.get(id).unwrap().clone();
    store.set_pos(slot, [10.5, 64.25, 0.5]);
    store.set_vel(slot, [0.5, 0.0, 0.0]);
    store.set_rot(slot, 45.0, 12.0);
    store.set_ground(slot, false);
    assert_eq!(
        store.wire_marked.iter().filter(|marked| **marked).count(),
        1
    );
    assert_eq!(store.wire_pending, vec![(id, slot)]);
    assert!(
        store.touched_ids().is_empty(),
        "intra-chunk wire motion is not cold WAL"
    );
    let changed = store.views();
    assert_eq!(changed.changed(), &[id]);
    assert_eq!(changed.base_revision(), initial.revision());
    assert_eq!(initial.get(id), Some(&before));
    assert_wire_oracle(&store, &changed);
    store.set_pos(slot, store.pos(slot));
    store.set_vel(slot, store.vel(slot));
    let rot = store.rot(slot);
    store.set_rot(slot, rot[0], rot[1]);
    store.set_ground(slot, store.ground(slot));
    assert!(
        !store.wire_marked.iter().any(|marked| *marked),
        "equal column writes do not create candidates"
    );
    let _ = store.row_mut(slot);
    let same = store.views();
    assert_eq!(same.revision(), changed.revision());
    assert_eq!(same.base_revision(), changed.base_revision());
    assert_eq!(
        same.changed(),
        changed.changed(),
        "conservative borrow is not a wire change"
    );
    assert_eq!(
        store.take_touched(),
        vec![id],
        "wire publication never drains journal marks"
    );
    Arc::make_mut(&mut store.row_mut(slot).appearance).health = 9.0;
    store.set_uuid(slot, Uuid::from_u128(100_010));
    let appearance = store.views();
    assert_eq!(appearance.changed(), &[id]);
    assert_wire_oracle(&store, &appearance);
    assert_eq!(initial.get(id), Some(&before));
}

#[test]
fn incremental_wire_views_match_full_oracle_through_moves_births_and_swap_removals() {
    let mut store = EntityStore::default();
    for id in 1..=64 {
        store.spawn(
            EntityId(id),
            Uuid::from_u128(id as u128),
            EntityKind::Mob { type_id: 30 },
            wire_fields([f64::from(id), 64.0, -0.25]),
        );
    }
    let retained = store.views();
    for step in 0..32 {
        let id = EntityId(step + 1);
        let slot = store.slot_of(id).unwrap();
        store.set_pos(slot, [f64::from(step * 17) - 128.5, 65.0, f64::from(-step)]);
        if step % 3 == 0 {
            Arc::make_mut(&mut store.row_mut(slot).appearance).baby = true;
        }
        if step == 10 {
            store.remove(store.slot_of(EntityId(60)).unwrap());
        }
        if step == 15 {
            store.spawn(
                EntityId(100),
                Uuid::from_u128(100),
                EntityKind::Mob { type_id: 30 },
                wire_fields([-0.25, 64.0, 32.5]),
            );
        }
        let snapshot = store.views();
        assert_wire_oracle(&store, &snapshot);
        if step >= 10 {
            assert!(snapshot.get(EntityId(60)).is_none());
        }
        assert!(retained.get(EntityId(60)).is_some());
        assert_eq!(
            retained.get(EntityId(1)).unwrap().position,
            [1.0, 64.0, -0.25]
        );
    }
}

#[test]
fn wire_views_transfer_publishes_source_tombstone_and_destination_after_image() {
    let mut source = EntityStore::default();
    let mut destination = EntityStore::default();
    let id = EntityId(7);
    let slot = source.spawn(
        id,
        Uuid::from_u128(7),
        EntityKind::Mob { type_id: 30 },
        wire_fields([-0.25, 64.0, -0.25]),
    );
    let retained = source.views();
    source.transfer_to(slot, &mut destination, [32.5, 70.0, 48.5], [90.0, 5.0]);
    let removed = source.views();
    let arrived = destination.views();
    assert_eq!(removed.changed(), &[id]);
    assert!(removed.get(id).is_none());
    assert_eq!(arrived.changed(), &[id]);
    assert_eq!(arrived.get(id).unwrap().position, [32.5, 70.0, 48.5]);
    assert_eq!(retained.get(id).unwrap().position, [-0.25, 64.0, -0.25]);
    assert_wire_oracle(&source, &removed);
    assert_wire_oracle(&destination, &arrived);
}

#[test]
fn wire_views_preserve_sparse_signed_ids_and_duplicate_after_images() {
    let mut store = EntityStore::default();
    let slot = store.spawn(
        EntityId(1),
        Uuid::from_u128(1),
        EntityKind::Mob { type_id: 30 },
        wire_fields([0.0, 64.0, 0.0]),
    );
    let template = store.wire_view(slot);
    let ids = [i32::MIN, -1, 0, i32::MAX];
    let mut rows = Vec::new();
    for id in ids {
        let mut row = template.clone();
        row.id = EntityId(id);
        rows.push(row);
    }
    let mut moved = rows[1].clone();
    moved.position = [-16.5, 64.0, 32.5];
    rows.push(moved.clone());
    let mut views: crate::boundary::EntityViews = rows.into();
    assert_eq!(views.len(), ids.len());
    assert_eq!(
        views.iter().map(|row| row.id.0).collect::<Vec<_>>(),
        [-1, i32::MIN, 0, i32::MAX]
    );
    assert_eq!(views.get(EntityId(-1)), Some(&moved));
    assert_eq!(
        views.in_chunk(ChunkPos { x: 0, z: 0 }).count(),
        3,
        "duplicate identity must leave its previous chunk"
    );
    let retained = views.clone();
    let publish = |views: &mut crate::boundary::EntityViews, id, image: Option<&EntityView>| {
        views.publish(
            &[(id, image)],
            |&(id, image)| (id, image),
            |image| image.clone(),
        );
    };
    publish(&mut views, EntityId(-1), Some(&moved));
    assert_eq!(
        views.revision(),
        retained.revision(),
        "equal image is not a change"
    );
    publish(&mut views, EntityId(i32::MIN), None);
    assert_eq!(views.changed(), &[EntityId(i32::MIN)]);
    assert!(views.get(EntityId(i32::MIN)).is_none());
    assert!(retained.get(EntityId(i32::MIN)).is_some());
    assert_eq!(retained.len(), ids.len());
}

#[test]
fn wire_views_page_boundaries_reuse_slots_without_changing_retained_images() {
    let mut store = EntityStore::default();
    for id in 1..=1000 {
        store.spawn(
            EntityId(id),
            Uuid::from_u128(id as u128),
            EntityKind::Mob { type_id: 30 },
            wire_fields([f64::from(id), 64.0, 0.0]),
        );
    }
    let initial = store.views();
    store.set_pos(store.slot_of(EntityId(1)).unwrap(), [1.0, 65.0, 0.0]);
    let moved = store.views();
    assert_eq!(initial.get(EntityId(1)).unwrap().position[1], 64.0);
    assert_eq!(moved.get(EntityId(1)).unwrap().position[1], 65.0);
    assert!(std::ptr::eq(
        initial.get(EntityId(700)).unwrap(),
        moved.get(EntityId(700)).unwrap()
    ));
    for id in [1, 64, 65, 128, 129] {
        store.remove(store.slot_of(EntityId(id)).unwrap());
    }
    for id in 2000..2005 {
        store.spawn(
            EntityId(id),
            Uuid::from_u128(id as u128),
            EntityKind::Mob { type_id: 30 },
            wire_fields([-32.5, 70.0, f64::from(id)]),
        );
    }
    let reused = store.views();
    assert_wire_oracle(&store, &reused);
    assert_eq!(reused.len(), initial.len());
    for id in [1, 64, 65, 128, 129] {
        assert!(reused.get(EntityId(id)).is_none());
        assert!(initial.get(EntityId(id)).is_some());
        assert!(moved.get(EntityId(id)).is_some());
    }
    for id in 2000..2005 {
        assert!(reused.get(EntityId(id)).is_some());
        assert!(initial.get(EntityId(id)).is_none());
    }
}

#[test]
fn wire_pending_queue_reuses_capacity_after_large_and_sparse_publications() {
    let mut store = EntityStore::default();
    for id in 1..=1000 {
        store.spawn(
            EntityId(id),
            Uuid::from_u128(id as u128),
            EntityKind::Mob { type_id: 30 },
            wire_fields([f64::from(id), 64.0, 0.0]),
        );
    }
    store.views();
    let capacity = store.wire_pending.capacity();
    assert!(capacity >= 1000);
    let id = EntityId(7);
    let slot = store.slot_of(id).unwrap();
    for _ in 0..10 {
        let _ = store.row_mut(slot);
    }
    assert_eq!(store.wire_pending, vec![(id, slot)]);
    let quiet = store.views();
    assert!(!store.wire_marked.iter().any(|marked| *marked));
    assert!(store.wire_pending.is_empty());
    assert_eq!(store.wire_pending.capacity(), capacity);
    store.set_pos(slot, [7.0, 65.0, 0.0]);
    let moved = store.views();
    assert_eq!(moved.changed(), &[id]);
    assert_eq!(moved.base_revision(), quiet.revision());
    assert_eq!(store.wire_pending.capacity(), capacity);
    assert_wire_oracle(&store, &moved);
}

#[test]
fn wire_pending_queue_deduplicates_removal_and_rebirth_before_publication() {
    let mut store = EntityStore::default();
    let id = EntityId(7);
    let slot = store.spawn(
        id,
        Uuid::from_u128(7),
        EntityKind::Mob { type_id: 30 },
        wire_fields([0.0, 64.0, 0.0]),
    );
    let retained = store.views();
    store.remove(slot);
    store.spawn(
        id,
        Uuid::from_u128(8),
        EntityKind::Mob { type_id: 30 },
        wire_fields([2.0, 64.0, 0.0]),
    );
    assert_eq!(store.wire_pending, vec![(id, slot)]);
    let updated = store.views();
    assert_eq!(updated.changed(), &[id]);
    assert_eq!(updated.get(id).unwrap().uuid, Uuid::from_u128(8));
    assert_eq!(retained.get(id).unwrap().uuid, Uuid::from_u128(7));
    assert_wire_oracle(&store, &updated);
}

#[test]
fn wire_pending_slot_hints_survive_compaction_rebirth_and_transfer() {
    let mut source = EntityStore::default();
    let mut destination = EntityStore::default();
    for id in 1..=96 {
        source.spawn(
            EntityId(id),
            Uuid::from_u128(id as u128),
            EntityKind::Mob { type_id: 30 },
            wire_fields([f64::from(id), 64.0, 0.0]),
        );
    }
    let retained = source.views();
    let moved_id = EntityId(96);
    let old_slot = source.slot_of(moved_id).unwrap();
    source.set_pos(old_slot, [120.5, 65.0, 0.5]);
    source.remove(source.slot_of(EntityId(1)).unwrap());
    // Occupy the queued hint with another identity, then reuse the removed ID
    // at a different slot without replacing its first deduplicated hint.
    source.spawn(
        EntityId(97),
        Uuid::from_u128(97),
        EntityKind::Mob { type_id: 30 },
        wire_fields([97.0, 64.0, 0.0]),
    );
    source.spawn(
        EntityId(1),
        Uuid::from_u128(1001),
        EntityKind::Mob { type_id: 30 },
        wire_fields([1.5, 70.0, 0.5]),
    );
    assert_eq!(source.wire_pending[0], (moved_id, old_slot));
    let updated = source.views();
    assert_wire_oracle(&source, &updated);
    assert_eq!(updated.get(moved_id).unwrap().position, [120.5, 65.0, 0.5]);
    assert_eq!(
        updated.get(EntityId(1)).unwrap().uuid,
        Uuid::from_u128(1001)
    );
    assert_eq!(retained.get(EntityId(1)).unwrap().uuid, Uuid::from_u128(1));

    let slot = source.slot_of(moved_id).unwrap();
    source.set_vel(slot, [0.25, 0.0, 0.0]);
    source.transfer_to(slot, &mut destination, [32.5, 70.0, 48.5], [90.0, 5.0]);
    let removed = source.views();
    let arrived = destination.views();
    assert!(removed.get(moved_id).is_none());
    assert_eq!(arrived.get(moved_id).unwrap().position, [32.5, 70.0, 48.5]);
    assert_eq!(arrived.get(moved_id).unwrap().velocity, [0.25, 0.0, 0.0]);
    assert_wire_oracle(&source, &removed);
    assert_wire_oracle(&destination, &arrived);
}

#[test]
fn wire_view_chunks_preserve_floor_cast_boundaries() {
    let coordinates = [
        f64::NEG_INFINITY,
        f64::from(i32::MIN) - 0.5,
        -16.25,
        -16.0,
        -0.25,
        -0.0,
        0.0,
        15.999,
        16.0,
        f64::from(i32::MAX) + 0.5,
        f64::INFINITY,
        f64::NAN,
    ];
    let mut store = EntityStore::default();
    for (index, &x) in coordinates.iter().enumerate() {
        let z = coordinates[coordinates.len() - 1 - index];
        store.spawn(
            EntityId(index as i32 + 1),
            Uuid::from_u128(index as u128 + 1),
            EntityKind::Mob { type_id: 30 },
            wire_fields([x, 64.0, z]),
        );
    }
    let views = store.views();
    for (index, &x) in coordinates.iter().enumerate() {
        let z = coordinates[coordinates.len() - 1 - index];
        let chunk = ChunkPos {
            x: (x.floor() as i32).div_euclid(16),
            z: (z.floor() as i32).div_euclid(16),
        };
        assert!(
            views
                .in_chunk(chunk)
                .any(|view| view.id == EntityId(index as i32 + 1)),
            "membership differs from floor/cast for ({x}, {z})"
        );
    }
}

#[test]
fn idle_clock_changes_keep_durable_and_brain_marks_without_wire_or_motion_work() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    for (name, resident) in [
        ("minecraft:zombie", false),
        ("minecraft:zombie_villager", true),
    ] {
        let type_id = registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap();
        let mut store = EntityStore::default();
        let id = EntityId(7);
        let slot = store.spawn(
            id,
            Uuid::from_u128(7),
            EntityKind::Mob { type_id },
            wire_fields([0.0, 64.0, 0.0]),
        );
        if resident {
            store.row_mut(slot).villager = Some(Default::default());
        }
        let retained = store.views();
        store.take_touched();
        store.brain_changes.clear();
        store.prepare_motion_hints(&registry);
        store.set_no_action_time(slot, 602);
        assert_eq!(store.row(slot).no_action_time, 602);
        assert_eq!(store.touched_ids(), &[id]);
        assert_eq!(store.brain_changes.contains(&id), resident);
        assert!(store.wire_pending.is_empty());
        assert!(!store.wire_marked.iter().any(|marked| *marked));
        assert_eq!(store.dirty_ids_len_for_test(), 0);
        assert_eq!(
            crate::journal::EntityStateImage::from_row(store.body(slot), store.row(slot))
                .no_action_time,
            602
        );
        let unchanged = store.views();
        assert_eq!(unchanged.revision(), retained.revision());
        assert_wire_oracle(&store, &unchanged);
        store.take_touched();
        store.brain_changes.clear();
        store.set_no_action_time(slot, 602);
        assert!(store.touched_ids().is_empty());
        assert!(store.brain_changes.is_empty());
    }
}

/// Everything an entity notification consumer can observe.
fn motion_observation(store: &EntityStore) -> String {
    let mut wire_changed: Vec<_> = store.wire_pending.iter().map(|(id, _)| *id).collect();
    wire_changed.sort_unstable();
    let mut brain_changes: Vec<_> = store.brain_changes.iter().copied().collect();
    brain_changes.sort_unstable();
    let mut villager_chunks: Vec<_> = store.villager_chunks.iter().collect();
    villager_chunks.sort_unstable_by_key(|(chunk, _)| (chunk.x, chunk.z));
    let mut sensor_chunks: Vec<_> = store.sensor_chunks.iter().collect();
    sensor_chunks.sort_unstable_by_key(|(chunk, _)| (chunk.x, chunk.z));
    format!(
        "pos={:?} vel={:?} rot={:?} ground={:?} cold={:?} wire_pending={:?} wire_changed={:?} \
         touched={:?} touched_flag={:?} dirty_ids={:?} dirty_flag={:?} brain={:?} \
         villager_chunks={:?} sensor_chunks={:?}",
        store.pos,
        store.vel,
        store.rot,
        store.ground,
        store.cold,
        store.wire_pending,
        wire_changed,
        store.touched,
        store.touched_flag,
        store.dirty_ids,
        store.dirty_flag,
        brain_changes,
        villager_chunks,
        sensor_chunks,
    )
}

#[test]
fn consolidated_motion_apply_matches_the_ordered_setter_sequence() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let type_of = |name: &str| {
        registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let (cow, villager) = (type_of("minecraft:cow"), type_of("minecraft:villager"));
    struct Case {
        type_id: u32,
        pickup_delay: u32,
        attack_cooldown: u32,
        result: MotionResult,
    }
    let base = MotionResult {
        pos: [1.5, 64.0, 1.5],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        ground: true,
        horizontal_impulse: [0.0; 2],
        air_supply: 300,
        attack_cooldown: None,
    };
    let cases = [
        // chunk crossing mob, air changes
        Case {
            type_id: cow,
            pickup_delay: 0,
            attack_cooldown: 0,
            result: MotionResult {
                pos: [17.5, 64.0, 1.5],
                air_supply: 299,
                ..base
            },
        },
        // chunk crossing resident, air retained
        Case {
            type_id: villager,
            pickup_delay: 0,
            attack_cooldown: 0,
            result: MotionResult {
                pos: [1.5, 64.0, -0.5],
                ..base
            },
        },
        // same-chunk resident, air changes: brain mark
        Case {
            type_id: villager,
            pickup_delay: 0,
            attack_cooldown: 0,
            result: MotionResult {
                pos: [2.0, 64.0, 1.5],
                air_supply: 250,
                ..base
            },
        },
        // only velocity
        Case {
            type_id: cow,
            pickup_delay: 0,
            attack_cooldown: 0,
            result: MotionResult {
                vel: [0.0, -0.08, 0.0],
                ..base
            },
        },
        // nothing changes
        Case {
            type_id: cow,
            pickup_delay: 0,
            attack_cooldown: 0,
            result: base,
        },
        // melee cooldown reset and pickup decrement, unmarked
        Case {
            type_id: cow,
            pickup_delay: 5,
            attack_cooldown: 3,
            result: MotionResult {
                attack_cooldown: Some(20),
                ..base
            },
        },
        // decrement only, plus impulse
        Case {
            type_id: cow,
            pickup_delay: 5,
            attack_cooldown: 3,
            result: MotionResult {
                horizontal_impulse: [0.25, -0.5],
                ..base
            },
        },
        // rotation only; third component mirrors yaw
        Case {
            type_id: cow,
            pickup_delay: 0,
            attack_cooldown: 0,
            result: MotionResult {
                yaw: 90.0,
                pitch: -10.0,
                ..base
            },
        },
        // ground only
        Case {
            type_id: cow,
            pickup_delay: 0,
            attack_cooldown: 0,
            result: MotionResult {
                ground: false,
                ..base
            },
        },
    ];
    let build = || {
        let mut store = EntityStore::default();
        let mut slots = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            let id = EntityId(index as i32 + 1);
            let slot = store.spawn(
                id,
                Uuid::from_u128(id.0 as u128),
                EntityKind::Mob {
                    type_id: case.type_id,
                },
                wire_fields([1.5, 64.0, 1.5]),
            );
            let (body, row) = store.parts_mut(slot);
            body.pickup_delay = case.pickup_delay;
            body.attack_cooldown = case.attack_cooldown;
            body.air_supply = 300;
            if case.type_id == villager {
                row.villager = Some(Default::default());
            }
            slots.push(slot);
        }
        store.take_touched();
        store.wire_marked.fill(false);
        store.wire_removed.clear();
        store.wire_pending.clear();
        store.brain_changes.clear();
        store.dirty_ids.clear();
        store.dirty_flag.fill(false);
        (store, slots)
    };
    let (mut old, slots) = build();
    let (mut new, _) = build();
    assert_eq!(motion_observation(&old), motion_observation(&new));
    // Apply in a scrambled slot order: notification order is the apply order.
    for index in [3, 0, 8, 1, 5, 2, 7, 4, 6] {
        let (slot, case) = (slots[index], &cases[index]);
        let result = &case.result;
        let previous = old.pos(slot);
        old.set_pos(slot, result.pos);
        old.set_vel(slot, result.vel);
        old.set_ground(slot, result.ground);
        old.set_rot(slot, result.yaw, result.pitch);
        let body = old.body(slot);
        let pickup_delay = body.pickup_delay.saturating_sub(1);
        let attack_cooldown = result
            .attack_cooldown
            .unwrap_or_else(|| body.attack_cooldown.saturating_sub(1));
        old.set_motion_state(
            slot,
            result.horizontal_impulse,
            result.air_supply,
            pickup_delay,
            attack_cooldown,
        );
        assert_eq!(new.apply_motion_result(slot, result), previous);
        assert_eq!(
            motion_observation(&old),
            motion_observation(&new),
            "case {index}"
        );
    }
    assert!(new.touched.contains(&EntityId(1)), "crossing lists journal");
    assert!(new.brain_changes.contains(&EntityId(3)));
    // A resident chunk crossing reindexes through the brain-body notification.
    assert!(new.brain_changes.contains(&EntityId(2)));
    assert!(!new.brain_changes.contains(&EntityId(1)));
    assert!(new.touched_flag[0] && !new.touched_flag[3]);
    assert_eq!(new.rot[7], [90.0, -10.0, 90.0]);
}

#[test]
fn resident_slots_are_sorted_unique_and_skip_removed_ids() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let villager = registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let mut store = EntityStore::default();
    let mut ids = Vec::new();
    for index in 0..6 {
        let id = EntityId(100 - index);
        let type_id = if index % 2 == 0 { villager } else { 30 };
        store.spawn(
            id,
            Uuid::from_u128(id.0 as u128),
            EntityKind::Mob { type_id },
            wire_fields([f64::from(index) * 20.0, 64.0, 0.0]),
        );
        ids.push((id, type_id == villager));
    }
    // Chunk crossing and removal (swap-remove compaction) keep the index exact.
    let moved = store.slot_of(ids[2].0).unwrap();
    store.set_pos(moved, [-40.0, 64.0, 40.0]);
    let removed = store.slot_of(ids[0].0).unwrap();
    store.remove(removed);
    let mut slots = Vec::new();
    store.resident_slots_sorted(&mut slots);
    let oracle: Vec<_> = store
        .slots()
        .filter(|&slot| {
            matches!(store.body(slot).kind, EntityKind::Mob { type_id } if type_id == villager)
        })
        .collect();
    assert_eq!(slots, oracle);
    assert_eq!(slots.len(), 2);
}

#[test]
#[ignore = "algorithm-level release timing; requires Main's exclusive perf GO"]
fn motion_writeback_and_resident_index_micro() {
    const ROWS: usize = 100_000;
    const VILLAGERS: usize = 64;
    const ITERATIONS: usize = 20;
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let type_of = |name: &str| {
        registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let (cow, villager) = (type_of("minecraft:cow"), type_of("minecraft:villager"));
    let mut store = EntityStore::default();
    let mut slots = Vec::with_capacity(ROWS);
    for index in 0..ROWS {
        let id = EntityId(index as i32 + 1);
        let resident = index % (ROWS / VILLAGERS) == 0;
        let slot = store.spawn(
            id,
            Uuid::from_u128(id.0 as u128),
            EntityKind::Mob {
                type_id: if resident { villager } else { cow },
            },
            wire_fields([
                (index % 1000) as f64 * 0.7,
                64.0,
                (index / 1000) as f64 * 0.7,
            ]),
        );
        if resident {
            store.row_mut(slot).villager = Some(Default::default());
        }
        slots.push(slot);
    }
    let results = |phase: f64| -> Vec<MotionResult> {
        slots
            .iter()
            .map(|&slot| {
                let pos = store.pos(slot);
                MotionResult {
                    pos: [pos[0] + phase, pos[1], pos[2] + phase * 0.5],
                    vel: [phase * 0.1, -0.08, 0.0],
                    yaw: (slot.0 % 360) as f32 + phase as f32,
                    pitch: 1.0,
                    ground: slot.0 % 3 == 0,
                    horizontal_impulse: [0.0; 2],
                    air_supply: 300,
                    attack_cooldown: None,
                }
            })
            .collect()
    };
    // Alternate forward/back so every iteration changes every column.
    let (forward, back) = (results(0.05), results(0.0));
    let reset = |store: &mut EntityStore| {
        store.take_touched();
        store.wire_marked.fill(false);
        store.wire_removed.clear();
        store.wire_pending.clear();
        store.brain_changes.clear();
        store.dirty_ids.clear();
        store.dirty_flag.fill(false);
    };
    type Writeback = fn(&mut EntityStore, EntitySlot, &MotionResult);
    let old: Writeback = |store, slot, result| {
        store.set_pos(slot, result.pos);
        store.set_vel(slot, result.vel);
        store.set_ground(slot, result.ground);
        store.set_rot(slot, result.yaw, result.pitch);
        let body = store.body(slot);
        let (pickup, attack) = (
            body.pickup_delay.saturating_sub(1),
            body.attack_cooldown.saturating_sub(1),
        );
        store.set_motion_state(
            slot,
            result.horizontal_impulse,
            result.air_supply,
            pickup,
            attack,
        );
    };
    let new: Writeback = |store, slot, result| {
        store.apply_motion_result(slot, result);
    };
    let mut elapsed = [0.0_f64; 4];
    for (leg, (label, run)) in [
        ("B_new", new),
        ("A_old", old),
        ("B_new", new),
        ("A_old", old),
    ]
    .into_iter()
    .enumerate()
    {
        let run = std::hint::black_box(run);
        let mut total = std::time::Duration::ZERO;
        for iteration in 0..ITERATIONS {
            let results = if iteration % 2 == 0 { &forward } else { &back };
            reset(&mut store);
            let start = std::time::Instant::now();
            for (&slot, result) in slots.iter().zip(results) {
                run(std::hint::black_box(&mut store), slot, result);
            }
            total += start.elapsed();
        }
        elapsed[leg] = total.as_secs_f64() * 1e3 / ITERATIONS as f64;
        println!(
            "MOTION_MICRO leg={leg} {label} ms_per_100k_apply={:.3}",
            elapsed[leg]
        );
    }
    let mut targets = Vec::new();
    let mut resident_ms = [0.0_f64; 4];
    for (leg, label) in ["B_index", "A_scan", "B_index", "A_scan"]
        .into_iter()
        .enumerate()
    {
        let start = std::time::Instant::now();
        for _ in 0..ITERATIONS * 5 {
            targets = if label == "B_index" {
                let mut found = Vec::new();
                store.resident_slots_sorted(&mut found);
                found
                    .into_iter()
                    .filter(|&slot| {
                        let body = store.body(slot);
                        let row = store.row(slot);
                        body.alive
                            && row
                                .villager
                                .as_ref()
                                .is_some_and(|v| v.conversion_time.is_none())
                    })
                    .map(|slot| (store.body(slot).id, store.pos(slot)))
                    .collect()
            } else {
                store
                    .slots()
                    .filter(|slot| {
                        store.body(*slot).alive
                            && store
                                .row(*slot)
                                .villager
                                .as_ref()
                                .is_some_and(|v| v.conversion_time.is_none())
                    })
                    .map(|slot| (store.body(slot).id, store.pos(slot)))
                    .collect()
            };
            std::hint::black_box(&targets);
        }
        resident_ms[leg] = start.elapsed().as_secs_f64() * 1e3 / (ITERATIONS * 5) as f64;
        println!(
            "RESIDENT_MICRO leg={leg} {label} ms={:.4} n={}",
            resident_ms[leg],
            targets.len()
        );
    }
    println!(
        "{}",
        serde_json::json!({
            "scope": "algorithm-level EntityStore writeback + villager list, not whole-tick",
            "rows": ROWS,
            "order": "B,A,B,A",
            "apply_new_ms": (elapsed[0] + elapsed[2]) / 2.0,
            "apply_old_ms": (elapsed[1] + elapsed[3]) / 2.0,
            "villagers_index_ms": (resident_ms[0] + resident_ms[2]) / 2.0,
            "villagers_scan_ms": (resident_ms[1] + resident_ms[3]) / 2.0,
        })
    );
}

fn assert_tick_membership_oracle(store: &EntityStore) {
    // Independent full scans retain the pre-optimization definition and order.
    for type_id in [0, 7, 30, 999, u32::MAX] {
        let expected: Vec<_> = store
            .slots()
            .filter(|&slot| {
                matches!(store.body(slot).kind, EntityKind::Mob { type_id: actual } if actual == type_id)
            })
            .collect();
        assert_eq!(store.mob_slots(type_id).collect::<Vec<_>>(), expected);
    }
    let expected: Vec<_> = store
        .slots()
        .filter(|&slot| matches!(store.body(slot).kind, EntityKind::Mob { type_id } if type_id == 7 || type_id == 30))
        .collect();
    assert_eq!(store.mob_slots_matching(|id| id == 7 || id == 30), expected);
    let riders: Vec<_> = store
        .slots()
        .filter(|&slot| store.row(slot).entity_life.mount.rider.is_some())
        .collect();
    let leashed: Vec<_> = store
        .slots()
        .filter(|&slot| store.row(slot).entity_life.leash.is_some())
        .collect();
    assert_eq!(store.rider_slots().collect::<Vec<_>>(), riders);
    assert_eq!(store.leashed_slots().collect::<Vec<_>>(), leashed);
}

#[test]
fn tick_memberships_match_full_scans_through_compaction_kind_changes_and_transfer() {
    let mut source = EntityStore::default();
    let mut destination = EntityStore::default();
    // Reverse IDs ensure an ID-ordered index cannot accidentally pass.
    for (index, type_id) in [7, 30, 7, 999, 30, u32::MAX].into_iter().enumerate() {
        let id = EntityId(100 - index as i32);
        let slot = source.spawn(
            id,
            Uuid::from_u128(id.0 as u128),
            EntityKind::Mob { type_id },
            wire_fields([0.0, 64.0, 0.0]),
        );
        if index % 2 == 0 {
            source.set_rider(slot, Some(Uuid::from_u128(9000 + index as u128)));
        }
        if index % 3 == 0 {
            source.set_leash(
                slot,
                Some(crate::leash::Holder::Fence([index as i32, 64, 0])),
            );
        }
        assert_tick_membership_oracle(&source);
    }
    source.remove(EntitySlot(1)); // Last row swaps into the middle.
    assert_tick_membership_oracle(&source);
    source.set_kind(EntitySlot(0), EntityKind::Mob { type_id: 30 });
    source.set_rider(EntitySlot(0), None);
    source.set_leash(EntitySlot(0), None);
    assert_tick_membership_oracle(&source);
    let mut life = crate::entity_life::EntityLifeState::default();
    life.mount.rider = Some(Uuid::from_u128(42));
    life.leash = Some(crate::leash::Holder::Entity(Uuid::from_u128(43)));
    source.set_entity_life(EntitySlot(1), life);
    assert_tick_membership_oracle(&source);
    source.transfer_to(EntitySlot(1), &mut destination, [16.0, 64.0, 0.0], [0.0; 2]);
    assert_tick_membership_oracle(&source);
    assert_tick_membership_oracle(&destination);
    destination.set_entity_life(EntitySlot(0), Default::default());
    assert_tick_membership_oracle(&destination);
    source.set_kind(EntitySlot(0), EntityKind::Item { type_id: 7 });
    assert_tick_membership_oracle(&source);
    while !source.is_empty() {
        source.remove(EntitySlot(0));
        assert_tick_membership_oracle(&source);
    }
    destination.transfer_to(EntitySlot(0), &mut source, [0.0, 64.0, 0.0], [0.0; 2]);
    assert_tick_membership_oracle(&source);
    assert_tick_membership_oracle(&destination);
}

#[test]
#[ignore = "release B,A,B,A membership micro; requires Main perf GO"]
fn tick_membership_release_micro() {
    const ROWS: usize = 105_000;
    const ITERATIONS: usize = 100;
    let mut store = EntityStore::default();
    for index in 0..ROWS {
        let id = EntityId(index as i32 + 1);
        let slot = store.spawn(
            id,
            Uuid::from_u128(id.0 as u128),
            EntityKind::Mob {
                type_id: (index % 16) as u32,
            },
            wire_fields([index as f64, 64.0, 0.0]),
        );
        if index.is_multiple_of(8192) {
            store.set_rider(slot, Some(Uuid::from_u128(index as u128 + 1)));
        }
        if index.is_multiple_of(4096) {
            store.set_leash(
                slot,
                Some(crate::leash::Holder::Fence([index as i32, 64, 0])),
            );
        }
    }
    let old = || {
        let store = std::hint::black_box(&store);
        let mut checksum = 0u64;
        for kind in [0, 2, 3, 5, 7, 11] {
            for slot in store.slots().filter(|&slot| {
                matches!(store.body(slot).kind, EntityKind::Mob { type_id } if type_id == kind)
            }) {
                checksum = checksum.wrapping_add(u64::from(slot.0));
            }
        }
        for slot in store
            .slots()
            .filter(|&slot| store.row(slot).entity_life.mount.rider.is_some())
        {
            checksum = checksum.wrapping_add(u64::from(slot.0));
        }
        for slot in store
            .slots()
            .filter(|&slot| store.row(slot).entity_life.leash.is_some())
        {
            checksum = checksum.wrapping_add(u64::from(slot.0));
        }
        checksum
    };
    let new = || {
        let store = std::hint::black_box(&store);
        let mut checksum = 0u64;
        for kind in [0, 2, 3, 5, 7, 11] {
            for slot in store.mob_slots_matching(|type_id| type_id == kind) {
                checksum = checksum.wrapping_add(u64::from(slot.0));
            }
        }
        // Mutable callers snapshot these sparse lists before applying effects.
        let riders: Vec<_> = store.rider_slots().collect();
        for slot in std::hint::black_box(riders) {
            checksum = checksum.wrapping_add(u64::from(slot.0));
        }
        let leashed: Vec<_> = store.leashed_slots().collect();
        for slot in std::hint::black_box(leashed) {
            checksum = checksum.wrapping_add(u64::from(slot.0));
        }
        checksum
    };
    assert_eq!(old(), new());
    for indexed in [true, false, true, false] {
        let start = std::time::Instant::now();
        for _ in 0..ITERATIONS {
            std::hint::black_box(if indexed { new() } else { old() });
        }
        println!(
            "membership_micro path={} rows={ROWS} iterations={ITERATIONS} ms_per_stage={:.6}",
            if indexed { "B-index" } else { "A-scan" },
            start.elapsed().as_secs_f64() * 1000.0 / ITERATIONS as f64
        );
    }
}

#[test]
fn dense_wire_marks_match_identity_set_through_compaction_and_rebirth() {
    let mut store = EntityStore::default();
    let mut reference = rustc_hash::FxHashSet::default();
    let mut expected = Vec::new();
    let mark = |reference: &mut rustc_hash::FxHashSet<EntityId>,
                expected: &mut Vec<(EntityId, EntitySlot)>,
                id,
                slot| {
        if reference.insert(id) {
            expected.push((id, slot));
        }
    };
    for index in 1..=32 {
        let id = EntityId(index);
        let slot = store.spawn(
            id,
            Uuid::from_u128(index as u128),
            EntityKind::Mob { type_id: 30 },
            wire_fields([f64::from(index), 64.0, 0.0]),
        );
        mark(&mut reference, &mut expected, id, slot);
    }
    for iteration in 0..256 {
        let slot = EntitySlot((iteration * 17 % store.len()) as u32);
        let id = store.body(slot).id;
        if iteration % 3 == 0 {
            // remove() notifies before compaction; a reborn identity keeps its
            // original queued hint just as the old identity-set path did.
            store.remove(slot);
            mark(&mut reference, &mut expected, id, slot);
            let reborn = store.spawn(
                id,
                Uuid::from_u128(1000 + iteration as u128),
                EntityKind::Mob { type_id: 30 },
                wire_fields([iteration as f64, 65.0, 0.0]),
            );
            mark(&mut reference, &mut expected, id, reborn);
        } else {
            let _ = store.row_mut(slot);
            mark(&mut reference, &mut expected, id, slot);
        }
        assert_eq!(store.wire_pending, expected, "iteration={iteration}");
        if iteration % 7 == 0 {
            let views = store.views();
            assert_wire_oracle(&store, &views);
            reference.clear();
            expected.clear();
            assert!(!store.wire_marked.iter().any(|&marked| marked));
            assert!(store.wire_removed.is_empty());
        }
    }
    let views = store.views();
    assert_wire_oracle(&store, &views);
}

#[test]
#[ignore = "algorithm-level release timing; requires Main's perf lane"]
fn dense_wire_mark_dedup_micro() {
    const ROWS: usize = 100_000;
    const ITERATIONS: usize = 32;
    let mut store = EntityStore::default();
    for index in 0..ROWS {
        store.spawn(
            EntityId(index as i32 + 1),
            Uuid::from_u128(index as u128 + 1),
            EntityKind::Mob { type_id: 30 },
            wire_fields([index as f64, 64.0, 0.0]),
        );
    }
    let mut reference = rustc_hash::FxHashSet::default();
    reference.reserve(ROWS);
    for (label, dense) in [
        ("B_dense", true),
        ("A_identity_set", false),
        ("B_dense", true),
        ("A_identity_set", false),
    ] {
        let start = std::time::Instant::now();
        for _ in 0..ITERATIONS {
            store.wire_pending.clear();
            store.wire_marked.fill(false);
            reference.clear();
            for index in 0..ROWS {
                let slot = EntitySlot(index as u32);
                let id = store.body(slot).id;
                if dense {
                    store.entity_changed(std::hint::black_box(id), false, slot);
                } else if reference.insert(std::hint::black_box(id)) {
                    store.wire_pending.push((id, slot));
                }
            }
            assert_eq!(std::hint::black_box(&store.wire_pending).len(), ROWS);
        }
        println!(
            "WIRE_MARK_MICRO {label} rows={ROWS} ns_per_batch={:.0}",
            start.elapsed().as_nanos() as f64 / ITERATIONS as f64
        );
    }
}

#[test]
fn armor_motion_flag_matches_cold_predicate_after_equipment_and_lifecycle_changes() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let armor_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_chestplate").unwrap())
        .unwrap();
    let armor = mc_data::ItemStack::new(armor_id, 1);
    let mut source = EntityStore::default();
    let mut destination = EntityStore::default();
    let mut armored = wire_fields([0.5, 64.0, 0.5]);
    Arc::make_mut(&mut armored.appearance).equipment[4] = Some(armor.clone());
    let first = source.spawn(
        EntityId(1),
        Uuid::from_u128(1),
        EntityKind::Mob {
            type_id: registry.cow_type_id,
        },
        armored.clone(),
    );
    let second = source.spawn(
        EntityId(2),
        Uuid::from_u128(2),
        EntityKind::Mob {
            type_id: registry.cow_type_id,
        },
        wire_fields([1.5, 64.0, 0.5]),
    );
    source.spawn(
        EntityId(3),
        Uuid::from_u128(3),
        EntityKind::Player {
            connection: crate::boundary::ConnectionId(3),
        },
        armored.clone(),
    );
    source.spawn(
        EntityId(4),
        Uuid::from_u128(4),
        EntityKind::Item { type_id: 0 },
        armored,
    );
    let check = |store: &mut EntityStore| {
        store.prepare_motion_hints(&registry);
        for slot in store.slots() {
            let body = store.body(slot);
            let row = store.row(slot);
            let expected = !matches!(body.kind, EntityKind::Player { .. })
                && row.appearance.equipment[2..6].iter().any(Option::is_some);
            assert_eq!(
                store.motion_flags(slot) & MOTION_ARMORED != 0,
                expected,
                "id={:?}",
                body.id
            );
        }
    };
    check(&mut source);
    Arc::make_mut(&mut source.row_mut(second).appearance).equipment[4] = Some(armor);
    Arc::make_mut(&mut source.row_mut_unmarked(first).appearance).equipment[2..6].fill(None);
    check(&mut source);
    source.transfer_to(second, &mut destination, [32.5, 64.0, 0.5], [0.0; 2]);
    source.remove(first);
    check(&mut source);
    check(&mut destination);
}

#[test]
fn quiet_timer_flag_matches_cold_predicate_after_all_motion_write_paths() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let mut store = EntityStore::default();
    let slot = store.spawn(
        EntityId(1),
        Uuid::from_u128(1),
        EntityKind::Mob {
            type_id: registry.cow_type_id,
        },
        wire_fields([0.5, 64.0, 0.5]),
    );
    let check = |store: &mut EntityStore| {
        store.prepare_motion_hints(&registry);
        let body = store.body(slot);
        let expected = body.pickup_delay == 0
            && body.attack_cooldown == 0
            && store.horizontal_impulse(slot) == [0.0; 2];
        assert_eq!(
            store.motion_flags(slot) & MOTION_QUIET_TIMERS != 0,
            expected,
        );
    };
    check(&mut store);
    store.body_mut(slot).pickup_delay = 1;
    check(&mut store);
    store.body_mut(slot).attack_cooldown = 2;
    check(&mut store);
    store.set_motion_state(slot, [0.0; 2], 300, 0, 0);
    check(&mut store);
    store.set_brain_impulse(slot, [1.0, 0.0]);
    check(&mut store);
    let mut result = MotionResult {
        pos: store.pos(slot),
        vel: store.vel(slot),
        yaw: store.rot(slot)[0],
        pitch: store.rot(slot)[1],
        ground: store.ground(slot),
        horizontal_impulse: [0.0; 2],
        air_supply: 300,
        attack_cooldown: Some(2),
    };
    store.apply_motion_result(slot, &result);
    check(&mut store);
    result.attack_cooldown = None;
    for _ in 0..2 {
        store.apply_motion_result(slot, &result);
        check(&mut store);
    }
    assert!(store.motion_flags(slot) & MOTION_QUIET_TIMERS != 0);
}
