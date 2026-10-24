use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::dimensions::Dimension;
use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
use crate::tick::run_tick;
use std::sync::Arc;

fn state() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator =
        Arc::new(mc_worldgen::EndGenerator::try_new(42, Arc::clone(&registry.blocks)).unwrap());
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 70, 0],
        Dimension::End.geometry(),
    );
    let mut state = TickState::new(registry, world, 0, 4096);
    state.dimension = Dimension::End;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let want = (-4..=4)
        .flat_map(|x| (-4..=4).map(move |z| (mc_world::ChunkPos { x, z }, 0)))
        .collect();
    state.world.ensure_blocking(&want);
    state
}
fn join(state: &mut TickState) {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: Uuid::from_u128(12345),
            name: "fighter".into(),
            permission_level: 0,
        }],
    );
}
fn running() -> TickState {
    let mut s = state();
    join(&mut s);
    s
}
fn full_restore(source: &TickState) -> TickState {
    let mut target = state();
    target.tick = source.tick;
    target.end_fight =
        serde_json::from_slice(&serde_json::to_vec(&source.end_fight).unwrap()).unwrap();
    let (_, chunks, _) = crate::core::checkpoint_data_for(source, source.tick);
    for chunk in chunks.values() {
        for record in &chunk.entities {
            let id = record.wire_id;
            let type_id = type_id(source, &record.id);
            let slot = source.entities.slot_of_uuid(record.uuid).unwrap();
            let row = source.entities.row(slot);
            let image = crate::journal::ReplayEntity {
                uuid: record.uuid.to_string(),
                type_id,
                pos: record.pos,
                vel: record.motion.map(|v| v * 20.0),
                yaw: record.yaw,
                pitch: record.pitch,
                ground: record.on_ground,
                health: record.health,
                alive: true,
                age: row.age,
                breed_cooldown: row.breed_cooldown,
                pickup_delay: source.entities.body(slot).pickup_delay,
                item: None,
                appearance: crate::journal::EntityAppearanceImage::from(row.appearance.as_ref()),
                active_effects: row.active_effects.clone(),
                villager: None,
                state: crate::journal::EntityStateImage::from_row(source.entities.body(slot), row),
                projectile: row.projectile.as_deref().cloned(),
                tick: source.tick,
            };
            crate::core::restore_entity(&mut target, id, &image).unwrap();
        }
    }
    restore(&mut target).unwrap();
    crate::dimensions::validate_identity(&mut target).unwrap();
    target
}
#[test]
fn first_entry_spawns_once_and_builds_ten_crystals_two_cages() {
    let mut s = running();
    assert!(s.end_fight.initialized);
    assert_eq!(s.end_fight.crystals.len(), 10);
    let uuid = s.end_fight.dragon_uuid.unwrap();
    for _ in 0..3 {
        run_tick(&mut s, &[]);
    }
    assert_eq!(s.end_fight.dragon_uuid, Some(uuid));
    assert_eq!(
        s.entities
            .slots()
            .filter(|slot| is_dragon(&s, *slot))
            .count(),
        1
    );
    let mut guarded = 0;
    for index in 0..10 {
        let [x, y, z] = pillars::position(&s, index);
        let x = x.floor() as i32;
        let y = y as i32 - 1;
        let z = z.floor() as i32;
        assert_eq!(
            s.registry
                .blocks
                .by_id(mc_world::BlockStateId(s.world.block_at(x, y, z).unwrap()))
                .unwrap()
                .block
                .id
                .path(),
            "bedrock"
        );
        if s.registry
            .blocks
            .by_id(mc_world::BlockStateId(
                s.world.block_at(x - 2, y + 2, z).unwrap(),
            ))
            .unwrap()
            .block
            .id
            .path()
            == "iron_bars"
        {
            guarded += 1;
        }
    }
    assert_eq!(guarded, 2);
}
#[test]
fn multipart_damage_routes_head_and_reduces_body_and_reserves_ids() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    let id = s.entities.body(slot).id;
    assert!(s.entities.wire_id_high_water() >= id.0 + 8);
    for offset in 1..=8 {
        assert!(part_bounds(&s, EntityId(id.0 + offset)).is_some());
    }
    let actor = s.connections.entity_of(ConnectionId(1)).unwrap();
    let mut intents = vec![Intent {
        actor,
        seq: 0,
        kind: IntentKind::DamageEntity {
            target: EntityId(id.0 + 3),
            amount: 20.0,
            kind: crate::survival::DamageKind::Melee,
            source: None,
        },
        footprint: Footprint {
            entity_writes: vec![EntityId(id.0 + 3)],
            ..Default::default()
        },
    }];
    s.merge(&mut intents);
    assert_eq!(s.merge_damage[0].1.amount, 6.0);
    let (deaths, _, _, _) = s.commit_merge_effects();
    assert!(deaths.is_empty());
    assert_eq!(s.entities.row(slot).health, 194.0);
}
#[test]
fn mid_fight_native_checkpoint_and_wal_restart_keep_phase_target_and_position() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    s.end_fight.phase = Phase::Charging;
    s.end_fight.phase_ticks = 7;
    s.end_fight.fly_target = [18.0, 90.0, 3.0];
    s.end_fight.target = Some(Uuid::from_u128(12345));
    s.entities.set_pos(slot, [12.0, 91.0, 2.0]);
    let directory = tempfile::tempdir().unwrap();
    native_fixture(&s, directory.path());
    assert_eq!(
        crate::native_checkpoint::tests::read_manifest(&config(directory.path()))
            .unwrap()
            .dimensions[&Dimension::End]
            .end_fight
            .as_ref()
            .unwrap(),
        &s.end_fight,
    );
    let mut restored = full_restore(&s);
    assert_eq!(restored.end_fight, s.end_fight);
    let (out, _, _) = run_tick(&mut s, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut s, &out);
    let wal = directory.path().join("journal");
    let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
    writer.append(1, out.tick, &entries).unwrap();
    writer.flush().unwrap();
    let base = Replay {
        end_fight: Some(restored.end_fight.clone()),
        ..Default::default()
    };
    let replay = replay_batches(base, &read_complete_batches_after(&wal, 0).unwrap());
    restored.end_fight = replay.end_fight.unwrap();
    assert_eq!(restored.end_fight, s.end_fight);
    let id = s.entities.body(slot).id.0;
    assert_eq!(replay.entities[&id].pos, s.entities.pos(slot));
}
#[test]
fn corrupt_fight_records_fail_closed_with_field_diagnostics() {
    let s = running();
    let mut json = serde_json::to_value(&s.end_fight).unwrap();
    json["gateways"] = serde_json::json!([1, 1]);
    assert!(
        serde_json::from_value::<Fight>(json)
            .unwrap_err()
            .to_string()
            .contains("duplicate index")
    );
    let mut fight = s.end_fight.clone();
    fight.crystals.push(fight.crystals[0].clone());
    assert!(
        fight
            .validate()
            .unwrap_err()
            .contains("duplicate crystal UUID")
    );
    let mut timed = s.end_fight.clone();
    timed.dragon_killed = true;
    timed.respawn = Some(Respawn {
        crystals: timed.crystals[..4]
            .iter()
            .map(|c| c.uuid)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap(),
        stage: 0,
        time: 1,
    });
    assert!(
        timed
            .validate()
            .unwrap_err()
            .contains("invalid respawn stage/time")
    );
    let mut broken = s.end_fight.clone();
    broken.dragon_uuid = Some(Uuid::nil());
    let mut restored = state();
    restored.end_fight = broken;
    assert!(restore(&mut restored).unwrap_err().to_string().contains(
        "DragonFight.dragon_uuid 00000000-0000-0000-0000-000000000000: missing retained entity"
    ));
}
#[test]
fn death_restart_awards_exactly_once_activates_exit_egg_and_gateway() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    begin_death(&mut s, slot);
    s.end_fight.death_ticks = 149;
    let mut total = 0;
    for _ in 150..=175 {
        let mut changes = ChangeSet::default();
        death_tick(&mut s, &mut changes, slot);
        total += changes.entities_spawned.len();
    }
    assert!(total > 0);
    assert_eq!(xp(&s), 4800);
    let mut restarted = full_restore(&s);
    let slot = dragon_slot(&restarted).unwrap();
    for _ in 176..=200 {
        death_tick(&mut restarted, &mut ChangeSet::default(), slot);
    }
    assert_eq!(xp(&restarted), 12000);
    assert!(restarted.end_fight.dragon_killed);
    assert!(restarted.end_fight.previously_killed);
    assert_eq!(restarted.end_fight.gateways.len(), 19);
    let [x, y, z] = restarted.end_fight.exit_portal;
    assert_eq!(
        restarted
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(
                restarted.world.block_at(x, y + 4, z).unwrap()
            ))
            .unwrap()
            .block
            .id
            .path(),
        "dragon_egg"
    );
    assert!(
        restarted
            .entities
            .slots()
            .all(|slot| !is_dragon(&restarted, slot))
    );
}
#[test]
fn respawn_requires_four_crystals_and_survives_each_stage_restart() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    begin_death(&mut s, slot);
    s.end_fight.death_ticks = 199;
    death_tick(&mut s, &mut ChangeSet::default(), slot);
    let [x, y, z] = s.end_fight.exit_portal;
    for [dx, dz] in [[0, -2], [2, 0], [0, 2], [-2, 0]] {
        let position = [
            f64::from(x + dx) + 0.5,
            f64::from(y + 1),
            f64::from(z + dz) + 0.5,
        ];
        let id = s.spawn_mob(type_id(&s, "minecraft:end_crystal"), position, 1.0);
        let slot = s.entities.slot_of(id).unwrap();
        s.end_fight.crystals.push(Crystal {
            uuid: s.entities.row(slot).uuid,
            position,
            pillar: None,
            beam: None,
            invulnerable: false,
        });
        try_respawn(&mut s, &mut ChangeSet::default());
        if dx != -2 {
            assert!(s.end_fight.respawn.is_none());
        }
    }
    assert!(s.end_fight.respawn.is_some());
    for _ in 0..604 {
        respawn_effects(&mut s, &mut ChangeSet::default());
        respawn_tick(&mut s, &mut ChangeSet::default());
        if s.end_fight.respawn.as_ref().is_some_and(|r| r.time == 1) {
            let directory = tempfile::tempdir().unwrap();
            native_fixture(&s, directory.path());
            let core = crate::Core::open(config(directory.path())).unwrap();
            assert!(core.checkpoint());
            core.shutdown();
            let manifest =
                crate::native_checkpoint::tests::read_manifest(&config(directory.path())).unwrap();
            assert_eq!(
                manifest.dimensions[&Dimension::End]
                    .end_fight
                    .as_ref()
                    .unwrap()
                    .respawn,
                s.end_fight.respawn
            );
            s = full_restore(&s);
        }
    }
    assert!(s.end_fight.respawn.is_none());
    assert!(!s.end_fight.dragon_killed);
    assert_eq!(s.end_fight.crystals.len(), 10);
    assert_eq!(s.end_fight.gateways.len(), 19);
    assert_eq!(
        s.entities
            .slots()
            .filter(|slot| is_dragon(&s, *slot))
            .count(),
        1
    );
    let before = xp(&s);
    let slot = dragon_slot(&s).unwrap();
    begin_death(&mut s, slot);
    for _ in 0..200 {
        death_tick(&mut s, &mut ChangeSet::default(), slot);
    }
    assert_eq!(xp(&s) - before, 500);
    assert_eq!(s.end_fight.gateways.len(), 18);
}

fn xp(state: &TickState) -> i32 {
    state
        .entities
        .slots()
        .filter_map(|s| state.entities.row(s).appearance.experience_orb)
        .map(|(value, count)| value * count)
        .sum()
}
fn native_fixture(state: &TickState, root: &std::path::Path) {
    let config = config(root);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let mut image = store.read_checkpoint().unwrap();
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    let tick = state.tick.max(1);
    manifest.tick = tick;
    for child in manifest.dimensions.values_mut() {
        child.tick = tick;
    }
    manifest
        .dimensions
        .get_mut(&Dimension::End)
        .unwrap()
        .end_fight = Some(state.end_fight.clone());
    let (_, entities, _) = crate::core::checkpoint_data_for(state, tick);
    let entities = crate::native_checkpoint::encode_entities(
        Dimension::End,
        &entities,
        &state.registry.items,
        tick,
    )
    .unwrap();
    for global in image.globals.values_mut() {
        global.captured_tick = tick;
    }
    image.globals.insert(
        "checkpoint".into(),
        crate::native_checkpoint::record(tick, serde_json::to_vec(&manifest).unwrap()),
    );
    store
        .commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &entities,
                players: &[],
                globals: &image.globals,
            },
            &state.registry.blocks,
            &state.registry.items,
        )
        .unwrap();
}
fn config(root: &std::path::Path) -> crate::config::CoreConfig {
    let mut config = crate::config::CoreConfig::new(root.to_owned(), 42);
    config.view_distance = 1;
    config.simulation_distance = 0;
    config.checkpoint_every_ticks = u64::MAX;
    config.journal_fsync_every_batches = 1;
    config
}
#[test]
fn peaceful_end_checkpoint_reopens_with_the_same_dragon() {
    let mut s = running();
    let uuid = s.end_fight.dragon_uuid.unwrap();
    s.difficulty = crate::survival::Difficulty::Peaceful;
    run_tick(&mut s, &[]);
    let directory = tempfile::tempdir().unwrap();
    native_fixture(&s, directory.path());
    let core = crate::Core::open(config(directory.path())).unwrap();
    core.shutdown();
    assert_eq!(s.end_fight.dragon_uuid, Some(uuid));
    assert!(s.entities.slot_of_uuid(uuid).is_some());
    let core = crate::Core::open(config(directory.path())).unwrap();
    core.shutdown();
}

#[test]
fn native_metadata_uses_measured_26_1_keys_and_rejects_malformed_phase_beam() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    s.end_fight.phase = Phase::Strafing;
    s.end_fight.death_ticks = 0;
    let fields = entity_fields(&s, slot);
    assert!(fields.contains(&("DragonPhase".into(), mc_nbt::Tag::Int(1))));
    let mut record = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:ender_dragon".into(),
        extra_fields: vec![("DragonPhase".into(), mc_nbt::Tag::Int(99))],
        ..Default::default()
    };
    assert!(
        native_appearance(&record, &mut Default::default())
            .unwrap_err()
            .contains("DragonPhase")
    );
    record.id = "minecraft:end_crystal".into();
    record.extra_fields = vec![("beam_target".into(), mc_nbt::Tag::IntArray(vec![1, 2]))];
    assert!(
        native_appearance(&record, &mut Default::default())
            .unwrap_err()
            .contains("beam_target")
    );
    let mut nbt = persistence::to_nbt(&s.end_fight);
    let mc_nbt::Tag::Compound(root) = &mut nbt else {
        unreachable!()
    };
    let mc_nbt::Tag::Compound(data) = &mut root.iter_mut().find(|(k, _)| k == "data").unwrap().1
    else {
        unreachable!()
    };
    *data
        .iter_mut()
        .find(|(k, _)| k == "needs_state_scanning")
        .unwrap() = ("needs_state_scanning".into(), mc_nbt::Tag::Byte(1));
    assert!(
        persistence::from_nbt(&nbt)
            .unwrap_err()
            .contains("native fields disagree")
    );
}
#[test]
fn interrupted_wal_death_award_reopens_native_checkpoint_without_duplicate_xp() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    begin_death(&mut s, slot);
    s.end_fight.death_ticks = 149;
    let directory = tempfile::tempdir().unwrap();
    native_fixture(&s, directory.path());
    let core = crate::Core::open(config(directory.path())).unwrap();
    assert!(core.checkpoint());
    core.shutdown();
    let base = crate::native_checkpoint::tests::read_manifest(&config(directory.path()))
        .unwrap()
        .tick;
    s.tick = base;
    let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
    for _ in 0..6 {
        let (out, _, _) = run_tick(&mut s, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut s, &out);
        writer
            .append(
                out.tick,
                out.tick,
                &[crate::journal::JournalEntry::Dimension {
                    dimension: Dimension::End,
                    entries,
                }],
            )
            .unwrap();
    }
    writer.flush().unwrap();
    drop(writer);
    assert_eq!(s.end_fight.death_ticks, 155);
    assert_eq!(xp(&s), 960);
    // A process interruption leaves an unpublished temporary batch, never a
    // valid post-image. Recovery must retain the six complete atomic batches.
    std::fs::write(
        directory.path().join("journal/interrupted.tmp"),
        b"{partial",
    )
    .unwrap();
    for _ in 0..2 {
        let core = crate::Core::open(config(directory.path())).unwrap();
        let mut receiver = core.subscribe();
        let out = receiver.blocking_recv().unwrap();
        let end = out.in_dimension(Dimension::End).unwrap();
        let actual: i32 = end
            .snapshot
            .entities
            .iter()
            .filter_map(|e| e.appearance.experience_orb)
            .map(|(v, n)| v * n)
            .sum();
        assert_eq!(actual, 960);
        assert_eq!(
            end.snapshot
                .entities
                .iter()
                .filter(|e| dragon_type(e.type_id))
                .count(),
            1
        );
        assert!(core.checkpoint());
        core.shutdown();
        let manifest =
            crate::native_checkpoint::tests::read_manifest(&config(directory.path())).unwrap();
        assert_eq!(
            manifest.dimensions[&Dimension::End]
                .end_fight
                .as_ref()
                .unwrap()
                .death_ticks,
            155
        );
    }
}

#[test]
fn restored_ritual_crystal_loss_aborts_and_reactivates_exit_portal() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    begin_death(&mut s, slot);
    s.end_fight.death_ticks = 199;
    death_tick(&mut s, &mut ChangeSet::default(), slot);
    let [x, y, z] = s.end_fight.exit_portal;
    let mut ids = [Uuid::nil(); 4];
    for (index, [dx, dz]) in [[0, -2], [2, 0], [0, 2], [-2, 0]].into_iter().enumerate() {
        let id = s.spawn_mob(
            type_id(&s, "minecraft:end_crystal"),
            [
                f64::from(x + dx) + 0.5,
                f64::from(y + 1),
                f64::from(z + dz) + 0.5,
            ],
            1.0,
        );
        let slot = s.entities.slot_of(id).unwrap();
        ids[index] = s.entities.row(slot).uuid;
    }
    sync_crystals(&mut s);
    try_respawn(&mut s, &mut ChangeSet::default());
    assert!(s.end_fight.respawn.is_some());
    let slot = s.entities.slot_of_uuid(ids[0]).unwrap();
    s.entities.remove(slot);
    commit(&mut s, &mut ChangeSet::default());
    assert!(s.end_fight.respawn.is_none());
    assert!(s.end_fight.dragon_killed);
    assert!(
        s.end_fight
            .crystals
            .iter()
            .all(|c| c.beam.is_none() && !c.invulnerable)
    );
    let portal = s.world.block_at(x + 1, y, z).unwrap();
    assert_eq!(
        s.registry
            .blocks
            .by_id(mc_world::BlockStateId(portal))
            .unwrap()
            .block
            .id
            .path(),
        "end_portal"
    );
}
#[test]
fn perched_projectiles_are_immune_and_only_committed_damage_triggers_takeoff() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    s.end_fight.phase = Phase::PerchingFlaming;
    let mut piece = crate::tick::DamagePiece {
        physical: None,
        custom_armor: false,
        amount: 20.0,
        kind: crate::survival::DamageKind::Projectile,
        source: None,
    };
    assert!(!damage_allowed(&mut s, slot, &mut piece));
    assert_eq!(s.end_fight.perched_damage, 0.0);
    piece.kind = crate::survival::DamageKind::Melee;
    assert!(damage_allowed(&mut s, slot, &mut piece));
    assert_eq!(s.end_fight.perched_damage, 0.0);
    damage_committed(&mut s, slot, 50.0);
    assert_eq!(s.end_fight.phase, Phase::PerchingFlaming);
    damage_committed(&mut s, slot, 1.0);
    assert_eq!(s.end_fight.phase, Phase::Takeoff);
}

#[test]
fn strafe_emits_owned_fireball_and_perch_cycle_emits_retained_dragon_cloud() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    let player = s.connections.entity_of(ConnectionId(1)).unwrap();
    let player_slot = s.entities.slot_of(player).unwrap();
    s.entities.set_pos(slot, [0.0, 128.0, 0.0]);
    s.entities.set_rot(slot, 0.0, 0.0);
    s.entities.set_pos(player_slot, [0.0, 128.0, -40.0]);
    s.end_fight.target = Some(s.entities.row(player_slot).uuid);
    s.end_fight.phase = Phase::Strafing;
    s.end_fight.phase_ticks = 0;
    s.end_fight.fly_target = [0.0, 128.0, -40.0];
    for _ in 0..4 {
        flight::run(&mut s, &mut Vec::new(), slot);
    }
    assert_eq!(s.end_fight.fireball_charge, 4);
    assert!(s.entities.slots().all(|slot| {
        s.entities
            .row(slot)
            .projectile
            .as_ref()
            .is_none_or(|p| p.kind != crate::projectiles::ProjectileKind::DragonFireball)
    }));
    flight::run(&mut s, &mut Vec::new(), slot);
    assert!(
        s.entities.slots().any(
            |slot| s
                .entities
                .row(slot)
                .projectile
                .as_ref()
                .is_some_and(
                    |p| p.kind == crate::projectiles::ProjectileKind::DragonFireball
                        && p.owner == s.end_fight.dragon_uuid
                )
        )
    );
    let [x, y, z] = s.end_fight.exit_portal;
    s.entities
        .set_pos(slot, [f64::from(x), f64::from(y + 4), f64::from(z)]);
    s.entities.set_pos(
        player_slot,
        [f64::from(x), f64::from(y + 4), f64::from(z - 8)],
    );
    s.end_fight.phase = Phase::PerchingScanning;
    s.end_fight.phase_ticks = 0;
    for _ in 0..75 {
        flight::run(&mut s, &mut Vec::new(), slot);
    }
    assert_eq!(s.end_fight.phase, Phase::PerchingFlaming);
    let cloud = s
        .entities
        .slots()
        .filter_map(|slot| s.entities.row(slot).projectile.as_ref())
        .find(|p| p.kind == crate::projectiles::ProjectileKind::AreaEffectCloud)
        .unwrap();
    assert_eq!(cloud.owner, s.end_fight.dragon_uuid);
    let crate::projectiles::ProjectileData::Cloud(payload) = &cloud.data else {
        unreachable!()
    };
    assert_eq!(payload.radius, 5.0);
    assert_eq!(payload.duration, 200);
    let restarted = full_restore(&s);
    assert!(restarted.entities.slots().any(|slot| {
        restarted
            .entities
            .row(slot)
            .projectile
            .as_ref()
            .is_some_and(|p| {
                p.kind == crate::projectiles::ProjectileKind::AreaEffectCloud
                    && p.owner == restarted.end_fight.dragon_uuid
            })
    }));
    let cloud_uuid = s.end_fight.flame_cloud.unwrap();
    s.end_fight.phase = Phase::Takeoff;
    let mut changes = ChangeSet::default();
    commit(&mut s, &mut changes);
    assert!(s.entities.slot_of_uuid(cloud_uuid).is_none());
    assert!(s.end_fight.flame_cloud.is_none());
    assert!(!changes.entities_removed.is_empty());
}

#[test]
fn healing_crystal_restores_one_health_and_its_loss_deals_ten() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    let crystal = s.end_fight.crystals[0].clone();
    let mut at = crystal.position;
    at[1] += 5.0;
    s.entities.set_pos(slot, at);
    s.entities.row_mut(slot).health = 190.0;
    s.end_fight.phase = Phase::Hover;
    s.end_fight.fly_target = at;
    s.tick = 10;
    flight::run(&mut s, &mut Vec::new(), slot);
    assert_eq!(s.entities.row(slot).health, 191.0);
    assert_eq!(s.end_fight.healing_crystal, Some(crystal.uuid));
    let crystal_slot = s.entities.slot_of_uuid(crystal.uuid).unwrap();
    s.entities.remove(crystal_slot);
    sync_crystals(&mut s);
    assert_eq!(s.entities.row(dragon_slot(&s).unwrap()).health, 181.0);
    assert!(s.end_fight.healing_crystal.is_none());
}
#[test]
fn crystal_hit_is_lethal_and_chained_explosion_does_not_emit_second_blast() {
    let mut s = running();
    let first = s.spawn_mob(type_id(&s, "minecraft:end_crystal"), [0.5, 150.0, 0.5], 1.0);
    let second = s.spawn_mob(type_id(&s, "minecraft:end_crystal"), [2.5, 150.0, 0.5], 1.0);
    let slot = s.entities.slot_of(first).unwrap();
    s.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 1.0,
            kind: crate::survival::DamageKind::Melee,
            source: None,
        },
    ));
    crystal_hits(&mut s);
    assert_eq!(s.hostile.explosions.len(), 1);
    assert_eq!(s.hostile.explosions[0].radius, 6.0);
    assert!(
        s.merge_damage
            .iter()
            .any(|(slot, piece)| s.entities.body(*slot).id == second && piece.amount >= 1000.0)
    );
    let (deaths, _, _, _) = s.commit_merge_effects();
    assert!(deaths.contains(&first));
    assert!(deaths.contains(&second));
}

#[test]
fn landing_approach_reaches_exit_fountain_and_enters_perch_without_teleport() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    s.end_fight.phase = Phase::LandingApproach;
    s.end_fight.phase_ticks = 0;
    s.end_fight.fly_target = [40.0, 85.0, 0.0];
    for _ in 0..3000 {
        flight::run(&mut s, &mut Vec::new(), slot);
        if s.end_fight.phase == Phase::PerchingScanning {
            break;
        }
    }
    assert_eq!(s.end_fight.phase, Phase::PerchingScanning);
    let [x, y, z] = s.end_fight.exit_portal;
    let at = s.entities.pos(slot);
    assert!(
        (at[0] - (f64::from(x) + 0.5)).powi(2)
            + (at[1] - f64::from(y + 4)).powi(2)
            + (at[2] - (f64::from(z) + 0.5)).powi(2)
            < 1.0
    );
}

#[test]
fn swept_arrows_hit_derived_head_and_body_boxes_through_tick_damage_pipeline() {
    let mut s = running();
    let slot = dragon_slot(&s).unwrap();
    s.entities.set_pos(slot, [0.0, 150.0, 0.0]);
    s.entities.set_rot(slot, 0.0, 0.0);
    s.entities.row_mut(slot).controls.no_ai = true;
    s.end_fight.flight_history = [[150.0, 0.0]; 17];
    s.end_fight.yaw_accel = 0.0;
    let player = s.connections.entity_of(ConnectionId(1)).unwrap();
    let owner = s.entities.row(s.entities.slot_of(player).unwrap()).uuid;
    let arrow = crate::projectiles::ProjectileState::new(
        crate::projectiles::ProjectileKind::Arrow,
        Some(owner),
        mc_data::ItemStack::EMPTY,
    );
    crate::projectiles::launch(&mut s, arrow.clone(), [0.0, 150.5, -10.0], [0.0, 0.0, 5.0])
        .unwrap();
    run_tick(&mut s, &[]);
    let slot = dragon_slot(&s).unwrap();
    assert_eq!(s.entities.row(slot).health, 190.0);
    s.tick += 11;
    crate::projectiles::launch(&mut s, arrow, [0.0, 150.5, 5.0], [0.0, 0.0, -7.0]).unwrap();
    run_tick(&mut s, &[]);
    let slot = dragon_slot(&s).unwrap();
    assert_eq!(s.entities.row(slot).health, 185.5);
}

#[test]
fn native_gateway_int_array_import_preserves_order_and_rejects_invalid_indices() {
    use mc_nbt::Tag;
    let root = |gateways| {
        Tag::Compound(vec![(
            "data".into(),
            Tag::Compound(vec![("gateways".into(), gateways)]),
        )])
    };
    let order = vec![
        8, 11, 10, 13, 15, 14, 3, 9, 0, 12, 6, 2, 18, 7, 19, 4, 5, 1, 16, 17,
    ];
    let fight = from_nbt(&root(Tag::IntArray(order.clone()))).unwrap();
    assert_eq!(
        fight.gateways,
        order.iter().map(|i| *i as u8).collect::<Vec<_>>()
    );
    // Solaris's existing native writer emits an Int list, also accepted by the
    // vanilla Codec.list(INT) reader. Keep its round-trip alongside IntArray.
    let encoded = to_nbt(&fight);
    assert_eq!(from_nbt(&encoded).unwrap().gateways, fight.gateways);
    for values in [vec![-1], vec![20], vec![i32::MAX], vec![1, 1]] {
        assert!(from_nbt(&root(Tag::IntArray(values))).is_err());
    }
    assert!(from_nbt(&root(Tag::LongArray(vec![1]))).is_err());
}
