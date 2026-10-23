use super::*;

fn resting_mob(name: &str) -> (TickState, EntitySlot) {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }];
    state
        .world
        .ensure_blocking(&chunks.into_iter().map(|pos| (pos, 0)).collect());
    state
        .world
        .set_active_chunks(chunks.into_iter().collect(), 1);
    let stone = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    for x in 8..=16 {
        state.world.set_block(x, 100, 8, stone);
    }
    state.world.fold_overlay();
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap();
    let entity = state.spawn_mob(type_id, [8.5, 101.0, 8.5], 10.0);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_vel(slot, [0.0; 3]);
    state.entities.set_ground(slot, true);
    state.tick = 1;
    if state
        .tick
        .wrapping_add(entity.0 as u64)
        .is_multiple_of(MOB_FAR_PERIOD)
    {
        state.tick += 1;
    }
    state.entity_phase_revision = state.world.revision_now();
    (state, slot)
}

fn resting_cow() -> (TickState, EntitySlot) {
    resting_mob("minecraft:cow")
}

#[test]
fn pickup_lanes_use_current_liveness_delay_and_post_move_slot_order() {
    let (mut state, player_slot) = resting_cow();
    let player = state.entities.body(player_slot).id;
    state.entities.set_kind(
        player_slot,
        EntityKind::Player {
            connection: crate::boundary::ConnectionId(1),
        },
    );
    let pos = state.entities.pos(player_slot);
    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(spawn_item_entity(
            &mut state,
            pos,
            mc_data::ItemStack::new(1, 1),
        ));
    }
    let dead = state.entities.slot_of(ids[0]).unwrap();
    state.entities.body_mut(dead).alive = false;
    state.entities.body_mut(dead).pickup_delay = 0;
    let delayed = state.entities.slot_of(ids[1]).unwrap();
    state.entities.body_mut(delayed).pickup_delay = 2;
    let first = state.entities.slot_of(ids[2]).unwrap();
    state.entities.body_mut(first).pickup_delay = 1;
    let last = state.entities.slot_of(ids[3]).unwrap();
    state.entities.body_mut(last).pickup_delay = 0;
    // Fresh rows have zero motion flags. Admission still sees both kinds.
    let mut intents = Vec::new();
    collect_pickup_intents(&state, &mut intents);
    assert_eq!(intents.len(), 1);
    assert!(matches!(intents[0].kind, IntentKind::PickupItem { item, .. } if item == ids[3]));
    // Motion writeback makes the earlier item eligible in this very tick.
    state.entities.apply_motion_result(
        first,
        &crate::entities::MotionResult {
            pos,
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            ground: true,
            horizontal_impulse: [0.0; 2],
            air_supply: 300,
            attack_cooldown: None,
        },
    );
    intents.clear();
    collect_pickup_intents(&state, &mut intents);
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].actor, player);
    assert!(matches!(intents[0].kind, IntentKind::PickupItem { item, .. } if item == ids[2]));
    // Removing the dead item compacts the last item ahead of the earlier one.
    state.entities.remove(dead);
    intents.clear();
    collect_pickup_intents(&state, &mut intents);
    assert_eq!(intents.len(), 1);
    assert!(matches!(intents[0].kind, IntentKind::PickupItem { item, .. } if item == ids[3]));
    state.entities.body_mut(player_slot).alive = false;
    intents.clear();
    collect_pickup_intents(&state, &mut intents);
    assert!(intents.is_empty());
}

fn can_early_rest(state: &mut TickState, slot: EntitySlot) -> bool {
    !phase_admitted(state, slot, MOB_FAR_PERIOD as u8)
}

#[test]
fn early_rest_avoids_wide_rows_but_advances_motion_clocks() {
    let (mut state, slot) = resting_cow();
    state.entities.body_mut(slot).pickup_delay = 3;
    state.entities.body_mut(slot).attack_cooldown = 4;
    let pos = state.entities.pos(slot);
    assert!(can_early_rest(&mut state, slot));
    assert!(run_entity_phase(&mut state, true).is_empty());
    assert_eq!(state.entities.pos(slot), pos);
    assert_eq!(state.entities.vel(slot), [0.0; 3]);
    assert!(state.entities.ground(slot));
    assert_eq!(state.entities.body(slot).pickup_delay, 2);
    assert_eq!(state.entities.body(slot).attack_cooldown, 3);
    assert!(state.entity_phase_scratch.rows.is_empty());
    assert_eq!(
        state.entity_phase_scratch.writes.capacity(),
        0,
        "no worker row was materialized"
    );
    let capacity = state.entity_phase_scratch.resting.capacity();
    assert!(capacity > 0);
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(state.entity_phase_scratch.resting.capacity(), capacity);
    assert_eq!(state.entities.body(slot).pickup_delay, 1);
    assert_eq!(state.entities.body(slot).attack_cooldown, 2);
}

#[test]
fn early_rest_wakes_for_an_edit_in_the_neighboring_footprint_chunk() {
    let (mut state, slot) = resting_cow();
    state.entities.set_pos(slot, [15.9, 101.0, 8.5]);
    assert!(can_early_rest(&mut state, slot));
    state.world.set_block(16, 100, 8, state.registry.states.air);
    state.world.fold_overlay();
    assert!(!can_early_rest(&mut state, slot));
    let _ = run_entity_phase(&mut state, true);
    assert!(
        state.entity_phase_scratch.writes.capacity() > 0,
        "the changed footprint ran its normal worker"
    );
}

#[test]
fn early_rest_retains_breeding_partners_and_suppressed_ai_in_the_snapshot() {
    let (mut state, slot) = resting_cow();
    state.entities.row_mut(slot).love = 100;
    assert!(!can_early_rest(&mut state, slot));
    let _ = run_entity_phase(&mut state, true);
    assert!(state.entity_phase_scratch.writes.capacity() > 0);
    state.entities.row_mut(slot).love = 0;
    state.entities.row_mut(slot).controls.no_ai = true;
    assert!(!can_early_rest(&mut state, slot));
    state.entities.row_mut(slot).controls.no_ai = false;
    state.entities.body_mut(slot).hurt_at = Some(state.tick);
    assert!(!can_early_rest(&mut state, slot));
}

#[test]
fn column_cache_does_not_keep_inactive_clocks_frozen_after_reactivation() {
    let (mut state, slot) = resting_cow();
    state.entities.body_mut(slot).attack_cooldown = 4;
    state
        .world
        .set_active_chunks(HashSet::default(), state.tick);
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(state.entities.body(slot).attack_cooldown, 4);
    state
        .world
        .set_active_chunks([ChunkPos { x: 0, z: 0 }].into_iter().collect(), state.tick);
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(state.entities.body(slot).attack_cooldown, 3);
}

#[test]
fn targetless_idle_ranged_and_swell_skip_worker_and_serial_special_rows() {
    for name in ["minecraft:skeleton", "minecraft:creeper"] {
        let (mut state, slot) = resting_mob(name);
        let pos = state.entities.pos(slot);
        assert!(run_entity_phase(&mut state, true).is_empty(), "{name}");
        assert_eq!(state.entities.pos(slot), pos, "{name}");
        assert_eq!(state.entity_phase_scratch.writes.capacity(), 0, "{name}");
        assert_eq!(state.entity_phase_scratch.specials.capacity(), 0, "{name}");
    }
}

#[test]
fn retained_swell_metadata_still_reaches_the_serial_special_pass() {
    let (mut state, slot) = resting_mob("minecraft:creeper");
    let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
    appearance
        .extra
        .retain(|(index, _)| *index != crate::hostile::CREEPER_SWELL_DIR_INDEX);
    appearance.extra.push((
        crate::hostile::CREEPER_SWELL_DIR_INDEX,
        crate::boundary::EntityDataValue::VarInt(1),
    ));
    assert!(!crate::hostile::swell_metadata_neutral(
        state.entities.row(slot)
    ));
    let _ = run_entity_phase(&mut state, true);
    assert!(state.entity_phase_scratch.writes.capacity() > 0);
    assert!(state.entity_phase_scratch.specials.capacity() > 0);
    assert!(crate::hostile::swell_metadata_neutral(
        state.entities.row(slot)
    ));
}

#[test]
fn synthetic_player_body_keeps_global_motion_admission_without_player_state() {
    let (mut state, slot) = resting_cow();
    state.entities.set_kind(
        slot,
        EntityKind::Player {
            connection: crate::boundary::ConnectionId(991),
        },
    );
    let body = state.entities.body_mut(slot);
    body.pickup_delay = 3;
    state
        .world
        .set_active_chunks(HashSet::default(), state.tick);
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(state.entities.body(slot).pickup_delay, 2);
    assert!(state.entity_phase_scratch.players.capacity() > 0);
    assert!(state.entity_phase_scratch.writes.capacity() > 0);
}

#[test]
fn cached_id_phase_matches_wrapping_decisions_at_tick_and_id_boundaries() {
    for id in [i32::MIN, -1, 0, 1, 2, i32::MAX] {
        let flags = ((id as u64 & 3) as u16) << MOTION_PHASE_SHIFT;
        for tick in [0, 1, 2, 3, 254, 255, 256, u64::MAX - 1, u64::MAX] {
            for period in [1, 2, 4] {
                assert_eq!(
                    motion_thinks(tick, flags, period),
                    tick.wrapping_add(id as u64)
                        .is_multiple_of(u64::from(period)),
                    "id={id} tick={tick} period={period}",
                );
            }
        }
    }
}

fn admission_oracle(state: &TickState, global_players: bool) -> Vec<EntitySlot> {
    state
        .entities
        .slots()
        .filter(|&slot| {
            let pos = state.entities.pos(slot);
            state.world.is_active(
                mc_physics::mth::floor_i32(pos[0]).div_euclid(16),
                mc_physics::mth::floor_i32(pos[2]).div_euclid(16),
            ) || global_players && state.players.contains_key(&state.entities.body(slot).id)
        })
        .collect()
}

#[test]
fn active_admission_dense_range_survives_compaction_and_reuses_capacity() {
    let (mut state, first) = resting_cow();
    let middle = state.spawn_mob(state.registry.cow_type_id, [24.5, 101.0, 8.5], 10.0);
    let tail = state.spawn_mob(state.registry.cow_type_id, [8.5, 101.0, 8.5], 10.0);
    state.entities.body_mut(first).alive = false;
    state.remove_entity(state.entities.slot_of(middle).unwrap());
    let slots = active_entity_slots(&mut state, false);
    assert_eq!(slots, vec![EntitySlot(0), EntitySlot(1)]);
    let capacity = slots.capacity();
    state.serial_entity_slots = slots;
    let slots = active_entity_slots(&mut state, true);
    assert_eq!(slots, vec![EntitySlot(0), EntitySlot(1)]);
    assert_eq!(slots.capacity(), capacity);
    state.serial_entity_slots = slots;
    state
        .entities
        .set_pos(state.entities.slot_of(tail).unwrap(), [64.5, 101.0, 8.5]);
    let expected = admission_oracle(&state, false);
    assert_eq!(active_entity_slots(&mut state, false), expected);
    assert_eq!(
        state.serial_entity_marks.capacity(),
        0,
        "mixed activity still uses dense admission without a mark buffer"
    );
}

#[test]
fn active_admission_rebuilds_after_removal_and_keeps_player_modes_distinct() {
    let (mut state, cow_slot) = resting_cow();
    let cow = state.entities.body(cow_slot).id;
    let connection = ConnectionId(992);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection,
            uuid: uuid::Uuid::from_u128(992),
            name: "admission".to_owned(),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(connection).unwrap();
    state.entities.set_pos(
        state.entities.slot_of(player).unwrap(),
        [128.5, 101.0, 128.5],
    );
    state
        .world
        .set_active_chunks([ChunkPos { x: 0, z: 0 }].into_iter().collect(), state.tick);
    for global_players in [false, true] {
        let expected = admission_oracle(&state, global_players);
        let slots = active_entity_slots(&mut state, global_players);
        assert_eq!(slots, expected);
        state.serial_entity_slots = slots;
    }
    // The previous player-inclusive marks must not survive dense compaction.
    state.remove_entity(state.entities.slot_of(cow).unwrap());
    let expected = admission_oracle(&state, false);
    assert_eq!(active_entity_slots(&mut state, false), expected);
}

#[test]
fn dense_motion_admission_freezes_only_the_inactive_column() {
    let (mut state, active) = resting_cow();
    let inactive = state.spawn_mob(state.registry.cow_type_id, [24.5, 101.0, 8.5], 10.0);
    let inactive = state.entities.slot_of(inactive).unwrap();
    state.entities.body_mut(active).pickup_delay = 3;
    state.entities.body_mut(inactive).pickup_delay = 3;
    state
        .world
        .set_active_chunks([ChunkPos { x: 0, z: 0 }].into_iter().collect(), state.tick);
    assert!(state.entities.has_exact_chunk_membership());
    let expected = admission_oracle(&state, false);
    assert_eq!(active_entity_slots(&mut state, false), expected);
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(state.entities.body(active).pickup_delay, 2);
    assert_eq!(state.entities.body(inactive).pickup_delay, 3);
}

#[test]
fn old_cross_column_type_membership_retains_indexed_admission() {
    let (mut state, slot) = resting_mob("minecraft:villager");
    state.entities.set_kind(
        slot,
        EntityKind::Mob {
            type_id: state.registry.cow_type_id,
        },
    );
    let (body, row) = state.entities.parts_mut(slot);
    row.villager = None;
    body.pickup_delay = 3;
    state.entities.set_pos(slot, [24.5, 101.0, 8.5]);
    state
        .world
        .set_active_chunks([ChunkPos { x: 0, z: 0 }].into_iter().collect(), state.tick);
    assert!(!state.entities.has_exact_chunk_membership());
    assert!(!state.world.is_active(1, 0));
    for global_players in [false, true] {
        let slots = active_entity_slots(&mut state, global_players);
        assert_eq!(
            slots,
            vec![slot],
            "the old active membership remains authoritative"
        );
        state.serial_entity_slots = slots;
    }
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(state.entities.body(slot).pickup_delay, 2);
}

#[test]
fn sparse_admission_retains_indexed_path_for_dormant_rows() {
    let (mut state, active) = resting_cow();
    for _ in 0..3 {
        state.spawn_mob(state.registry.cow_type_id, [24.5, 101.0, 8.5], 10.0);
    }
    state
        .world
        .set_active_chunks([ChunkPos { x: 0, z: 0 }].into_iter().collect(), state.tick);
    assert_eq!(state.serial_entity_marks.capacity(), 0);
    assert_eq!(active_entity_slots(&mut state, true), vec![active]);
    assert!(
        state.serial_entity_marks.capacity() >= state.entities.len(),
        "mostly dormant rows must use indexed admission, not scan cold IDs"
    );
}

#[test]
fn worker_snapshot_keeps_rare_state_in_entity_store() {
    let bytes = std::mem::size_of::<ReadRow>();
    assert!(
        bytes <= 256,
        "hot combat/breeding scalar snapshot copies {bytes} bytes"
    );
}

// R6b admission, before the partially-active dense path. Keep this indexed
// reference separate from the position-based correctness oracle above.
fn r6b_active_entity_slots(state: &mut TickState, global_players: bool) -> Vec<EntitySlot> {
    let mut slots = std::mem::take(&mut state.serial_entity_slots);
    slots.clear();
    let all_active = state
        .entities
        .villager_chunks()
        .keys()
        .all(|chunk| state.world.is_active(chunk.x, chunk.z))
        && state
            .entities
            .brain_sensor_chunks()
            .keys()
            .all(|chunk| state.world.is_active(chunk.x, chunk.z));
    if all_active {
        slots.extend(state.entities.slots());
        return slots;
    }
    let mut marks = std::mem::take(&mut state.serial_entity_marks);
    marks.resize(state.entities.len(), 0);
    let mut admitted = 0;
    for chunk in state
        .world
        .active_positions()
        .filter(|chunk| state.world.is_active(chunk.x, chunk.z))
    {
        state.entities.for_each_slot_in_chunk(chunk, |slot| {
            let mark = &mut marks[slot.0 as usize];
            admitted += usize::from(*mark == 0);
            *mark = 1;
        });
    }
    if global_players {
        for id in state.players.keys() {
            if let Some(slot) = state.entities.slot_of_any(*id) {
                let mark = &mut marks[slot.0 as usize];
                admitted += usize::from(*mark == 0);
                *mark = 1;
            }
        }
    }
    slots.reserve(admitted);
    for (index, &marked) in marks.iter().enumerate() {
        if marked != 0 {
            slots.push(EntitySlot(index as u32));
        }
    }
    marks.clear();
    state.serial_entity_marks = marks;
    slots
}

#[test]
#[ignore = "algorithm-level release timing; requires Main's exclusive perf GO"]
fn sparse_and_almost_active_admission_algorithm_micro() {
    const ROWS: usize = 100_000;
    const ITERATIONS: usize = 1024;
    type Admission = fn(&mut TickState, bool) -> Vec<EntitySlot>;

    let (mut state, _) = resting_cow();
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(993),
            uuid: uuid::Uuid::from_u128(993),
            name: "admission-micro".to_owned(),
            permission_level: 0,
        }],
    );
    assert_eq!(state.players.len(), 1);
    let chunks: Vec<_> = (0..100).map(|x| ChunkPos { x, z: 0 }).collect();
    state
        .world
        .ensure_blocking(&chunks.iter().copied().map(|pos| (pos, 0)).collect());
    let position = |index: usize| [(index / 1000) as f64 * 16.0 + 8.5, 101.0, 8.5];
    for index in 0..state.entities.len() {
        state
            .entities
            .set_pos(EntitySlot(index as u32), position(index));
    }
    while state.entities.len() < ROWS {
        state.spawn_mob(
            state.registry.cow_type_id,
            position(state.entities.len()),
            10.0,
        );
    }
    assert_eq!(state.entities.len(), ROWS);
    assert_eq!(state.entities.brain_sensor_chunks().len(), 100);
    assert!(state.entities.villager_chunks().is_empty());
    assert!(state.entities.has_exact_chunk_membership());

    for active_chunks in [1, 99] {
        state.world.set_active_chunks(
            chunks[..active_chunks].iter().copied().collect(),
            state.tick,
        );
        assert_eq!(prefer_dense_entity_admission(&state), active_chunks == 99);
        let expected = r6b_active_entity_slots(&mut state, true);
        assert_eq!(expected.len(), active_chunks * 1000);
        state.serial_entity_slots = expected.clone();
        let follow_up: Admission = active_entity_slots;
        let reference: Admission = r6b_active_entity_slots;
        for run in [follow_up, reference] {
            for _ in 0..8 {
                let slots = run(&mut state, true);
                assert_eq!(slots, expected);
                state.serial_entity_slots = slots;
            }
        }
        let slot_capacity = state.serial_entity_slots.capacity();
        let mark_capacity = state.serial_entity_marks.capacity();
        let mut samples = Vec::new();
        let mut elapsed_ns = [0.0; 4];
        for (leg, (label, run)) in [
            ("B_follow_up", follow_up),
            ("A_r6b_reference", reference),
            ("B_follow_up", follow_up),
            ("A_r6b_reference", reference),
        ]
        .into_iter()
        .enumerate()
        {
            let run = std::hint::black_box(run);
            let start = std::time::Instant::now();
            for _ in 0..ITERATIONS {
                let slots = run(std::hint::black_box(&mut state), true);
                std::hint::black_box(&slots);
                state.serial_entity_slots = slots;
            }
            let ns = start.elapsed().as_nanos() as f64 / ITERATIONS as f64;
            elapsed_ns[leg] = ns;
            assert_eq!(state.serial_entity_slots, expected);
            assert_eq!(state.serial_entity_slots.capacity(), slot_capacity);
            assert_eq!(state.serial_entity_marks.capacity(), mark_capacity);
            samples.push(serde_json::json!({"algorithm": label, "ns_per_call": ns}));
        }
        let follow_up_ns = (elapsed_ns[0] + elapsed_ns[2]) / 2.0;
        let reference_ns = (elapsed_ns[1] + elapsed_ns[3]) / 2.0;
        println!(
            "ADMISSION_MICRO {}",
            serde_json::json!({
                "scope": "algorithm-level active_entity_slots, not whole-tick/native",
                "rows": ROWS,
                "players": state.players.len(),
                "occupied_chunks": 100,
                "active_chunks": active_chunks,
                "iterations_per_leg": ITERATIONS,
                "order": "B,A,B,A",
                "slot_capacity": slot_capacity,
                "mark_capacity": mark_capacity,
                "samples": samples,
                "follow_up_ns": follow_up_ns,
                "r6b_reference_ns": reference_ns,
                "follow_up_over_r6b": follow_up_ns / reference_ns,
            })
        );
    }
}

#[test]
fn dense_chunk_membership_keeps_floor_saturation_and_nan_semantics() {
    let (mut state, slot) = resting_cow();
    let id = state.entities.body(slot).id;
    for x in [
        f64::NEG_INFINITY,
        f64::MIN,
        -2_147_483_648.5,
        -16.000_000_001,
        -16.0,
        -0.001,
        -0.0,
        0.0,
        15.999_999_999,
        16.0,
        2_147_483_647.5,
        f64::MAX,
        f64::INFINITY,
        f64::NAN,
    ] {
        state.entities.set_pos(slot, [x, 101.0, x]);
        let axis = (x.floor() as i32).div_euclid(16);
        let expected = ChunkPos { x: axis, z: axis };
        assert_eq!(state.entities.chunk_of(slot), expected, "{x:?}");
        assert!(state.entities.brain_sensor_chunks()[&expected].contains(&id));
        assert!(state.entities.has_exact_chunk_membership());
    }
}

#[test]
fn resident_villager_targets_match_the_dense_scan_in_slot_order() {
    let (mut state, _) = resting_cow();
    let type_of = |state: &TickState, name: &str| {
        state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let villager = type_of(&state, "minecraft:villager");
    let zombie_villager = type_of(&state, "minecraft:zombie_villager");
    let trader = type_of(&state, "minecraft:wandering_trader");
    let cow = state.registry.cow_type_id;
    let mut spawned = Vec::new();
    for index in 0..40_u32 {
        let type_id = match index % 5 {
            0 | 1 => villager,
            2 => zombie_villager,
            3 => trader,
            _ => cow,
        };
        let x = f64::from(index) * 7.0 - 100.0;
        let id = state.spawn_mob(type_id, [x, 101.0, 8.5], 10.0);
        let slot = state.entities.slot_of(id).unwrap();
        if type_id != cow {
            let converting = type_id == zombie_villager && index % 2 == 0;
            state.entities.row_mut(slot).villager =
                Some(Box::new(crate::villager::VillagerState {
                    wandering: type_id == trader,
                    conversion_time: converting.then_some(-1),
                    ..Default::default()
                }));
        }
        spawned.push(id);
    }
    // Dead resident, a chunk-crossing resident and compaction by removal.
    let dead = state.entities.slot_of(spawned[5]).unwrap();
    state.entities.body_mut(dead).alive = false;
    let crossing = state.entities.slot_of(spawned[10]).unwrap();
    state.entities.set_pos(crossing, [-300.0, 101.0, 300.0]);
    let doomed = state.entities.slot_of(spawned[1]).unwrap();
    state.remove_entity(doomed);
    let oracle: Vec<(EntityId, [f64; 3])> = state
        .entities
        .slots()
        .filter(|slot| {
            state.entities.body(*slot).alive
                && state
                    .entities
                    .row(*slot)
                    .villager
                    .as_ref()
                    .is_some_and(|v| v.conversion_time.is_none())
        })
        .map(|slot| (state.entities.body(slot).id, state.entities.pos(slot)))
        .collect();
    assert!(oracle.len() > 10);
    assert_eq!(resident_villager_targets(&state.entities), oracle);
}

#[test]
fn parallel_row_prep_matches_the_serial_prep_including_rare_fixups() {
    let (mut state, _) = resting_cow();
    let villager = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let zombie = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let dolphin = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:dolphin").unwrap())
        .unwrap();
    state.spawn_mob(dolphin, [9.5, 101.0, 8.5], 10.0);
    let chunks: Vec<_> = (0..2).map(|x| ChunkPos { x, z: 0 }).collect();
    state
        .world
        .set_active_chunks(chunks.iter().copied().collect(), 1);
    for index in 0..(3 * PREP_CHUNK + 777) as u32 {
        let x = 8.5 + f64::from(index % 8);
        let type_id = match index % 7 {
            0 => zombie,
            3 => villager,
            _ => state.registry.cow_type_id,
        };
        let id = state.spawn_mob(type_id, [x, 101.0, 8.5], 10.0);
        let slot = state.entities.slot_of(id).unwrap();
        if type_id == state.registry.cow_type_id {
            state.entities.row_mut(slot).love = 600;
        }
        if index % 7 == 3 && index % 21 == 3 {
            // No-AI plugin NPC with a committed route: a rare, serial-only input.
            let row = state.entities.row_mut(slot);
            row.controls.no_ai = true;
            row.persistent = true;
            row.villager = Some(Box::new(crate::villager::VillagerState {
                navigation_path: vec![crate::boundary::BlockPos {
                    x: 10,
                    y: 101,
                    z: 8,
                }],
                ..Default::default()
            }));
        }
    }
    state.entities.prepare_motion_hints(&state.registry);
    let periods: Vec<u8> = (0..state.entities.len())
        .map(|index| [1, 2, MOB_FAR_PERIOD as u8, 0][index % 4])
        .collect();
    let env = PrepEnv {
        targets_empty: true,
        villagers_empty: resident_villager_targets(&state.entities).is_empty(),
        unchanged_since: state.entity_phase_revision,
    };
    let mut serial = PrepChunk::default();
    prepare_rows(&state, &periods, &env, false, &mut serial);
    let mut parallel = PrepChunk::default();
    prepare_rows(&state, &periods, &env, true, &mut parallel);
    assert!(serial.rows.len() > 2 * PREP_CHUNK);
    assert!(serial.rows.iter().any(|row| row.plugin_target.is_some()));
    assert!(serial.rare.is_empty() && parallel.rare.is_empty());
    assert_eq!(serial.summary, parallel.summary);
    assert!(parallel.summary.has_dolphin);
    assert_eq!(
        parallel.summary.consumers[0],
        serial
            .rows
            .iter()
            .filter(|row| row.chaser && row.alive)
            .count()
    ); // Zombies are the only chase-consumer class in this fixture.
    let mut expected_cells: HashMap<(i32, i32), Vec<usize>> = HashMap::default();
    for (index, row) in serial.rows.iter().enumerate() {
        if row.breed_ready && row.alive {
            expected_cells
                .entry((
                    mc_physics::mth::floor_i32(row.pos[0]) / 4,
                    mc_physics::mth::floor_i32(row.pos[2]) / 4,
                ))
                .or_default()
                .push(index);
        }
    }
    assert!(!expected_cells.is_empty());
    assert_eq!(parallel.summary.cow_cells, expected_cells);
    assert_eq!(format!("{:?}", serial.rows), format!("{:?}", parallel.rows));
    assert_eq!(serial.specials, parallel.specials);
    assert_eq!(serial.resting, parallel.resting);
    let slots: Vec<_> = serial.rows.iter().map(|row| row.slot.0).collect();
    assert!(slots.windows(2).all(|pair| pair[0] < pair[1]));
    // The fix-up equals the all-in-one helper path for every rare row.
    for row in &serial.rows {
        let view = PrepView::of(&state);
        let flags = state.entities.motion_flags(row.slot);
        let period = periods[row.slot.0 as usize];
        let rare = RarePrep::of(&state, row.slot);
        let (expected, _) = read_row(&view, row.slot, flags, period, &rare);
        assert_eq!(format!("{row:?}"), format!("{expected:?}"));
    }
}

#[test]
#[ignore = "algorithm-level release timing; requires Main's exclusive perf GO"]
fn parallel_row_prep_algorithm_micro() {
    const ROWS: usize = 100_000;
    const ITERATIONS: usize = 20;
    let (mut state, _) = resting_cow();
    while state.entities.len() < ROWS {
        let index = state.entities.len();
        let id = state.spawn_mob(
            state.registry.cow_type_id,
            [8.5 + (index % 8) as f64, 101.0, 8.5],
            10.0,
        );
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.set_vel(slot, [0.0, -0.08, 0.0]);
    }
    state.entities.prepare_motion_hints(&state.registry);
    let periods: Vec<u8> = (0..ROWS).map(|index| [1, 2, 4][index % 3]).collect();
    let env = PrepEnv {
        targets_empty: true,
        villagers_empty: true,
        unchanged_since: state.entity_phase_revision,
    };
    let mut out = PrepChunk::default();
    let mut expected = None;
    let mut elapsed = [0.0_f64; 4];
    for (leg, (label, parallel)) in [
        ("B_parallel", true),
        ("A_serial", false),
        ("B_parallel", true),
        ("A_serial", false),
    ]
    .into_iter()
    .enumerate()
    {
        let mut total = std::time::Duration::ZERO;
        for _ in 0..ITERATIONS {
            out.rows.clear();
            out.specials.clear();
            out.resting.clear();
            let start = std::time::Instant::now();
            prepare_rows(
                &state,
                &periods,
                &env,
                std::hint::black_box(parallel),
                &mut out,
            );
            total += start.elapsed();
            std::hint::black_box(&out.rows);
        }
        let digest = format!("{:?}|{:?}", out.rows, out.resting);
        assert_eq!(*expected.get_or_insert(digest.clone()), digest);
        elapsed[leg] = total.as_secs_f64() * 1e3 / ITERATIONS as f64;
        println!(
            "PREP_MICRO leg={leg} {label} ms={:.3} rows={} resting={}",
            elapsed[leg],
            out.rows.len(),
            out.resting.len()
        );
    }
    println!(
        "{}",
        serde_json::json!({
            "scope": "algorithm-level prepare_rows, not whole-tick",
            "rows": ROWS,
            "order": "B,A,B,A",
            "parallel_ms": (elapsed[0] + elapsed[2]) / 2.0,
            "serial_ms": (elapsed[1] + elapsed[3]) / 2.0,
            "threads": rayon::current_num_threads(),
        })
    );
}

fn full_step_without_early_rest(
    state: &TickState,
    slot: EntitySlot,
) -> (MoveWrite, Vec<RawIntent>) {
    let view = PrepView::of(state);
    let flags = state.entities.motion_flags(slot);
    let row = read_row(&view, slot, flags, 1, &RarePrep::of(state, slot)).0;
    let targets = [];
    let cow_cells = HashMap::default();
    let world = &state.world;
    let loaded = |x: i32, z: i32| world.has_chunk(x.div_euclid(16), z.div_euclid(16));
    let revision = |x, z| world.chunk_revision(x, z);
    let ctx = StepCtx {
        rows: std::slice::from_ref(&row),
        entities: &state.entities,
        retained_targets: &state.hostile.targets,
        players: crate::target_grid::TargetGrid::for_consumers(&targets, 0),
        passive: passive_follow::Context {
            players: crate::target_grid::TargetGrid::for_consumers(&targets, 0),
            states: &state.players,
            locked: state.require_plugin_login.then_some(&state.locked_sessions),
        },
        dolphin_toys: &[],
        villagers: crate::target_grid::TargetGrid::for_consumers(&targets, 0),
        golem_targets: crate::target_grid::TargetGrid::for_consumers(&targets, 0),
        piglin_targets: crate::target_grid::TargetGrid::for_consumers(&targets, 0),
        zoglin_targets: crate::target_grid::TargetGrid::for_consumers(&targets, 0),
        cow_cells: &cow_cells,
        world: &state.world,
        chunk_loaded: &loaded,
        registry: &state.registry,
        tick: state.tick,
        chunk_revision: &revision,
        unchanged_since: state.entity_phase_revision,
        difficulty: state.difficulty,
        night: crate::natural_spawn::is_night(state.day_time),
    };
    step_entity(row, &ctx)
}

#[test]
fn idle_wander_decision_rest_matches_full_step_for_two_episodes() {
    for (species, creative) in [
        ("minecraft:cow", false),
        ("minecraft:pig", false),
        ("minecraft:sheep", false),
        ("minecraft:chicken", false),
        ("minecraft:skeleton", true),
        ("minecraft:spider", true),
        ("minecraft:creeper", true),
        ("minecraft:zombie", true),
        ("minecraft:zombie_villager", true),
    ] {
        let (mut state, slot) = resting_mob(species);
        let _ = run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(994),
                uuid: uuid::Uuid::from_u128(994),
                name: "idle-rest-reference".to_owned(),
                permission_level: 0,
            }],
        );
        let player = *state.players.keys().next().unwrap();
        if creative {
            state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
        }
        state
            .entities
            .set_pos(state.entities.slot_of(player).unwrap(), [8.5, 101.0, 8.5]);
        let stone = state.world.block_at(8, 100, 8).unwrap();
        for x in 0..32 {
            for z in 0..16 {
                state.world.set_block(x, 100, z, stone);
            }
        }
        state.world.fold_overlay();
        state.entity_phase_revision = state.world.revision_now();
        state.entities.body_mut(slot).pickup_delay = 240;
        state.entities.body_mut(slot).attack_cooldown = 250;
        let mut rested_decisions = 0;
        for tick in 1..=240 {
            state.tick = tick;
            state.entities.prepare_motion_hints(&state.registry);
            let (expected, raw) = full_step_without_early_rest(&state, slot);
            assert!(raw.is_empty(), "{species} tick={tick}");
            let body = state.entities.body(slot);
            let (pickup, attack) = (
                body.pickup_delay.saturating_sub(1),
                body.attack_cooldown.saturating_sub(1),
            );
            if early_entity_rest(
                &PrepView::of(&state),
                slot,
                1,
                state.entity_phase_revision,
                true,
                !crate::hostile::has_live_targets(&state),
            ) {
                rested_decisions += 1;
            }
            let intents = run_entity_phase(&mut state, true);
            assert!(
                intents
                    .iter()
                    .all(|intent| intent.actor != state.entities.body(slot).id)
            );
            assert_eq!(
                state.entities.pos(slot).map(f64::to_bits),
                expected.pos.map(f64::to_bits),
                "{species} tick={tick} pos"
            );
            assert_eq!(
                state.entities.vel(slot).map(f64::to_bits),
                expected.vel.map(f64::to_bits),
                "{species} tick={tick} vel"
            );
            assert_eq!(
                state.entities.rot(slot).map(f32::to_bits),
                expected.rotation.map(f32::to_bits),
                "{species} tick={tick} rotation"
            );
            assert_eq!(state.entities.ground(slot), expected.ground);
            let body = state.entities.body(slot);
            assert_eq!(
                state.entities.horizontal_impulse(slot).map(f64::to_bits),
                expected.horizontal_impulse.map(f64::to_bits)
            );
            assert_eq!(body.air_supply, expected.air_supply);
            assert_eq!((body.pickup_delay, body.attack_cooldown), (pickup, attack));
        }
        assert!(
            rested_decisions >= 80,
            "{species}: {rested_decisions} decision rests"
        );
    }
}

#[test]
fn idle_wander_decision_proof_keeps_air_support_and_goal_changes_live() {
    let (mut state, slot) = resting_cow();
    let id = state.entities.body(slot).id;
    state.tick = (1..120)
        .find(|&tick| {
            crate::entity_motion::wander(state.entities.body(slot).movement, id.0, tick, 1.0)
                == [0.0; 3]
        })
        .unwrap();
    state.entities.prepare_motion_hints(&state.registry);
    let can_rest = |state: &TickState| {
        early_entity_rest(
            &PrepView::of(state),
            slot,
            1,
            state.entity_phase_revision,
            true,
            !crate::hostile::has_live_targets(state),
        )
    };
    assert!(can_rest(&state));
    let stone = state.world.block_at(8, 100, 8).unwrap();
    state.world.set_block(12, 100, 8, state.registry.states.air);
    state.world.fold_overlay();
    assert!(
        can_rest(&state),
        "current-geometry proof tolerates unrelated edits"
    );
    state.world.set_block(8, 100, 8, state.registry.states.air);
    state.world.fold_overlay();
    assert!(!can_rest(&state), "removed support must execute");
    state.world.set_block(8, 100, 8, stone);
    state.world.fold_overlay();
    state.entities.body_mut(slot).air_supply = 299;
    state.entities.prepare_motion_hints(&state.registry);
    assert!(!can_rest(&state), "air recovery must execute");
    let (expected, _) = full_step_without_early_rest(&state, slot);
    assert_eq!(expected.air_supply, 300);
    state.entities.body_mut(slot).air_supply = 300;
    state.entities.set_pos(slot, [8.5, 102.0, 8.5]);
    assert!(!can_rest(&state), "imported ground=true is not support");
    state.entities.set_pos(slot, [8.5, 101.0, 8.5]);
    state.entities.row_mut(slot).love = 100;
    state.entities.prepare_motion_hints(&state.registry);
    assert!(!can_rest(&state), "breeding must execute");
    state.entities.row_mut(slot).love = 0;
    state.entities.body_mut(slot).hurt_at = Some(state.tick);
    state.entities.prepare_motion_hints(&state.registry);
    assert!(!can_rest(&state), "panic must execute");
    state.entities.body_mut(slot).hurt_at = None;
    state
        .entities
        .row_mut(slot)
        .active_effects
        .add(crate::effects::ActiveEffect::new(
            crate::effects::EffectId::Levitation,
            100,
            0,
        ));
    state.entities.prepare_motion_hints(&state.registry);
    assert!(!can_rest(&state), "levitation must execute");
    let _ = state.entities.row_mut(slot).active_effects.clear(None);
    state.entities.row_mut(slot).controls.no_ai = true;
    state.entities.prepare_motion_hints(&state.registry);
    assert!(!can_rest(&state), "standing override is not a wander goal");
}

#[test]
fn targetless_chaser_decision_rest_keeps_all_target_classes_live() {
    for species in [
        "minecraft:skeleton",
        "minecraft:spider",
        "minecraft:creeper",
        "minecraft:zombie",
        "minecraft:zombie_villager",
        "minecraft:zoglin",
        "minecraft:iron_golem",
        "minecraft:piglin",
        "minecraft:hoglin",
    ] {
        let (mut state, slot) = resting_mob(species);
        let id = state.entities.body(slot).id;
        state.tick = (1..120)
            .find(|&tick| {
                crate::entity_motion::wander(state.entities.body(slot).movement, id.0, tick, 1.0)
                    == [0.0; 3]
            })
            .unwrap();
        state.entities.prepare_motion_hints(&state.registry);
        let view = PrepView::of(&state);
        let targetless_rest = matches!(
            species,
            "minecraft:skeleton"
                | "minecraft:spider"
                | "minecraft:creeper"
                | "minecraft:zombie"
                | "minecraft:zombie_villager"
        );
        assert_eq!(
            early_entity_rest(&view, slot, 1, state.entity_phase_revision, true, true),
            targetless_rest,
            "{species}"
        );
        assert!(
            !early_entity_rest(&view, slot, 1, state.entity_phase_revision, true, false),
            "a live player target wakes {species}"
        );
    }
}

fn idle_zombie() -> (TickState, EntitySlot) {
    let (mut state, slot) = resting_mob("minecraft:zombie");
    state.day_time = 14_000;
    let id = state.entities.body(slot).id;
    state.tick = (1_u64..120)
        .find(|&tick| {
            tick.wrapping_add(id.0 as u64)
                .is_multiple_of(MOB_FAR_PERIOD)
                && crate::entity_motion::wander(state.entities.body(slot).movement, id.0, tick, 1.0)
                    == [0.0; 3]
        })
        .unwrap();
    assert!(can_early_rest(&mut state, slot));
    assert!(run_entity_phase(&mut state, true).is_empty());
    assert_eq!(state.entity_phase_scratch.writes.capacity(), 0);
    (state, slot)
}

fn phase_admitted(state: &mut TickState, slot: EntitySlot, period: u8) -> bool {
    state.entities.prepare_motion_hints(&state.registry);
    admit_slot(
        &PrepView::of(state),
        slot.0 as usize,
        period,
        &PrepEnv {
            targets_empty: !crate::hostile::has_live_targets(state),
            villagers_empty: resident_villager_targets(&state.entities).is_empty(),
            unchanged_since: state.entity_phase_revision,
        },
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .is_some()
}

fn join_zombie_target(state: &mut TickState, pos: [f64; 3]) -> EntityId {
    input::session::join(
        state,
        (
            &ConnectionId(996),
            &uuid::Uuid::from_u128(996),
            &"zombie-rest-target".to_owned(),
            &0,
        ),
        &mut ChangeSet::default(),
    );
    let player = *state.players.keys().next().unwrap();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, pos);
    state.entities.set_ground(slot, true);
    player
}

#[test]
fn zombie_rest_acquires_spawned_player_and_resident_on_normal_decision() {
    for resident in [false, true] {
        let (mut state, slot) = idle_zombie();
        let id = state.entities.body(slot).id;
        let pos = [9.5, 101.0, 8.5];
        let target = if resident {
            let type_id = state
                .registry
                .entity_types
                .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
                .unwrap();
            state.spawn_mob(type_id, pos, 20.0)
        } else {
            join_zombie_target(&mut state, pos)
        };
        // Arrival just after a decision cannot extend the ordinary period.
        let mut acquired = None;
        for elapsed in 1..=MOB_FAR_PERIOD {
            state.tick += 1;
            let intents = run_entity_phase(&mut state, true);
            if intents.iter().any(|intent| {
                intent.actor == id
                    && matches!(intent.kind, IntentKind::DamageEntity { target: victim, .. } if victim == target)
            }) {
                acquired = Some(elapsed);
                break;
            }
        }
        assert_eq!(acquired, Some(if resident { MOB_FAR_PERIOD } else { 1 }));
    }
}

#[test]
fn zombie_rest_rechecks_same_chunk_range_entry_and_remote_wall_opening() {
    let (mut state, slot) = idle_zombie();
    let pos = state.entities.pos(slot);
    let sight = f64::from(state.entities.row(slot).sight_range);
    let outside = [pos[0] + sight + 0.25, pos[1], pos[2]];
    let inside = [pos[0] + sight - 0.25, pos[1], pos[2]];
    let player = join_zombie_target(&mut state, outside);
    let player_slot = state.entities.slot_of(player).unwrap();
    let chunk = state.entities.chunk_of(player_slot);
    assert!(
        phase_admitted(&mut state, slot, 1),
        "conservative wake before range entry"
    );
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(state.entities.pos(slot), pos, "out of sight and idle");
    state.entities.set_pos(player_slot, inside);
    assert_eq!(state.entities.chunk_of(player_slot), chunk);
    state.tick += 1;
    let _ = run_entity_phase(&mut state, true);
    assert!(
        state.entities.pos(slot)[0] > pos[0],
        "next near-player sensor decision chases"
    );

    // Occlusion is not a rest dependency, even outside the mob's footprint chunk.
    state.entities.set_pos(slot, pos);
    state.entities.set_vel(slot, [0.0; 3]);
    state.entities.set_ground(slot, true);
    state.entities.set_pos(player_slot, [18.5, pos[1], pos[2]]);
    state
        .world
        .set_block(17, 102, 8, state.registry.states.stone);
    state.world.fold_overlay();
    let eyes = [pos[0], 102.5, pos[2]];
    let target_eyes = [18.5, 102.5, pos[2]];
    assert!(!crate::hostile::sight_clear(&state, eyes, target_eyes));
    assert!(phase_admitted(&mut state, slot, 1));
    state.world.set_block(17, 102, 8, state.registry.states.air);
    state.world.fold_overlay();
    assert!(crate::hostile::sight_clear(&state, eyes, target_eyes));
    assert!(phase_admitted(&mut state, slot, 1));
    let _ = run_entity_phase(&mut state, true);
    assert!(state.entities.pos(slot)[0] > pos[0]);
}

#[test]
fn zombie_rest_keeps_daylight_water_and_support_reactions() {
    let (mut state, slot) = idle_zombie();
    // Existing daylight processing requires an observer; creative observes
    // without supplying a chase target.
    let observer = join_zombie_target(&mut state, [12.5, 101.0, 8.5]);
    state.players.get_mut(&observer).unwrap().game_mode = crate::commands::GameMode::Creative;
    assert!(can_early_rest(&mut state, slot));
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.day_time = 6_000;
    state.tick = crate::natural_spawn::CYCLE;
    crate::natural_spawn::run(&mut state, &mut ChangeSet::default());
    assert!(
        state.entities.body(slot).burn_ticks > 0,
        "daylight ignites a rested zombie"
    );
    assert!(phase_admitted(&mut state, slot, 1));

    for (water, air) in [(false, 300), (true, 300), (true, 299)] {
        let (mut state, slot) = idle_zombie();
        state.entities.body_mut(slot).air_supply = air;
        if water {
            for y in 101..=102 {
                state.world.set_block(8, y, 8, state.registry.states.water);
            }
        } else {
            state.world.set_block(8, 100, 8, state.registry.states.air);
        }
        state.world.fold_overlay();
        if !water || air != 300 {
            assert!(phase_admitted(&mut state, slot, 1));
        }
        let (expected, _) = full_step_without_early_rest(&state, slot);
        let _ = run_entity_phase(&mut state, true);
        assert_eq!(state.entities.pos(slot), expected.pos);
        assert_eq!(state.entities.vel(slot), expected.vel);
        assert_eq!(state.entities.body(slot).air_supply, expected.air_supply);
        if water {
            // The existing generic Walker path replenishes air even in water;
            // rest must preserve that behavior, including non-no-op recovery.
            assert_eq!(expected.air_supply, 300);
        } else {
            assert!(expected.pos[1] < 101.0, "removed support causes a fall");
        }
    }
}

/// Isolated entity-phase acceptance fixture; no player, night, 300 zombies.
fn zombie_rest_probe_state() -> TickState {
    let (mut state, first) = resting_mob("minecraft:zombie");
    state.entities.remove(first);
    let chunks: Vec<_> = (0..8)
        .flat_map(|x| (0..8).map(move |z| ChunkPos { x, z }))
        .collect();
    state
        .world
        .ensure_blocking(&chunks.iter().map(|&pos| (pos, 0)).collect());
    state
        .world
        .set_active_chunks(chunks.into_iter().collect(), 1);
    let stone = state.world.block_at(8, 100, 8).unwrap();
    for x in 0..128 {
        for z in 0..128 {
            state.world.set_block(x, 100, z, stone);
        }
    }
    state.world.fold_overlay();
    state.entity_phase_revision = state.world.revision_now();
    state.day_time = 14_000;
    let zombie_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    for index in 0..300 {
        let pos = [
            24.5 + f64::from(index % 20) * 4.0,
            101.0,
            24.5 + f64::from(index / 20) * 4.0,
        ];
        let id = state.spawn_mob(zombie_type, pos, 20.0);
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.set_ground(slot, true);
    }
    for _ in 0..120 {
        state.tick += 1;
        let _ = run_entity_phase(&mut state, true);
    }
    state
}

#[test]
#[ignore = "release acceptance measurement; requires Main's exclusive perf GO"]
fn zombie_rest_release_probe_300_zombies_no_player() {
    let mut state = zombie_rest_probe_state();
    let mut prepared = 0;
    let mut decisions = 0;
    for _ in 0..600 {
        state.tick += 1;
        state.entities.prepare_motion_hints(&state.registry);
        let periods = vec![MOB_FAR_PERIOD as u8; state.entities.len()];
        let mut out = PrepChunk::default();
        prepare_rows(
            &state,
            &periods,
            &PrepEnv {
                targets_empty: true,
                villagers_empty: true,
                unchanged_since: state.entity_phase_revision,
            },
            false,
            &mut out,
        );
        assert_eq!(out.rows.len() + out.resting.len(), 300);
        prepared += out.rows.len();
        decisions += out
            .rows
            .iter()
            .filter(|row| (state.tick.wrapping_add(row.id.0 as u64)).is_multiple_of(MOB_FAR_PERIOD))
            .count();
        let _ = run_entity_phase(&mut state, true);
        assert!(state.entities.slots().all(|slot| {
            let chunk = state.entities.chunk_of(slot);
            state.world.is_active_position(chunk.x, chunk.z)
        }));
    }
    eprintln!("ZREST_COUNTS ticks=600 mobs=300 prepared={prepared} decisions={decisions}");
    for sample in 0..3 {
        let mut state = zombie_rest_probe_state();
        let start = std::time::Instant::now();
        for _ in 0..600 {
            state.tick += 1;
            let _ = std::hint::black_box(run_entity_phase(&mut state, true));
        }
        eprintln!(
            "ZREST_TIME sample={sample} entity_phase_600_ticks_ms={:.6}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}

/// Vanilla `Entity.checkFallDamage` applies to mobs: a 20-block drop owes
/// `ceil(20 - 3)` damage on landing, while `#fall_damage_immune` types (a
/// chicken here) and flyers take none.
#[test]
fn mobs_take_fall_damage_on_landing_unless_immune() {
    for (species, immune) in [("minecraft:skeleton", false), ("minecraft:chicken", true)] {
        let (mut state, slot) = resting_mob(species);
        let _ = run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(995),
                uuid: uuid::Uuid::from_u128(995),
                name: "fall-watcher".to_owned(),
                permission_level: 0,
            }],
        );
        let player = *state.players.keys().next().unwrap();
        state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
        state
            .entities
            .set_pos(state.entities.slot_of(player).unwrap(), [8.5, 101.0, 12.5]);
        let stone = state.world.block_at(8, 100, 8).unwrap();
        for x in 0..32 {
            for z in 0..16 {
                state.world.set_block(x, 100, z, stone);
            }
        }
        state.world.fold_overlay();
        state.entity_phase_revision = state.world.revision_now();
        state.entities.row_mut(slot).health = 100.0;
        state.entities.set_pos(slot, [8.5, 121.0, 8.5]);
        state.entities.set_ground(slot, false);
        // Stop at the first landing: the wandering mob would otherwise leave
        // the stone pad and take a second, longer fall.
        for _ in 0..80 {
            let _ = run_tick(&mut state, &[]);
            if state.entities.ground(slot) && state.entities.pos(slot)[1] < 110.0 {
                break;
            }
        }
        let health = state.entities.row(slot).health;
        if immune {
            assert_eq!(health, 100.0, "{species} is fall_damage_immune");
        } else {
            assert!(
                (82.0..=83.0).contains(&health),
                "{species} fell 20 blocks and took 17-18 damage, health {health}"
            );
        }
    }
}

fn second_sensor_player(state: &mut TickState, pos: [f64; 3]) -> EntityId {
    let connection = ConnectionId(997);
    input::session::join(
        state,
        (
            &connection,
            &uuid::Uuid::from_u128(997),
            &"second-target".to_owned(),
            &0,
        ),
        &mut ChangeSet::default(),
    );
    let id = state.connections.entity_of(connection).unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, pos);
    id
}

#[test]
fn sensors_retain_target_until_range_death_removal_or_player_ineligibility() {
    for event in [
        "range",
        "death",
        "despawn",
        "creative",
        "spectator",
        "locked",
    ] {
        let (mut state, mob) = resting_mob("minecraft:zombie");
        state.day_time = 14_000;
        let actor = state.entities.body(mob).id;
        let first = join_zombie_target(&mut state, [12.5, 101.0, 8.5]);
        let second = second_sensor_player(&mut state, [15.5, 101.0, 8.5]);
        let _ = run_entity_phase(&mut state, true);
        assert_eq!(state.hostile.targets[&actor].0.0, first);
        let first_slot = state.entities.slot_of(first).unwrap();
        let second_slot = state.entities.slot_of(second).unwrap();
        // A closer candidate in the same cell is not a retarget event.
        state.entities.set_pos(second_slot, [9.5, 101.0, 8.5]);
        let _ = run_entity_phase(&mut state, true);
        assert_eq!(
            state.hostile.targets[&actor].0.0, first,
            "{event}: retained"
        );
        match event {
            "range" => state.entities.set_pos(first_slot, [120.5, 101.0, 8.5]),
            "death" => state.entities.body_mut(first_slot).alive = false,
            "despawn" => {
                state.remove_entity(first_slot);
            }
            "creative" => {
                state.players.get_mut(&first).unwrap().game_mode =
                    crate::commands::GameMode::Creative
            }
            "spectator" => {
                state.players.get_mut(&first).unwrap().game_mode =
                    crate::commands::GameMode::Spectator
            }
            "locked" => {
                state.require_plugin_login = true;
                state
                    .locked_sessions
                    .insert(state.players[&first].connection);
            }
            _ => unreachable!(),
        }
        let _ = run_entity_phase(&mut state, true);
        assert_eq!(
            state.hostile.targets[&actor].0.0, second,
            "{event}: reacquired"
        );
    }
}

#[test]
fn sensors_damage_commit_replaces_target_with_attacker() {
    let (mut state, mob) = resting_mob("minecraft:zombie");
    state.day_time = 14_000;
    let actor = state.entities.body(mob).id;
    let first = join_zombie_target(&mut state, [12.5, 101.0, 8.5]);
    let second = second_sensor_player(&mut state, [15.5, 101.0, 8.5]);
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(state.hostile.targets[&actor].0.0, first);
    state.merge_damage.push((
        mob,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 1.0,
            kind: crate::survival::DamageKind::Melee,
            source: Some(crate::intent::DamageSource::Melee {
                attacker: second,
                position: [15.5, 101.0, 8.5],
                yaw: 0.0,
                bonus_knockback: 0.0,
                sweep: false,
                fire_ticks: 0,
                bane_ticks: 0,
            }),
        },
    ));
    let _ = state.commit_merge_effects();
    assert_eq!(
        state.hostile.targets[&actor].0.0, second,
        "damage event wins"
    );
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(
        state.hostile.targets[&actor].0.0, second,
        "nearer old target does not override revenge"
    );
}

#[test]
fn sensors_mob_revenge_does_not_rest_without_player_targets() {
    let (mut state, mob) = resting_mob("minecraft:zombie");
    state.day_time = 14_000;
    let actor = state.entities.body(mob).id;
    let kind = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:skeleton").unwrap())
        .unwrap();
    let attacker = state.spawn_mob(kind, [12.5, 101.0, 8.5], 20.0);
    crate::hostile::retaliate(&mut state, mob, attacker);
    crate::hostile::refresh_targets(&mut state);
    assert!(!can_early_rest(&mut state, mob));
    let _ = run_entity_phase(&mut state, true);
    assert_eq!(state.hostile.targets[&actor].0.0, attacker);
}
