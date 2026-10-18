use super::{flat_player_fixture, run_tick};
use crate::boundary::EntityId;
use crate::enderman;
use crate::entities::EntitySlot;
use crate::special_mobs::Retained;
use crate::survival::DamageKind;
use crate::tick::{DamagePiece, TickState};
use mc_data::{Identifier, ItemStack};

fn fixture() -> (TickState, EntityId, EntityId, EntitySlot, [f64; 3]) {
    let (mut state, player, mut pos) = flat_player_fixture();
    pos[1] = 201.0;
    for x in pos[0] as i32 - 31..=pos[0] as i32 + 31 {
        for z in pos[2] as i32 - 31..=pos[2] as i32 + 31 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..=235 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    state.world.fold_overlay();
    let actor = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(actor, pos);
    state.entities.set_rot(actor, 90.0, 0.0);
    let kind = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:enderman").unwrap())
        .unwrap();
    let mob = state.spawn_mob(kind, [pos[0], pos[1], pos[2] + 4.0], 40.0);
    let slot = state.entities.slot_of(mob).unwrap();
    state.entities.row_mut(slot).speed = 0.0;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    (state, player, mob, slot, pos)
}

fn stare(state: &mut TickState, player: EntityId, slot: EntitySlot) {
    let actor = state.entities.slot_of(player).unwrap();
    let mob = state.entities.pos(slot);
    let pos = state.entities.pos(actor);
    let dy = mob[1] + state.entities.body(slot).height * 0.9 - pos[1] - 1.62;
    let pitch = -(dy / (mob[2] - pos[2])).atan().to_degrees() as f32;
    state.entities.set_rot(actor, 0.0, pitch);
}

#[test]
fn enderman_projectile_is_immune_and_teleports_to_dry_solid_ground() {
    let (mut state, _, mob, slot, _) = fixture();
    let old = state.entities.pos(slot);
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 20.0,
            kind: DamageKind::Projectile,
            source: None,
        },
    ));
    let (output, _, _) = run_tick(&mut state, &[]);
    assert_eq!(state.entities.row(slot).health, 40.0);
    let new = state.entities.pos(slot);
    assert_ne!(new, old);
    assert!(
        new.iter()
            .zip(old)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            <= 1024.0
    );
    assert!(!crate::conduit::water(&state, new));
    assert_eq!(
        state
            .world
            .block_at(new[0] as i32, new[1] as i32 - 1, new[2] as i32),
        Some(state.registry.states.stone)
    );
    assert!(output.changes.entities_appearance_changed.contains(&mob));
}

#[test]
fn enderman_arrow_deflects_without_damage_or_consuming_ammunition() {
    let (mut state, player, _, slot, pos) = fixture();
    let owner = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let arrow = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:arrow").unwrap())
        .unwrap();
    let record = crate::projectiles::ProjectileState::new(
        crate::projectiles::ProjectileKind::Arrow,
        Some(owner),
        ItemStack::new(arrow, 1),
    );
    let projectile = crate::projectiles::launch(
        &mut state,
        record,
        [pos[0], pos[1] + 1.0, pos[2] + 1.0],
        [0.0, 0.0, 4.0],
    )
    .unwrap();
    let old = state.entities.pos(slot);
    run_tick(&mut state, &[]);
    assert_eq!(state.entities.row(slot).health, 40.0);
    assert_ne!(state.entities.pos(slot), old);
    let arrow_slot = state.entities.slot_of(projectile).unwrap();
    assert!(state.entities.vel(arrow_slot)[2] < 0.0);
    assert_eq!(
        state
            .entities
            .row(arrow_slot)
            .projectile
            .as_ref()
            .unwrap()
            .stack
            .count,
        1
    );
}

#[test]
fn enderman_water_and_exposed_rain_damage_and_teleport_but_roof_is_dry() {
    for rain in [false, true] {
        let (mut state, _, mob, slot, _) = fixture();
        let old = state.entities.pos(slot);
        if rain {
            state.weather = (crate::commands::Weather::Rain, 1000);
        } else {
            state.world.set_block(
                old[0] as i32,
                old[1] as i32,
                old[2] as i32,
                state.registry.states.water,
            );
        }
        let (output, _, _) = run_tick(&mut state, &[]);
        assert_eq!(state.entities.row(slot).health, 39.0);
        assert_ne!(state.entities.pos(slot), old);
        assert!(output.changes.entities_appearance_changed.contains(&mob));
    }
    let (mut state, _, _, slot, _) = fixture();
    let old = state.entities.pos(slot);
    state.weather = (crate::commands::Weather::Rain, 1000);
    state.world.set_block(
        old[0] as i32,
        205,
        old[2] as i32,
        state.registry.states.stone,
    );
    run_tick(&mut state, &[]);
    assert_eq!(state.entities.row(slot).health, 40.0);
    assert_eq!(state.entities.pos(slot), old);
}

#[test]
fn enderman_stare_aggro_pumpkin_range_and_occlusion() {
    for (pumpkin, blocked, far, expected) in [
        (false, false, false, true),
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
    ] {
        let (mut state, player, mob, slot, pos) = fixture();
        if far {
            state
                .entities
                .set_pos(slot, [pos[0], pos[1], pos[2] + 65.0]);
        }
        if pumpkin {
            let pumpkin = state
                .registry
                .items
                .id_of(&Identifier::parse("minecraft:carved_pumpkin").unwrap())
                .unwrap();
            state.players.get_mut(&player).unwrap().inventory.slots[39] =
                ItemStack::new(pumpkin, 1);
        }
        if blocked {
            for y in 201..205 {
                state.world.set_block(
                    pos[0] as i32,
                    y,
                    pos[2] as i32 + 2,
                    state.registry.states.stone,
                );
            }
        }
        stare(&mut state, player, slot);
        let (output, _, _) = run_tick(&mut state, &[]);
        assert_eq!(enderman::target(&state, slot).is_some(), expected);
        if expected {
            assert!(output.changes.entities_appearance_changed.contains(&mob));
        }
    }
}

fn checkpoint(state: &TickState, store: &mc_world::native::NativeStore) {
    use crate::dimensions::Dimension;
    use crate::native_checkpoint::{encode_entities, record, refresh_metadata};
    use mc_world::native::NativeCheckpoint;
    let tick = state.tick;
    let (_, captured, _) = crate::core::checkpoint_data_for(state, tick);
    let entities =
        encode_entities(Dimension::Overworld, &captured, &state.registry.items, tick).unwrap();
    let mut image = store.read_checkpoint().unwrap();
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.tick = tick;
    for child in manifest.dimensions.values_mut() {
        child.tick = tick;
    }
    image.globals.insert(
        "checkpoint".into(),
        record(tick, serde_json::to_vec(&manifest).unwrap()),
    );
    refresh_metadata(&mut image.globals, &manifest).unwrap();
    for global in image.globals.values_mut() {
        global.captured_tick = tick;
    }
    store
        .commit_checkpoint(
            NativeCheckpoint {
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

#[test]
fn enderman_anger_and_teleport_survive_wal_native_restart_and_anvil_export() {
    use crate::dimensions::Dimension;
    let (mut state, player, mob, slot, _) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(directory.path().to_path_buf(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let uuid = state.entities.row(slot).uuid;
    let tick = state.tick;
    checkpoint(&state, &store);
    state.appearance_changed.clear();
    stare(&mut state, player, slot);
    let (output, _, _) = run_tick(&mut state, &[]);
    let expected = state.entities.row(slot).special_mob.as_deref().cloned();
    assert!(matches!(expected, Some(Retained::Enderman { .. })));
    let mut writer = crate::journal::JournalWriter::open(directory.path(), 1).unwrap();
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    writer.append(output.tick, output.tick, &entries).unwrap();
    let old = state.entities.pos(slot);
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 20.0,
            kind: DamageKind::Projectile,
            source: None,
        },
    ));
    let (output, _, _) = run_tick(&mut state, &[]);
    let new = state.entities.pos(slot);
    assert_ne!(new, old);
    assert_eq!(state.entities.row(slot).health, 40.0);
    assert!(output.changes.entities_appearance_changed.contains(&mob));
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    drop(store);
    let restored = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let slot = restored.entities.slot_of_uuid(uuid).unwrap();
    assert_eq!(
        restored.entities.row(slot).special_mob.as_deref(),
        expected.as_ref()
    );
    assert_eq!(restored.entities.pos(slot), new);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    checkpoint(&restored, &store);
    drop(store);
    drop(restored);
    let restored = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let slot = restored.entities.slot_of_uuid(uuid).unwrap();
    assert_eq!(
        restored.entities.row(slot).special_mob.as_deref(),
        expected.as_ref()
    );
    assert_eq!(restored.entities.pos(slot), new);
    let (_, exported, _) = crate::core::checkpoint_data_for(&restored, tick);
    let path = directory.path().join("anvil");
    mc_world::anvil::entity_regions::write_entity_regions(
        &path,
        &restored.registry.items,
        &exported,
        tick.try_into().unwrap(),
    )
    .unwrap();
    let reread =
        mc_world::anvil::entity_regions::read_entity_regions(&path, &restored.registry.items)
            .unwrap();
    let record = reread
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|record| record.uuid == uuid)
        .unwrap();
    assert_eq!(
        crate::special_mobs::persistence::from_record(record).unwrap(),
        expected
    );
}

#[test]
fn slime_split_children_are_journaled_and_export_size_name_and_persistence() {
    for name in ["slime", "magma_cube"] {
        for size in [1, 2, 4] {
            let (mut state, _, _, _, pos) = fixture();
            let type_id = state
                .registry
                .entity_types
                .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
                .unwrap();
            let parent = state.spawn_mob(type_id, pos, 1.0);
            let slot = state.entities.slot_of(parent).unwrap();
            crate::entity_loot::set_size(&mut state, slot, size, true);
            state.entities.row_mut(slot).persistent = true;
            std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name =
                Some("split family".into());
            state.merge_damage.push((
                slot,
                DamagePiece {
                    physical: None,
                    custom_armor: false,
                    amount: 1000.0,
                    kind: DamageKind::Void,
                    source: None,
                },
            ));
            let (output, _, _) = run_tick(&mut state, &[]);
            let children: Vec<_> = output.changes.entities_spawned.iter().copied().filter(|id| {
                state.entities.slot_of(*id).is_some_and(|slot| matches!(state.entities.body(slot).kind, crate::entities::EntityKind::Mob { type_id: kind } if kind == type_id))
            }).collect();
            if size == 1 {
                assert!(children.is_empty());
                continue;
            }
            assert!((2..=4).contains(&children.len()));
            let entries = crate::core::journal_entries_for_tick(&mut state, &output);
            let replay = crate::journal::replay_batches(
                crate::journal::Replay::default(),
                &[(0, output.tick, entries)],
            );
            assert!(!replay.entities.contains_key(&parent.0));
            for child in &children {
                let image = &replay.entities[&child.0];
                assert_eq!(image.state.loot.slime_size, Some(size / 2));
                assert!(image.state.persistent);
                assert_eq!(
                    image.appearance.custom_name.as_deref(),
                    Some("split family")
                );
            }
            let (_, captured, _) = crate::core::checkpoint_data_for(&state, output.tick);
            let dir = tempfile::tempdir().unwrap();
            let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
            let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
            checkpoint(&state, &store);
            drop(store);
            let restored = crate::core::recover_dimension_state(
                &config,
                crate::dimensions::Dimension::Overworld,
            )
            .unwrap();
            for child in &children {
                let uuid = state
                    .entities
                    .row(state.entities.slot_of(*child).unwrap())
                    .uuid;
                let row = restored
                    .entities
                    .row(restored.entities.slot_of_uuid(uuid).unwrap());
                assert_eq!(row.loot.slime_size, Some(size / 2));
                assert!(row.persistent);
                assert_eq!(row.appearance.custom_name.as_deref(), Some("split family"));
            }
            mc_world::anvil::entity_regions::write_entity_regions(
                dir.path(),
                &state.registry.items,
                &captured,
                output.tick.try_into().unwrap(),
            )
            .unwrap();
            let recovered = mc_world::anvil::entity_regions::read_entity_regions(
                dir.path(),
                &state.registry.items,
            )
            .unwrap();
            let records: Vec<_> = recovered
                .values()
                .flat_map(|chunk| &chunk.entities)
                .filter(|entity| entity.id == format!("minecraft:{name}"))
                .collect();
            assert_eq!(records.len(), children.len());
            for record in records {
                let fields =
                    mc_data::entity_nbt::entity_to_fields(&state.registry.items, record).unwrap();
                assert!(fields.contains(&(
                    "CustomName".into(),
                    mc_nbt::Tag::String("split family".into())
                )));
                assert!(
                    fields.contains(&("Size".into(), mc_nbt::Tag::Int(i32::from(size / 2) - 1)))
                );
                assert!(fields.contains(&("PersistenceRequired".into(), mc_nbt::Tag::Byte(1))));
            }
        }
    }
}

#[test]
fn enderman_is_neutral_until_stared_at_then_commits_melee_attacks() {
    for provoke in [false, true] {
        let (mut state, player, _, slot, pos) = fixture();
        state.entities.set_pos(slot, [pos[0], pos[1], pos[2] + 1.0]);
        let actor = state.entities.slot_of(player).unwrap();
        state.entities.row_mut(actor).protected_until = 0;
        if provoke {
            stare(&mut state, player, slot);
        }
        run_tick(&mut state, &[]);
        assert_eq!(state.entities.row(actor).health < 20.0, provoke);
    }
}
