use super::flat_player_fixture;
use crate::boundary::{ConnectionId, EntityId, PlayerInput};
use crate::intent::{Intent, IntentKind};
use crate::survival::DamageKind;
use crate::tick::{TickState, run_tick};
use mc_data::Identifier;

fn place(state: &mut TickState, pos: [f64; 3], name: &str, properties: &[(&str, &str)]) {
    let block = state
        .registry
        .blocks
        .block(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    let id = block
        .states
        .iter()
        .copied()
        .find(|id| {
            let b = state.registry.blocks.by_id(*id).unwrap();
            properties
                .iter()
                .all(|(name, value)| b.properties.iter().any(|(k, v)| k == name && v == value))
        })
        .unwrap()
        .0;
    state.world.set_block(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
        id,
    );
    state.world.fold_overlay();
}
fn sample(state: &mut TickState, entity: EntityId, kind: DamageKind) -> bool {
    let mut intents = Vec::<Intent>::new();
    crate::contact_hazards::run(state, &mut intents);
    intents.iter().any(|intent| matches!(intent.kind, IntentKind::DamageEntity { target, kind:k, .. } if target == entity && k == kind))
}

#[test]
fn suffocation_triggers_on_the_eye_cell_without_requiring_solid_feet() {
    let (mut state, entity, pos) = flat_player_fixture();
    place(&mut state, pos, "stone", &[]);
    assert!(!sample(&mut state, entity, DamageKind::Suffocation));
    place(&mut state, [pos[0], pos[1] + 1.0, pos[2]], "stone", &[]);
    assert!(sample(&mut state, entity, DamageKind::Suffocation));
    place(&mut state, pos, "air", &[]);
    assert!(
        sample(&mut state, entity, DamageKind::Suffocation),
        "head in a wall suffocates even when the feet cell is air"
    );
}

#[test]
fn berry_seedlings_and_stationary_players_do_not_take_damage() {
    let (mut state, entity, pos) = flat_player_fixture();
    place(&mut state, pos, "sweet_berry_bush", &[("age", "0")]);
    assert!(!sample(&mut state, entity, DamageKind::SweetBerryBush));
    place(&mut state, pos, "sweet_berry_bush", &[("age", "3")]);
    assert!(!sample(&mut state, entity, DamageKind::SweetBerryBush));
    let slot = state.entities.slot_of(entity).unwrap();
    state
        .entities
        .set_pos(slot, [pos[0] + 0.01, pos[1], pos[2]]);
    assert!(sample(&mut state, entity, DamageKind::SweetBerryBush));
    assert!(!sample(&mut state, entity, DamageKind::SweetBerryBush));
}

#[test]
fn magma_support_damage_is_nonigniting_and_sneak_exempt() {
    let (mut state, entity, pos) = flat_player_fixture();
    place(
        &mut state,
        [pos[0], pos[1] - 1.0, pos[2]],
        "magma_block",
        &[],
    );
    assert!(sample(&mut state, entity, DamageKind::HotFloor));
    let slot = state.entities.slot_of(entity).unwrap();
    assert_eq!(state.entities.body(slot).burn_ticks, 0);
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags |= 2;
    assert!(!sample(&mut state, entity, DamageKind::HotFloor));
}

#[test]
fn magma_wall_contact_is_not_hot_floor_without_magma_support() {
    let (mut state, entity, pos) = flat_player_fixture();
    place(&mut state, pos, "magma_block", &[]);
    assert!(!sample(&mut state, entity, DamageKind::HotFloor));
    place(
        &mut state,
        [pos[0], pos[1] - 1.0, pos[2]],
        "magma_block",
        &[],
    );
    assert!(sample(&mut state, entity, DamageKind::HotFloor));
}

#[test]
fn hazard_free_sections_still_decay_burn_and_suffocate() {
    let (mut state, entity, pos) = flat_player_fixture();
    let slot = state.entities.slot_of(entity).unwrap();
    // Hazard-free stone/air sections: the gate skips body samples, but
    // suffocation and burn decay must still run every tick.
    place(&mut state, [pos[0], pos[1] + 1.0, pos[2]], "stone", &[]);
    crate::contact_hazards::ignite(&mut state, entity, 40);
    assert!(sample(&mut state, entity, DamageKind::Suffocation));
    assert_eq!(state.entities.body(slot).burn_ticks, 39);
    assert_ne!(state.entities.row(slot).appearance.shared_flags & 1, 0);
}

#[test]
fn stationary_overlay_hazard_still_burns_through_the_gate() {
    let (mut state, entity, pos) = flat_player_fixture();
    // Fresh overlay write, deliberately unfolded: the gate must fail open and
    // the exact body scan still observes the new hazard (no stale cache).
    let block = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:lava").unwrap())
        .unwrap();
    let id = block
        .states
        .iter()
        .copied()
        .find(|id| {
            state
                .registry
                .blocks
                .by_id(*id)
                .unwrap()
                .properties
                .iter()
                .any(|(k, v)| k == "level" && v == "0")
        })
        .unwrap()
        .0;
    state.world.set_block(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
        id,
    );
    assert!(!state.world.overlay_is_empty());
    assert!(sample(&mut state, entity, DamageKind::Lava));
}

#[test]
fn lit_campfires_need_posture_and_unlit_ones_stay_cold() {
    let (mut state, entity, pos) = flat_player_fixture();
    place(&mut state, pos, "campfire", &[("lit", "true")]);
    assert!(sample(&mut state, entity, DamageKind::Campfire));
    let slot = state.entities.slot_of(entity).unwrap();
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags |= 2;
    assert!(!sample(&mut state, entity, DamageKind::Campfire));
    place(&mut state, pos, "campfire", &[("lit", "false")]);
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags &= !2;
    assert!(!sample(&mut state, entity, DamageKind::Campfire));
}

#[test]
fn cactus_damage_routes_through_merge_and_respects_creative_immunity() {
    let (mut state, entity, pos) = flat_player_fixture();
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    place(&mut state, pos, "cactus", &[]);
    state.tick = 100;
    run_tick(&mut state, &[]);
    assert_eq!(state.players[&entity].vitals.health, 19.0);
    state.players.get_mut(&entity).unwrap().game_mode = crate::commands::GameMode::Creative;
    state.tick += 20;
    run_tick(&mut state, &[]);
    assert_eq!(state.players[&entity].vitals.health, 19.0);
}

#[test]
fn ignition_extends_timer_water_extinguishes_and_rule_suppresses_damage() {
    let (mut state, entity, pos) = flat_player_fixture();
    let slot = state.entities.slot_of(entity).unwrap();
    crate::contact_hazards::ignite(&mut state, entity, 100);
    crate::contact_hazards::ignite(&mut state, entity, 20);
    assert_eq!(state.entities.body(slot).burn_ticks, 100);
    place(&mut state, pos, "water", &[("level", "0")]);
    assert!(!sample(&mut state, entity, DamageKind::OnFire));
    assert_eq!(state.entities.body(slot).burn_ticks, 0);
    assert_eq!(state.entities.row(slot).appearance.shared_flags & 1, 0);
    place(&mut state, pos, "lava", &[("level", "0")]);
    state.gamerules.insert("fire_damage".into(), "false".into());
    state.entities.row_mut(slot).protected_until = 0;
    state.tick = 100;
    run_tick(&mut state, &[]);
    assert_eq!(state.players[&entity].vitals.health, 20.0);
    assert_eq!(state.entities.body(slot).burn_ticks, 0);
}

#[test]
fn retained_player_fire_survives_checkpoint_tail_and_rejoin() {
    let (mut state, entity, _) = flat_player_fixture();
    crate::contact_hazards::ignite(&mut state, entity, 123);
    let record = crate::core::player_record_for(&state, entity, state.tick).unwrap();
    let uuid = uuid::Uuid::new_v4();
    let dir = std::env::temp_dir().join(format!("contact-fire-{}", uuid));
    let config = crate::CoreConfig::new(dir.clone(), 42);
    let loaded = crate::native_checkpoint::tests::roundtrip_player(
        &config,
        &uuid.to_string(),
        &record,
        &state.registry.items,
    )
    .unwrap();
    assert_eq!(loaded.burn_ticks, 123);
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let replay = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &[(output.tick, output.tick, entries)],
    );
    assert!(replay.players.values().any(|p| p.burn_ticks == 122));
    let (mut restored, _, _) = flat_player_fixture();
    restored.offline_players.insert(uuid, loaded);
    run_tick(
        &mut restored,
        &[PlayerInput::Join {
            connection: ConnectionId(9),
            uuid,
            name: "burned".into(),
            permission_level: 0,
        }],
    );
    let id = restored.connections.entity_of(ConnectionId(9)).unwrap();
    let slot = restored.entities.slot_of(id).unwrap();
    let body = restored.entities.body(slot);
    let row = restored.entities.row(slot);
    assert_eq!(body.burn_ticks, 122);
    assert_ne!(row.appearance.shared_flags & 1, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pvp_blocks_attributed_projectiles_before_health_fire_and_wear() {
    let (mut state, victim, pos) = flat_player_fixture();
    let attacker = super::join(&mut state, 2, "attacker");
    let slot = state.entities.slot_of(victim).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    state.tick = 100;
    state.gamerules.insert("pvp".into(), "false".into());
    let source = crate::intent::DamageSource::Projectile {
        projectile: EntityId(1000),
        direct_type_id: mc_data::entity_types::entity_type_contract_26_1_2_by_name(
            "minecraft:arrow",
        )
        .unwrap()
        .protocol_id,
        owner: attacker,
        velocity: [1.0, 0.0],
        punch: 2,
        flame: true,
    };
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 6.0,
            kind: DamageKind::Projectile,
            source: Some(source),
        },
    ));
    state.commit_merge_effects();
    assert_eq!(state.players[&victim].vitals.health, 20.0);
    assert_eq!(state.entities.body(slot).burn_ticks, 0);
    assert_eq!(state.entities.pos(slot), pos);
    state.gamerules.insert("pvp".into(), "true".into());
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 6.0,
            kind: DamageKind::Projectile,
            source: Some(source),
        },
    ));
    state.commit_merge_effects();
    assert_eq!(state.players[&victim].vitals.health, 14.0);
    assert_eq!(state.entities.body(slot).burn_ticks, 100);
}

#[test]
fn mob_hazard_matching_uses_cached_ids_for_living_and_nonliving_kinds() {
    let (mut state, _, pos) = flat_player_fixture();
    let id_of = |state: &TickState, name: &str| {
        state
            .registry
            .entity_types
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap()
    };
    let cow = id_of(&state, "minecraft:cow");
    let boat = id_of(&state, "minecraft:acacia_boat");
    let at = [pos[0] + 2.0, pos[1], pos[2]];
    let cow = state.spawn_mob(cow, at, 10.0);
    let boat = state.spawn_mob(boat, at, 10.0);
    place(&mut state, at, "lava", &[("level", "0")]);
    assert!(
        sample(&mut state, cow, DamageKind::Lava),
        "living mob in lava still takes lava damage"
    );
    assert!(
        !sample(&mut state, boat, DamageKind::Lava),
        "non-living mob kind stays exempt from contact hazards"
    );
}

#[test]
fn scratch_caches_reset_each_pass_and_gate_the_run() {
    let (mut state, entity, pos) = flat_player_fixture();
    // Warm the real pass's negative result before changing its palette.
    assert!(!sample(&mut state, entity, DamageKind::Lava));
    assert!(!sample(&mut state, entity, DamageKind::Suffocation));
    place(&mut state, pos, "lava", &[("level", "0")]);
    assert!(sample(&mut state, entity, DamageKind::Lava));
    // A conservative section hit may survive in a palette after a cell clears;
    // the exact cell scan must still decide the resulting damage.
    place(&mut state, pos, "air", &[]);
    assert!(!sample(&mut state, entity, DamageKind::Lava));
    place(&mut state, [pos[0], pos[1] + 1.0, pos[2]], "stone", &[]);
    assert!(sample(&mut state, entity, DamageKind::Suffocation));
}

#[test]
fn hazard_free_pass_still_suffocates_and_decays_burn() {
    let (mut state, entity, pos) = flat_player_fixture();
    // Section gates must not suppress exact eye damage or retained burn clocks
    // when the body's actual cells contain no ignition hazard.
    place(&mut state, [pos[0], pos[1] + 1.0, pos[2]], "stone", &[]);
    crate::contact_hazards::ignite(&mut state, entity, 40);
    let slot = state.entities.slot_of(entity).unwrap();
    assert!(sample(&mut state, entity, DamageKind::Suffocation));
    assert_eq!(state.entities.body(slot).burn_ticks, 39);
}

#[test]
fn footprint_eye_and_support_stay_exact_across_section_and_chunk_borders() {
    let (mut state, entity, _) = flat_player_fixture();
    let chunks = [
        crate::boundary::ChunkPos { x: 0, z: 0 },
        crate::boundary::ChunkPos { x: 1, z: 0 },
    ];
    state
        .world
        .ensure_blocking(&chunks.into_iter().map(|chunk| (chunk, 0)).collect());
    state
        .world
        .set_active_chunks(chunks.into_iter().collect(), state.tick);
    // Feet/support occupy section 64..79; the eye lies in section 80..95.
    // The body also crosses x=16 into the neighboring chunk.
    let pos = [15.9, 79.0, 8.5];
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, pos);
    for x in 14..=17 {
        for z in 7..=9 {
            place(&mut state, [f64::from(x), 78.0, f64::from(z)], "stone", &[]);
            for y in 79..=81 {
                place(
                    &mut state,
                    [f64::from(x), f64::from(y), f64::from(z)],
                    "air",
                    &[],
                );
            }
        }
    }
    // The first actual hazard is only across the chunk border.
    place(&mut state, [16.0, pos[1], pos[2]], "cactus", &[]);
    assert!(sample(&mut state, entity, DamageKind::Cactus));
    place(&mut state, [16.0, pos[1], pos[2]], "air", &[]);
    place(&mut state, [pos[0], 78.0, pos[2]], "magma_block", &[]);
    place(&mut state, [pos[0], 80.0, pos[2]], "stone", &[]);
    assert!(sample(&mut state, entity, DamageKind::HotFloor));
    assert!(sample(&mut state, entity, DamageKind::Suffocation));
}

#[test]
fn single_column_scan_matches_per_cell_reads_with_overlay_fallback() {
    let (mut state, entity, pos) = flat_player_fixture();
    // Whole scan inside one column: borrowed-chunk reads must observe the
    // same body hazard, magma support and suffocating eye as `block_at`.
    place(&mut state, pos, "lava", &[("level", "0")]);
    place(
        &mut state,
        [pos[0], pos[1] - 1.0, pos[2]],
        "magma_block",
        &[],
    );
    place(&mut state, [pos[0], pos[1] + 1.0, pos[2]], "stone", &[]);
    assert!(state.world.overlay_is_empty());
    assert!(sample(&mut state, entity, DamageKind::Lava));
    assert!(sample(&mut state, entity, DamageKind::HotFloor));
    assert!(sample(&mut state, entity, DamageKind::Suffocation));
    // Unfolded overlay write in the same column: the exact per-cell path
    // must still observe the fresh hazard on an overlay-nonempty pass.
    place(&mut state, pos, "air", &[]);
    assert!(!sample(&mut state, entity, DamageKind::Lava));
    let block = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:cactus").unwrap())
        .unwrap();
    let id = block.default.0;
    state.world.set_block(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
        id,
    );
    assert!(!state.world.overlay_is_empty());
    assert!(sample(&mut state, entity, DamageKind::Cactus));
}

#[test]
fn warm_negative_memo_reuses_quiet_pass_and_wakes_on_new_cells() {
    let (mut state, _, pos) = flat_player_fixture();
    let cow = state.registry.cow_type_id;
    let at = [pos[0] + 2.0, pos[1], pos[2]];
    let mob = state.spawn_mob(cow, at, 10.0);
    let slot = state.entities.slot_of(mob).unwrap();
    // Warm the negative memo on truly empty cells.
    assert!(!sample(&mut state, mob, DamageKind::Cactus));
    let stamp = state.contact_hazards_scratch.negative[slot.0 as usize]
        .expect("quiet pass stores a negative stamp");
    assert_eq!(stamp.entity, mob);
    // Same raw position reuses the memo without geometric reads.
    assert!(!sample(&mut state, mob, DamageKind::Cactus));
    // Sub-cell drift inside the same cells keeps the memo alive via the
    // folded content revision.
    let drift = [at[0] + 0.02, at[1], at[2]];
    state.entities.set_pos(slot, drift);
    assert!(!sample(&mut state, mob, DamageKind::Cactus));
    assert_eq!(
        state.contact_hazards_scratch.negative[slot.0 as usize]
            .unwrap()
            .position,
        drift,
        "same-cell drift refreshes the raw-position fast path"
    );
    // A live overlay edit must wake the pass even before the fold updates
    // chunk revisions.
    let block = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:cactus").unwrap())
        .unwrap();
    let id = block.default.0;
    state.world.set_block(
        drift[0].floor() as i32,
        drift[1].floor() as i32,
        drift[2].floor() as i32,
        id,
    );
    assert!(sample(&mut state, mob, DamageKind::Cactus));
    // Re-warm on air, then commit an edit before the next pass. This tests the
    // folded revision independently of the overlay wake above.
    place(&mut state, drift, "air", &[]);
    assert!(!sample(&mut state, mob, DamageKind::Cactus));
    place(&mut state, drift, "cactus", &[]);
    assert!(sample(&mut state, mob, DamageKind::Cactus));
    // A pre-existing hazard beyond a warm box must wake on new cells without
    // an intervening world write.
    let quiet = [at[0] + 2.0, at[1], at[2]];
    let hazard = [at[0] + 4.0, at[1], at[2]];
    place(&mut state, hazard, "cactus", &[]);
    state.entities.set_pos(slot, quiet);
    assert!(!sample(&mut state, mob, DamageKind::Cactus));
    assert!(state.contact_hazards_scratch.negative[slot.0 as usize].is_some());
    state.entities.set_pos(slot, hazard);
    assert!(sample(&mut state, mob, DamageKind::Cactus));
}

#[test]
fn support_and_eye_cells_wake_when_body_box_is_unchanged() {
    let (mut state, _, pos) = flat_player_fixture();
    let cow = state.registry.cow_type_id;
    // Cow half-width 0.45: integral `at` x keeps the body box cells while a
    // -0.4 step crosses the foot-center support cell boundary.
    let at = [pos[0].floor() + 3.0, pos[1], pos[2].floor() + 0.5];
    let moved = [at[0] - 0.4, at[1], at[2]];
    // Plant the magma first so no world edit (and no revision bump) sits
    // between the warm pass and the step: only the support-probe comparison
    // can wake the re-probe.
    place(
        &mut state,
        [moved[0], moved[1] - 1.0, moved[2]],
        "magma_block",
        &[],
    );
    let mob = state.spawn_mob(cow, at, 10.0);
    let slot = state.entities.slot_of(mob).unwrap();
    assert!(!sample(&mut state, mob, DamageKind::HotFloor));
    assert!(
        state.contact_hazards_scratch.negative[slot.0 as usize].is_some(),
        "quiet pass stores a negative stamp"
    );
    // Same body box, new support cell: the exact probe must observe magma.
    // `at` x is integral, so -0.4 crosses the foot-center cell while the 0.45
    // half-width keeps xmin/xmax identical.
    state.entities.set_pos(slot, moved);
    assert!(sample(&mut state, mob, DamageKind::HotFloor));
    // Cow height 1.4 puts the eye at +1.26: a small vertical nudge crosses
    // the eye cell within an unchanged body box. Plant the wall first so
    // only the exact eye-probe comparison can wake the re-probe.
    let tall = [pos[0] - 2.0, 64.7, pos[2]];
    let risen = [tall[0], 64.8, tall[2]];
    place(&mut state, [risen[0], 66.0, risen[2]], "stone", &[]);
    state.entities.set_pos(slot, tall);
    for y in 63..=65 {
        place(&mut state, [tall[0], f64::from(y), tall[2]], "air", &[]);
    }
    assert!(!sample(&mut state, mob, DamageKind::Suffocation));
    let eye_stamp = state.contact_hazards_scratch.negative[slot.0 as usize]
        .expect("body and old eye cells are clear");
    assert_eq!((eye_stamp.min.1, eye_stamp.max.1), (64, 66));
    assert_eq!(eye_stamp.probes[1][1], 65);
    state.entities.set_pos(slot, risen);
    assert!(sample(&mut state, mob, DamageKind::Suffocation));
}

#[test]
fn ignited_quiet_mob_keeps_burn_and_emits_onfire_on_cadence() {
    let (mut state, _, pos) = flat_player_fixture();
    let cow = state.registry.cow_type_id;
    let at = [pos[0] + 2.0, pos[1], pos[2]];
    let mob = state.spawn_mob(cow, at, 10.0);
    let slot = state.entities.slot_of(mob).unwrap();
    assert!(!sample(&mut state, mob, DamageKind::Lava));
    assert!(
        state.contact_hazards_scratch.negative[slot.0 as usize].is_some(),
        "quiet pass stores a negative stamp"
    );
    // External ignition of the stationary body: the memo must not suppress
    // the retained burn clock or the shared burning bit.
    crate::contact_hazards::ignite(&mut state, mob, 41);
    state.tick = 41;
    assert!(!sample(&mut state, mob, DamageKind::OnFire));
    assert_eq!(state.entities.body(slot).burn_ticks, 40);
    assert_ne!(state.entities.row(slot).appearance.shared_flags & 1, 0);
    // Cadence tick: the timer decays and the OnFire intent still fires.
    state.tick = 60;
    assert!(sample(&mut state, mob, DamageKind::OnFire));
    assert_eq!(state.entities.body(slot).burn_ticks, 39);
    // Extinguish at the correct tick and clear the shared bit.
    state.entities.body_mut(slot).burn_ticks = 1;
    state.tick = 61;
    assert!(!sample(&mut state, mob, DamageKind::OnFire));
    assert_eq!(state.entities.body(slot).burn_ticks, 0);
    assert_eq!(state.entities.row(slot).appearance.shared_flags & 1, 0);
    // Raw absence can also be banked on the first probe of an already-burning
    // actor, rather than waiting for its retained timer to expire.
    let other = state.spawn_mob(cow, [at[0] + 2.0, at[1], at[2]], 10.0);
    crate::contact_hazards::ignite(&mut state, other, 5);
    assert!(!sample(&mut state, other, DamageKind::OnFire));
    let other_slot = state.entities.slot_of(other).unwrap();
    assert_eq!(state.entities.body(other_slot).burn_ticks, 4);
    assert!(state.contact_hazards_scratch.negative[other_slot.0 as usize].is_some());
}

#[test]
fn stale_burning_bit_clears_without_fire_damage() {
    let (mut state, _, pos) = flat_player_fixture();
    let cow = state.registry.cow_type_id;
    let at = [pos[0] + 2.0, pos[1], pos[2]];
    let mob = state.spawn_mob(cow, at, 10.0);
    let slot = state.entities.slot_of(mob).unwrap();
    // Warm the negative memo on truly empty cells.
    assert!(!sample(&mut state, mob, DamageKind::Cactus));
    assert!(
        state.contact_hazards_scratch.negative[slot.0 as usize].is_some(),
        "quiet pass stores a negative stamp"
    );
    // Stale metadata: burning bit set while the retained timer is zero.
    // Pure classifier returns Quiet, so the serial retained burn clears the
    // bit without emitting fire damage.
    let flags =
        &mut std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags;
    *flags |= 1;
    assert_eq!(state.entities.body(slot).burn_ticks, 0);
    assert!(!sample(&mut state, mob, DamageKind::OnFire));
    assert_eq!(state.entities.body(slot).burn_ticks, 0);
    assert_eq!(
        state.entities.row(slot).appearance.shared_flags & 1,
        0,
        "serial retained burn clears the stale burning bit"
    );
    // Serial and classifier paths agree on the cleared state.
    let mut serial = Vec::new();
    crate::contact_hazards::run_all_serial(&mut state, &mut serial);
    let mut parallel = Vec::new();
    crate::contact_hazards::run_with(&mut state, &mut parallel, 0);
    assert_eq!(serial, parallel, "serial vs classifier intents match");
    assert!(
        !serial.iter().any(|intent| matches!(
            intent.kind,
            IntentKind::DamageEntity { target, kind: DamageKind::OnFire, .. }
            if target == mob
        )),
        "no fire damage for a stale burning bit"
    );
}

#[test]
fn compacted_slot_cannot_reuse_stale_identity() {
    let (mut state, _, pos) = flat_player_fixture();
    let cow = state.registry.cow_type_id;
    let first = state.spawn_mob(cow, [pos[0] + 2.0, pos[1], pos[2]], 10.0);
    let slot = state.entities.slot_of(first).unwrap();
    assert!(!sample(&mut state, first, DamageKind::Lava));
    assert!(
        state.contact_hazards_scratch.negative[slot.0 as usize].is_some(),
        "quiet pass stores a negative stamp"
    );
    // Swap-remove compaction moves the tail into the freed slot; the stale
    // identity must not silence the replacement's hazard probe.
    let second = state.spawn_mob(cow, [pos[0] + 2.0, pos[1], pos[2]], 10.0);
    let tail = state.entities.slot_of(second).unwrap();
    state.remove_entity(slot);
    let moved = state.entities.slot_of(second).unwrap();
    assert_ne!(
        tail, moved,
        "compaction must move the tail into the freed slot"
    );
    assert!(!sample(&mut state, second, DamageKind::Lava));
    assert_eq!(
        state.contact_hazards_scratch.negative[moved.0 as usize]
            .unwrap()
            .entity,
        second,
        "same-cell replacement re-probes its own identity before any world edit"
    );
    place(
        &mut state,
        [pos[0] + 2.0, pos[1], pos[2]],
        "lava",
        &[("level", "0")],
    );
    assert!(sample(&mut state, second, DamageKind::Lava));
}

#[test]
fn conditional_contact_never_memoizes_absence() {
    let (mut state, entity, pos) = flat_player_fixture();
    let slot = state.entities.slot_of(entity).unwrap();
    // Sneaking suppresses magma damage but the raw support contact must block
    // the negative memo.
    place(
        &mut state,
        [pos[0], pos[1] - 1.0, pos[2]],
        "magma_block",
        &[],
    );
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags |= 2;
    assert!(!sample(&mut state, entity, DamageKind::HotFloor));
    assert!(
        state.contact_hazards_scratch.negative[slot.0 as usize].is_none(),
        "sneaking over magma must not memo absence"
    );
    // A stationary player inside fruiting berries takes no damage, but the raw
    // contact must still block the memo so later motion cannot reuse it.
    place(&mut state, [pos[0], pos[1] - 1.0, pos[2]], "stone", &[]);
    place(&mut state, pos, "sweet_berry_bush", &[("age", "3")]);
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags &= !2;
    assert!(!sample(&mut state, entity, DamageKind::SweetBerryBush));
    assert!(
        state.contact_hazards_scratch.negative[slot.0 as usize].is_none(),
        "stationary berry contact must not memo absence"
    );
    state
        .entities
        .set_pos(slot, [pos[0] + 0.01, pos[1], pos[2]]);
    assert!(sample(&mut state, entity, DamageKind::SweetBerryBush));
}
#[path = "item_hazards_tests.rs"]
mod item_tests;

/// Dense mob field over the fixture floor: hazards, burning, dead and
/// non-living rows, then `passes` rounds of mixed raw/cell/stationary motion.
/// Returns a per-pass transcript of intents and all contact-visible state.
fn contact_field(
    mobs: usize,
    passes: u64,
    run: impl Fn(&mut TickState, &mut Vec<Intent>),
) -> Vec<String> {
    let (mut state, _, pos) = flat_player_fixture();
    let cow = state.registry.cow_type_id;
    let boat = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:acacia_boat").unwrap())
        .unwrap();
    let at = |index: usize, pass: u64| {
        let mix = (index as u64 + 1)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(pass.wrapping_mul(0xbf58_476d_1ce4_e5b9));
        let mix = (mix ^ (mix >> 29)).wrapping_mul(0x94d0_49bb_1331_11eb);
        let (a, b, c) = (mix & 0xff, (mix >> 8) & 0xff, (mix >> 16) & 0xff);
        [
            pos[0] - 4.5 + (a as f64 / 255.0) * 9.0,
            pos[1] + if c % 11 == 0 { 0.4 } else { 0.0 },
            pos[2] - 2.5 + (b as f64 / 255.0) * 5.0,
        ]
    };
    let mut ids = Vec::new();
    for index in 0..mobs {
        let kind = if index % 97 == 0 { boat } else { cow };
        ids.push(state.spawn_mob(kind, at(index, 0), 10.0));
    }
    for (index, name, props) in [
        (1, "lava", &[("level", "0")][..]),
        (2, "cactus", &[]),
        (3, "sweet_berry_bush", &[("age", "3")]),
        (4, "fire", &[]),
        (5, "stone", &[]),
        (6, "water", &[("level", "0")]),
    ] {
        let spot = at(index * 7, 0);
        place(&mut state, [spot[0], spot[1] + 0.5, spot[2]], name, props);
    }
    for (index, id) in ids.iter().enumerate() {
        let slot = state.entities.slot_of(*id).unwrap();
        if index % 53 == 0 {
            crate::contact_hazards::ignite(&mut state, *id, 30 + index as u32 % 40);
        }
        if index % 211 == 0 {
            state.entities.body_mut(slot).alive = false;
        }
    }
    let mut transcript = Vec::new();
    for pass in 0..passes {
        state.tick = 18 + pass;
        if pass > 0 {
            for (index, id) in ids.iter().enumerate() {
                let Some(slot) = state.entities.slot_of(*id) else {
                    continue;
                };
                match (index as u64 + pass * 3) % 8 {
                    0 | 1 => state.entities.set_pos(slot, at(index, pass)),
                    2 => {
                        let old = state.entities.pos(slot);
                        state
                            .entities
                            .set_pos(slot, [old[0] + 0.003, old[1], old[2]]);
                    }
                    _ => {}
                }
            }
        }
        if pass == 2 {
            let spot = at(11, 0);
            place(
                &mut state,
                [spot[0], spot[1] + 0.5, spot[2]],
                "lava",
                &[("level", "0")],
            );
        }
        let mut intents = Vec::new();
        state.appearance_changed.clear();
        run(&mut state, &mut intents);
        let mut rows = String::new();
        for slot in 0..state.entities.len() {
            let slot = crate::entities::EntitySlot(slot as u32);
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            let stamp = state.contact_hazards_scratch.negative[slot.0 as usize]
                .map(|stamp| (stamp.entity, stamp.min, stamp.max, stamp.position));
            rows += &format!(
                "{:?} {} {} {:?}\n",
                body.id, body.burn_ticks, row.appearance.shared_flags, stamp
            );
        }
        transcript.push(format!(
            "{intents:?}\n{:?}\n{rows}",
            state.appearance_changed
        ));
    }
    transcript
}

#[test]
fn parallel_classifier_matches_all_serial_on_any_thread_count() {
    // > 2 jobs of rows so the pool really splits the classification.
    let mobs = crate::contact_hazards::MIN_PARALLEL_ROWS / 2 * 3;
    let reference = contact_field(mobs, 6, crate::contact_hazards::run_all_serial);
    assert!(
        reference.iter().any(|pass| pass.contains("DamageEntity")),
        "fixture must exercise real damage intents"
    );
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let got = pool.install(|| {
            contact_field(mobs, 6, |state, intents| {
                crate::contact_hazards::run_with(state, intents, 0);
            })
        });
        assert_eq!(got.len(), reference.len());
        for (pass, (got, want)) in got.iter().zip(&reference).enumerate() {
            assert!(got == want, "pass {pass} diverged on {threads} thread(s)");
        }
    }
}

/// Release micro: `cargo test --release -p mc-core --lib contact_classifier_micro -- --ignored --nocapture`.
/// 100k mobs in the fixture; B = all-serial scan, A = parallel classifier +
/// serial apply, each timed over warm passes (A/B alternate: B,A,B,A).
#[test]
#[ignore = "release micro: prints contact pass timings"]
fn contact_classifier_micro() {
    let (mut state, _, pos) = flat_player_fixture();
    let cow = state.registry.cow_type_id;
    let ids: Vec<_> = (0..100_000_u64)
        .map(|index| {
            let mix = (index + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let (a, b) = ((mix >> 8) & 0xff, (mix >> 24) & 0xff);
            let at = [
                pos[0] - 4.5 + a as f64 / 255.0 * 9.0,
                pos[1],
                pos[2] - 2.5 + b as f64 / 255.0 * 5.0,
            ];
            state.spawn_mob(cow, at, 10.0)
        })
        .collect();
    let slots: Vec<_> = ids
        .iter()
        .map(|id| state.entities.slot_of(*id).unwrap())
        .collect();
    for moving in [false, true] {
        for (label, parallel) in [("B", false), ("A", true), ("B", false), ("A", true)] {
            let mut intents = Vec::new();
            crate::contact_hazards::run_all_serial(&mut state, &mut intents); // warm stamps
            let mut spent = std::time::Duration::ZERO;
            for pass in 0..20_u32 {
                if moving {
                    for slot in slots.iter().step_by(10) {
                        let old = state.entities.pos(*slot);
                        let dx = if pass % 2 == 0 { 0.003 } else { -0.003 };
                        state.entities.set_pos(*slot, [old[0] + dx, old[1], old[2]]);
                    }
                }
                intents.clear();
                let started = std::time::Instant::now();
                if parallel {
                    crate::contact_hazards::run(&mut state, &mut intents);
                } else {
                    crate::contact_hazards::run_all_serial(&mut state, &mut intents);
                }
                spent += started.elapsed();
            }
            println!(
                "contact_classifier_micro moving={moving} {label}: {:.3} ms/pass",
                spent.as_secs_f64() * 1000.0 / 20.0
            );
        }
    }
}

#[test]
fn parity_lava_ignites_fifteen_seconds_then_retains_afterburn() {
    let (mut state, entity, pos) = flat_player_fixture();
    place(&mut state, pos, "lava", &[("level", "0")]);
    assert!(sample(&mut state, entity, DamageKind::Lava));
    let slot = state.entities.slot_of(entity).unwrap();
    assert_eq!(state.entities.body(slot).burn_ticks, 299);
    place(&mut state, pos, "air", &[]);
    sample(&mut state, entity, DamageKind::OnFire);
    assert_eq!(state.entities.body(slot).burn_ticks, 298);
}
