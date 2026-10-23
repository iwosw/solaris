use super::{flat_player_fixture, run_tick};
use crate::boundary::{EntityId, PlayerChange};
use crate::effects::{ActiveEffect, EffectId};
use crate::entities::EntityKind;
use crate::survival::DamageKind;
use crate::tick::{DamagePiece, TickState};
use mc_data::Identifier;

fn mob(state: &mut TickState, name: &str, pos: [f64; 3]) -> EntityId {
    let kind = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    state.spawn_mob(kind, pos, 100.0)
}
fn effect(state: &mut TickState, id: EntityId, effect: EffectId) {
    let slot = state.entities.slot_of(id).unwrap();
    state
        .entities
        .row_mut(slot)
        .active_effects
        .add(ActiveEffect::new(effect, 3600, 0));
}
fn names(state: &TickState, name: &str) -> Vec<EntityId> {
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            match body.kind {
                EntityKind::Mob { type_id }
                    if state
                        .registry
                        .blueprint(type_id)
                        .is_some_and(|bp| bp.name == name) =>
                {
                    Some(body.id)
                }
                _ => None,
            }
        })
        .collect()
}
fn hit(state: &mut TickState, id: EntityId, amount: f32) {
    let slot = state.entities.slot_of(id).unwrap();
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount,
            kind: DamageKind::Generic,
            source: None,
        },
    ));
}

#[test]
fn effect_trigger_oozing_births_two_medium_slimes_once_and_survives_wal_and_checkpoint() {
    let (mut state, _, pos) = flat_player_fixture();
    let victim = mob(&mut state, "cow", [pos[0] + 2.0, pos[1], pos[2]]);
    effect(&mut state, victim, EffectId::Oozing);
    hit(&mut state, victim, 1000.0);
    let (output, _, _) = run_tick(&mut state, &[]);
    let slimes = names(&state, "minecraft:slime");
    assert_eq!(slimes.len(), 2);
    for id in &slimes {
        assert_eq!(
            state
                .entities
                .row(state.entities.slot_of(*id).unwrap())
                .loot
                .slime_size,
            Some(2)
        );
    }
    assert!(state.entities.slot_of_any(victim).is_none());
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let replay = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &[(0, output.tick, entries.clone()), (0, output.tick, entries)],
    );
    let slime_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:slime").unwrap())
        .unwrap();
    assert_eq!(
        replay
            .entities
            .values()
            .filter(|e| e.type_id == slime_type)
            .count(),
        2
    );
    let (_, entities, _) = crate::core::checkpoint_data_for(&state, output.tick);
    let dir = std::env::temp_dir().join(format!("solaris-oozing-{}", uuid::Uuid::new_v4()));
    mc_world::anvil::entity_regions::write_entity_regions(
        &dir,
        &state.registry.items,
        &entities,
        u32::try_from(output.tick).unwrap(),
    )
    .unwrap();
    let recovered =
        mc_world::anvil::entity_regions::read_entity_regions(&dir, &state.registry.items).unwrap();
    let records: Vec<_> = recovered
        .values()
        .flat_map(|chunk| &chunk.entities)
        .filter(|r| r.id == "minecraft:slime")
        .collect();
    assert_eq!(records.len(), 2);
    for record in records {
        let fields = mc_data::entity_nbt::entity_to_fields(&state.registry.items, record).unwrap();
        assert!(fields.contains(&("Size".into(), mc_nbt::Tag::Int(1))));
    }
    std::fs::remove_dir_all(dir).unwrap();
    run_tick(&mut state, &[]);
    assert_eq!(names(&state, "minecraft:slime").len(), 2);
}

#[test]
fn effect_trigger_infested_admitted_hit_spawns_one_or_two_silverfish_but_refusals_never_proc() {
    let (mut state, _, pos) = flat_player_fixture();
    let victim = mob(&mut state, "cow", [pos[0] + 2.0, pos[1], pos[2]]);
    let slot = state.entities.slot_of(victim).unwrap();
    state.entities.row_mut(slot).uuid = uuid::Uuid::from_u128(1);
    effect(&mut state, victim, EffectId::Infested);
    for tick in 100..300 {
        state.tick = tick;
        let slot = state.entities.slot_of(victim).unwrap();
        state.entities.row_mut(slot).protected_until = tick + 1;
        hit(&mut state, victim, 1.0);
        state.commit_merge_effects();
    }
    assert!(names(&state, "minecraft:silverfish").is_empty());
    for tick in 300..500 {
        state.tick = tick;
        let slot = state.entities.slot_of(victim).unwrap();
        let (body, row) = state.entities.parts_mut(slot);
        row.protected_until = 0;
        body.hurt_at = None;
        hit(&mut state, victim, 0.1);
        let (_, _, spawned, _) = state.commit_merge_effects();
        let silverfish = names(&state, "minecraft:silverfish");
        if !silverfish.is_empty() {
            assert!((1..=2).contains(&silverfish.len()));
            assert!(silverfish.iter().all(|id| spawned.contains(id)));
            return;
        }
    }
    panic!("seeded Infested hit sequence did not proc");
}

#[test]
fn effect_trigger_weaving_places_supported_cobwebs_journals_them_and_honors_mob_griefing() {
    for enabled in [false, true] {
        let (mut state, _, pos) = flat_player_fixture();
        state.tick = 100;
        state
            .gamerules
            .insert("mob_griefing".into(), enabled.to_string());
        let victim = mob(&mut state, "cow", [pos[0] + 2.0, pos[1], pos[2]]);
        let slot = state.entities.slot_of(victim).unwrap();
        state.entities.row_mut(slot).uuid = uuid::Uuid::from_u128(1);
        effect(&mut state, victim, EffectId::Weaving);
        hit(&mut state, victim, 1000.0);
        let (output, _, _) = run_tick(&mut state, &[]);
        let web = state
            .registry
            .blocks
            .block(&Identifier::parse("minecraft:cobweb").unwrap())
            .unwrap()
            .default
            .0;
        let writes: Vec<_> = output
            .changes
            .blocks
            .iter()
            .filter(|b| b.state == web)
            .collect();
        if enabled {
            assert!((1..=3).contains(&writes.len()), "{} webs", writes.len());
            for write in &writes {
                assert_eq!(
                    state
                        .world
                        .block_at(write.position.x, write.position.y, write.position.z),
                    Some(web)
                );
            }
            let entries = crate::core::journal_entries_for_tick(&mut state, &output);
            let replay = crate::journal::replay_batches(
                crate::journal::Replay::default(),
                &[(0, output.tick, entries)],
            );
            for write in writes {
                assert_eq!(
                    replay
                        .blocks
                        .get(&(write.position.x, write.position.y, write.position.z))
                        .copied(),
                    Some(web)
                );
            }
        } else {
            assert!(writes.is_empty());
        }
    }
}

#[test]
fn effect_trigger_wind_charged_death_emits_non_damaging_burst_and_reliable_player_motion() {
    let (mut state, player, pos) = flat_player_fixture();
    let victim = mob(&mut state, "cow", [pos[0] + 1.0, pos[1], pos[2]]);
    effect(&mut state, victim, EffectId::WindCharged);
    let health = state.players[&player].vitals.health;
    hit(&mut state, victim, 1000.0);
    let (output, reliable, _) = run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].vitals.health, health);
    assert_eq!(output.changes.explosions.len(), 1);
    assert!((3.0..5.0).contains(&output.changes.explosions[0].radius));
    assert_eq!(output.changes.explosions[0].blocks, 0);
    assert!(
        reliable.iter().flat_map(|(_, changes)| changes).any(
            |change| matches!(change, PlayerChange::Motion { entity, .. } if *entity == player)
        )
    );
    assert!(
        state
            .entities
            .vel(state.entities.slot_of(player).unwrap())
            .iter()
            .any(|v| *v != 0.0)
    );
}

#[test]
fn effect_trigger_bogged_arrow_has_poison_payload_and_applies_it_only_after_hit_admission() {
    let (mut state, player, pos) = flat_player_fixture();
    let bogged = mob(&mut state, "bogged", [pos[0] + 3.0, pos[1], pos[2]]);
    let arrow =
        crate::hostile::launch_arrow(&mut state, bogged, pos, [0.0, 0.0, 1.0], 2.0).unwrap();
    let record = state
        .entities
        .row(state.entities.slot_of(arrow).unwrap())
        .projectile
        .clone()
        .unwrap();
    assert_eq!(
        record.stack.extended().potion_custom_effects[0].duration,
        100
    );
    crate::projectiles::payloads::arrow_effects(
        &mut state,
        &mut Vec::new(),
        arrow,
        &record,
        player,
    );
    assert!(!state.players[&player].active_effects.has(EffectId::Poison));
    let source = crate::intent::DamageSource::Projectile {
        projectile: arrow,
        direct_type_id: state
            .registry
            .entity_types
            .id_of(&Identifier::parse("minecraft:arrow").unwrap())
            .unwrap(),
        owner: bogged,
        velocity: [0.0, 1.0],
        punch: 0,
        flame: false,
    };
    crate::projectiles::payloads::accepted_hit(&mut state, player, source, &mut 5.0);
    assert_eq!(
        state.players[&player]
            .active_effects
            .get(EffectId::Poison)
            .unwrap()
            .duration,
        100
    );
}

#[test]
fn effect_tick_marks_only_rows_with_active_effects() {
    let (mut state, _, pos) = flat_player_fixture();
    // Effect-free rows that still pass the run-loop admission filter via a
    // stale appearance marker (leftover glow, no entries): nothing to tick,
    // so the journal touched set must stay clean.
    let plain = mob(&mut state, "cow", [pos[0] + 2.0, pos[1], pos[2]]);
    let plain_slot = state.entities.slot_of(plain).unwrap();
    std::sync::Arc::make_mut(&mut state.entities.row_mut(plain_slot).appearance).shared_flags |=
        0x40;
    let villager_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let villager = state.spawn_mob(villager_type, [pos[0] - 2.0, pos[1], pos[2]], 20.0);
    let villager_slot = state.entities.slot_of(villager).unwrap();
    std::sync::Arc::make_mut(&mut state.entities.row_mut(villager_slot).appearance).shared_flags |=
        0x40;
    // Row with a real effect: Regeneration at amplifier 0 ticks every 50
    // ticks; duration 3600 hits the interval exactly and heals below max.
    let dosed = mob(&mut state, "cow", [pos[0] + 4.0, pos[1], pos[2]]);
    let dosed_slot = state.entities.slot_of(dosed).unwrap();
    state.entities.row_mut(dosed_slot).health = 5.0;
    effect(&mut state, dosed, EffectId::Regeneration);
    // Drain spawn/setup marks: everything observed below comes from the tick.
    let _ = state.entities.take_touched();
    let mut intents = Vec::new();
    crate::effects::run(&mut state, &mut intents);
    let touched = state.entities.take_touched();
    assert!(
        !touched.contains(&plain),
        "effect-free cow tick must not mark the journal"
    );
    assert!(
        !touched.contains(&villager),
        "effect-free villager tick must not mark the journal"
    );
    assert!(
        touched.contains(&dosed),
        "ticked effect row must stay marked"
    );
    let dosed_slot = state.entities.slot_of(dosed).unwrap();
    assert_eq!(
        state
            .entities
            .row(dosed_slot)
            .active_effects
            .get(EffectId::Regeneration)
            .unwrap()
            .duration,
        3599
    );
    assert_eq!(state.entities.row(dosed_slot).health, 6.0);
}
