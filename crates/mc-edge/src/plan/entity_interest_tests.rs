use super::*;

fn type_id_of(name: &str) -> u32 {
    mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}

fn plan_entities_for(
    out: &TickOutput,
    viewer: &mut ViewerState,
    credit: usize,
) -> Vec<PlannedPacket> {
    let index = TickPlanIndex::with_previous(out, viewer.previous_entities());
    let mut scratch = PlanScratch::default();
    let mut packets = Vec::new();
    let mut credit = credit;
    plan_entities(
        &out.snapshot,
        viewer,
        4,
        &mut scratch,
        &index,
        &mut packets,
        &mut credit,
    );
    packets
}

#[test]
fn unchanged_revision_has_no_entity_work_after_delivery() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let out = output(
        vec![],
        (10..110).map(|id| entity(id, [9.0, 64.0, 8.0])).collect(),
        ChangeSet::default(),
    );
    assert!(
        plan_entities_for(&out, &mut viewer, usize::MAX)
            .iter()
            .any(|packet| packet.id == AddEntity::ID)
    );
    let index = TickPlanIndex::with_previous(&out, viewer.previous_entities());
    let mut interest = std::mem::take(&mut viewer.entity_interest);
    let mut scratch = PlanScratch::default();
    interest.prepare(
        &out.snapshot,
        &mut viewer,
        4,
        &index,
        &mut scratch,
        usize::MAX,
    );
    assert!(
        scratch.visible.is_empty(),
        "quiet rows must not be visited again"
    );
    assert!(scratch.spawns.is_empty());
    assert!(scratch.removed.is_empty());
    assert_eq!(viewer.tracked_entities.len(), 100);
}

#[test]
fn movement_waiting_for_its_phase_survives_an_unchanged_snapshot() {
    let mut mob = entity(10, [9.0, 64.0, 8.0]);
    mob.type_id = type_id_of("minecraft:cow");
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let mut baseline = mob.clone();
    baseline.position[0] -= 1.0;
    viewer
        .tracked_entities
        .insert(mob.id, TrackedEntity::new(&baseline, 0));
    let mut out = output(vec![], vec![mob.clone()], ChangeSet::default());
    let interval = u64::from(
        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(mob.type_id)
            .unwrap()
            .update_interval,
    );
    Arc::get_mut(&mut out.snapshot).unwrap().tick = (0..interval)
        .find(|&tick| !entity_movement_due(&mob, tick, None))
        .unwrap();
    assert!(plan_entities_for(&out, &mut viewer, 32).is_empty());
    for _ in 0..interval {
        Arc::get_mut(&mut out.snapshot).unwrap().tick += 1;
        let packets = plan_entities_for(&out, &mut viewer, 32);
        if packets.iter().any(|packet| packet.id == MoveEntityPos::ID) {
            assert_eq!(viewer.tracked_entities[&mob.id].baseline.0, mob.position);
            return;
        }
    }
    panic!("stopped entity's movement debt was lost");
}

#[test]
fn credit_blocked_movement_is_retained_without_new_entity_changes() {
    let mut mob = entity(10, [9.0, 64.0, 8.0]);
    mob.type_id = type_id_of("minecraft:cow");
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let mut baseline = mob.clone();
    baseline.position[0] -= 1.0;
    viewer
        .tracked_entities
        .insert(mob.id, TrackedEntity::new(&baseline, 0));
    let mut out = output(vec![], vec![mob.clone()], ChangeSet::default());
    while !entity_movement_due(&mob, out.snapshot.tick, None) {
        Arc::get_mut(&mut out.snapshot).unwrap().tick += 1;
    }
    assert!(plan_entities_for(&out, &mut viewer, 0).is_empty());
    assert_eq!(
        viewer.tracked_entities[&mob.id].baseline.0,
        baseline.position
    );
    Arc::get_mut(&mut out.snapshot).unwrap().tick += 3;
    assert!(
        plan_entities_for(&out, &mut viewer, 32)
            .iter()
            .any(|packet| packet.id == MoveEntityPos::ID)
    );
    assert_eq!(viewer.tracked_entities[&mob.id].baseline.0, mob.position);
}

#[test]
fn walking_inside_negative_chunk_reconciles_exact_enter_leave_strips() {
    let mut viewer = viewer_at(1, [-0.25, 64.0, -0.25]);
    for x in -7..=5 {
        for z in -7..=5 {
            viewer.loaded_chunks.insert((x, z), 0);
        }
    }
    let positions = [
        [95.8, 64.0, 8.0],
        [-96.2, 64.0, 8.0],
        [8.0, 64.0, 95.8],
        [8.0, 64.0, -96.2],
        [63.8, 64.0, 8.0],
        [-64.2, 64.0, 8.0],
        [8.0, 64.0, 63.8],
        [8.0, 64.0, -64.2],
        [8.0, 64.0, 8.0],
    ];
    let mut rows = Vec::new();
    for (i, position) in positions.into_iter().enumerate() {
        let mut row = entity(10 + i as i32, position);
        row.type_id = type_id_of(if i < 4 {
            "minecraft:cow"
        } else {
            "minecraft:snowball"
        });
        rows.push(row);
    }
    let out = output(vec![], rows, ChangeSet::default());
    for position in [
        [-0.25, 64.0, -0.25],
        [-0.1, 64.0, -0.25],
        [-0.1, 64.0, -0.1],
        [-0.25, 64.0, -0.25],
    ] {
        viewer.position = position;
        let index = TickPlanIndex::with_previous(&out, viewer.previous_entities());
        let mut scratch = PlanScratch::default();
        let mut packets = Vec::new();
        let mut credit = usize::MAX;
        plan_entities(
            &out.snapshot,
            &mut viewer,
            6,
            &mut scratch,
            &index,
            &mut packets,
            &mut credit,
        );
        let expected: HashSet<_> = out
            .snapshot
            .entities
            .iter()
            .filter(|row| entity_trackable(row, &viewer, 6))
            .map(|row| row.id)
            .collect();
        assert_eq!(
            viewer
                .tracked_entities
                .keys()
                .copied()
                .collect::<HashSet<_>>(),
            expected,
            "viewer {position:?}"
        );
    }
}

#[test]
fn shrinking_delivery_debt_resumes_at_first_blocked_entity() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let mut rows = Vec::new();
    for id in [10, 13, 16] {
        let mut mob = entity(id, [9.0, 64.0, 8.0]);
        mob.type_id = type_id_of("minecraft:cow");
        let mut baseline = mob.clone();
        baseline.position[0] -= 1.0;
        viewer
            .tracked_entities
            .insert(mob.id, TrackedEntity::new(&baseline, 0));
        rows.push(mob);
    }
    let mut out = output(vec![], rows, ChangeSet::default());
    while !entity_movement_due(
        out.snapshot.entities.get(EntityId(10)).unwrap(),
        out.snapshot.tick,
        None,
    ) {
        Arc::get_mut(&mut out.snapshot).unwrap().tick += 1;
    }
    let mut sent = Vec::new();
    for _ in 0..3 {
        let packets = plan_entities_for(&out, &mut viewer, 1);
        assert_eq!(packets.len(), 1);
        let movement = MoveEntityPos::decode(&mut &body_of(&packets[0])[..]).unwrap();
        sent.push(movement.entity_id);
    }
    assert_eq!(sent, [10, 13, 16]);
}

#[test]
fn changed_motion_preserves_deadline_appearance_and_credit_cadence() {
    use mc_core::blueprint::MovementClass;
    use mc_core::entities::{ColdFields, EntityKind, EntityStore};

    let mut store = EntityStore::default();
    let id = EntityId(10);
    let type_id = type_id_of("minecraft:cow");
    let slot = store.spawn(
        id,
        uuid::Uuid::from_u128(10),
        EntityKind::Mob { type_id },
        ColdFields {
            pos: [9.0, 64.0, 8.0],
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
        },
    );
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let mut out = output(vec![], vec![], ChangeSet::default());
    let snapshot = Arc::get_mut(&mut out.snapshot).unwrap();
    snapshot.entities = store.views();
    snapshot.tick = (0..10)
        .find(|&tick| entity_movement_due(snapshot.entities.get(id).unwrap(), tick, None))
        .unwrap();
    assert!(!plan_entities_for(&out, &mut viewer, 32).is_empty());
    assert!(plan_entities_for(&out, &mut viewer, 32).is_empty());

    store.set_pos(slot, [9.25, 64.0, 8.0]);
    let snapshot = Arc::get_mut(&mut out.snapshot).unwrap();
    snapshot.tick += 1;
    snapshot.entities = store.views();
    assert!(!entity_movement_due(
        snapshot.entities.get(id).unwrap(),
        snapshot.tick,
        None
    ));
    let index = TickPlanIndex::with_previous(&out, viewer.previous_entities());
    let mut interest = std::mem::take(&mut viewer.entity_interest);
    let mut scratch = PlanScratch::default();
    interest.prepare(&out.snapshot, &mut viewer, 4, &index, &mut scratch, 32);
    assert_eq!(
        scratch.visible,
        [id],
        "deadline arming must follow canonical delivery traversal"
    );
    viewer.entity_interest = interest;
    assert!(plan_entities_for(&out, &mut viewer, 32).is_empty());

    // Appearance still bypasses the movement cadence, including when the
    // same entity already has a future movement deadline.
    Arc::make_mut(&mut store.row_mut(slot).appearance).shared_flags ^= 1;
    Arc::get_mut(&mut out.snapshot).unwrap().entities = store.views();
    let packets = plan_entities_for(&out, &mut viewer, 32);
    assert!(
        packets
            .iter()
            .any(|packet| packet.id == ClientboundSetEntityData::ID)
    );
    assert!(!packets.iter().any(|packet| packet.id == MoveEntityPos::ID));

    for _ in 0..10 {
        Arc::get_mut(&mut out.snapshot).unwrap().tick += 1;
        let packets = plan_entities_for(&out, &mut viewer, 32);
        if packets.iter().any(|packet| packet.id == MoveEntityPos::ID) {
            assert_eq!(viewer.tracked_entities[&id].baseline.0, [9.25, 64.0, 8.0]);
            break;
        }
    }
    assert_eq!(
        viewer.tracked_entities[&id].baseline.0,
        [9.25, 64.0, 8.0],
        "queued motion must be sent even if the entity stops changing"
    );

    // A newly changed row first seen on its due tick does not already
    // have a queued deadline. Credit rejection must not invent a ready
    // deadline that sends it on the next, non-due tick.
    let interval = u64::from(
        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
            .unwrap()
            .update_interval,
    );
    store.set_pos(slot, [9.5, 64.0, 8.0]);
    let snapshot = Arc::get_mut(&mut out.snapshot).unwrap();
    snapshot.tick += interval;
    snapshot.entities = store.views();
    assert!(entity_movement_due(
        snapshot.entities.get(id).unwrap(),
        snapshot.tick,
        None
    ));
    assert!(plan_entities_for(&out, &mut viewer, 0).is_empty());
    store.set_pos(slot, [9.75, 64.0, 8.0]);
    let snapshot = Arc::get_mut(&mut out.snapshot).unwrap();
    snapshot.tick += 1;
    snapshot.entities = store.views();
    assert!(plan_entities_for(&out, &mut viewer, 32).is_empty());
    assert_eq!(viewer.tracked_entities[&id].baseline.0, [9.25, 64.0, 8.0]);
}
