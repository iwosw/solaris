use super::*;
use crate::journal::{EntityAppearanceImage, EntityStateImage, ReplayEntity};
use crate::projectiles::{ProjectileData, ProjectileKind};

fn fixture(name: &str, range: f64) -> (TickState, EntityId, EntityId, [f64; 3]) {
    let (mut state, player, pos) = crate::hostile_tests::setup(range);
    state.difficulty = crate::survival::Difficulty::Normal;
    let type_id = summons::type_id(&state, &format!("minecraft:{name}")).unwrap();
    let health = state.registry.blueprint(type_id).unwrap().params.max_health;
    let mob = state.spawn_mob(type_id, pos, health);
    // These fixtures exercise already-engaged attacks, not anger acquisition.
    if name == "warden" {
        let uuid = state
            .entities
            .row(state.entities.slot_of(player).unwrap())
            .uuid;
        let slot = state.entities.slot_of(mob).unwrap();
        if let Some(Retained::Warden(value)) =
            state.entities.row_mut(slot).special_mob.as_deref_mut()
        {
            value.anger.insert(uuid, 150);
            value.roaring_target = Some(uuid);
        }
    }
    (state, mob, player, pos)
}
fn advance(
    state: &mut TickState,
    id: EntityId,
    tick: u64,
    kind: GoalKind,
    player: EntityId,
) -> Vec<Intent> {
    state.tick = tick;
    let targets = state
        .entities
        .slot_of(player)
        .map(|slot| vec![(player, state.entities.pos(slot))])
        .unwrap_or_default();
    let mut intents = Vec::new();
    run(state, &mut intents, id, kind, &targets);
    intents
}
fn missiles(state: &TickState, kind: ProjectileKind) -> Vec<EntityId> {
    state
        .entities
        .slots()
        .filter_map(|slot| {
            state
                .entities
                .row(slot)
                .projectile
                .as_ref()
                .filter(|p| p.kind == kind)
                .map(|_| state.entities.body(slot).id)
        })
        .collect()
}
fn wall(state: &mut TickState, pos: [f64; 3]) {
    for y in pos[1] as i32..=pos[1] as i32 + 6 {
        for z in pos[2] as i32 - 2..=pos[2] as i32 + 2 {
            state
                .world
                .set_block(pos[0] as i32 - 2, y, z, state.registry.states.stone);
        }
    }
}
fn restore(state: &mut TickState, id: EntityId) {
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row(slot);
    let record = ReplayEntity {
        uuid: row.uuid.to_string(),
        type_id: match state.entities.body(slot).kind {
            crate::entities::EntityKind::Mob { type_id } => type_id,
            _ => panic!("mob"),
        },
        pos: state.entities.pos(slot),
        vel: state.entities.vel(slot),
        yaw: state.entities.rot(slot)[0],
        pitch: 0.0,
        ground: state.entities.ground(slot),
        health: row.health,
        alive: state.entities.body(slot).alive,
        age: row.age,
        breed_cooldown: row.breed_cooldown,
        pickup_delay: 0,
        item: None,
        appearance: EntityAppearanceImage::from(row.appearance.as_ref()),
        active_effects: row.active_effects.clone(),
        villager: None,
        state: EntityStateImage::from_row(state.entities.body(slot), row),
        projectile: row.projectile.as_deref().cloned(),
        tick: state.tick,
    };
    let bytes = serde_json::to_vec(&record).unwrap();
    let record: ReplayEntity = serde_json::from_slice(&bytes).unwrap();
    state.remove_entity(slot);
    state.hostile.native = Runtime::default();
    crate::core::restore_entity(state, id.0, &record).unwrap();
}
#[test]
fn blueprint_registration_keeps_all_special_families_native() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    for name in [
        "blaze",
        "ghast",
        "pillager",
        "witch",
        "breeze",
        "guardian",
        "elder_guardian",
        "warden",
        "evoker",
        "shulker",
        "wither",
        "evoker_fangs",
        "vex",
    ] {
        let bp = registry
            .blueprints
            .iter()
            .find(|(_, bp)| bp.name == format!("minecraft:{name}"))
            .unwrap()
            .1
            .clone();
        assert!(goal(&bp).is_some(), "{name}");
    }
}
#[test]
fn blaze_three_shot_burst_then_cooldown_and_melee_twenty_ticks() {
    let (mut state, blaze, player, pos) = fixture("blaze", 8.0);
    let mut times = Vec::new();
    let mut count = 0;
    for tick in 0..=239 {
        advance(&mut state, blaze, tick, GoalKind::BlazeBurst, player);
        let n = missiles(&state, ProjectileKind::SmallFireball).len();
        if n > count {
            times.push(tick);
            count = n;
        }
    }
    assert_eq!(times, [60, 66, 72, 238]);
    state.hostile.native = Runtime::default();
    state.entities.set_pos(
        state.entities.slot_of(player).unwrap(),
        [pos[0] - 1.0, pos[1], pos[2]],
    );
    assert_eq!(
        advance(&mut state, blaze, 300, GoalKind::BlazeBurst, player).len(),
        1
    );
    assert!(advance(&mut state, blaze, 319, GoalKind::BlazeBurst, player).is_empty());
    assert_eq!(
        advance(&mut state, blaze, 320, GoalKind::BlazeBurst, player).len(),
        1
    );
}
#[test]
fn ghast_twenty_charge_sixty_repeat_and_muzzle_outside_body() {
    let (mut state, ghast, player, pos) = fixture("ghast", 8.0);
    for tick in 0..19 {
        advance(&mut state, ghast, tick, GoalKind::SiegeShot, player);
    }
    assert!(missiles(&state, ProjectileKind::Fireball).is_empty());
    advance(&mut state, ghast, 19, GoalKind::SiegeShot, player);
    let fireball = missiles(&state, ProjectileKind::Fireball)[0];
    let slot = state.entities.slot_of(fireball).unwrap();
    assert_eq!(
        state.entities.pos(slot),
        [pos[0] - 4.0, pos[1] + 2.5, pos[2]]
    );
    for tick in 20..79 {
        advance(&mut state, ghast, tick, GoalKind::SiegeShot, player);
    }
    assert_eq!(missiles(&state, ProjectileKind::Fireball).len(), 1);
    advance(&mut state, ghast, 79, GoalKind::SiegeShot, player);
    assert_eq!(missiles(&state, ProjectileKind::Fireball).len(), 2);
}
#[test]
fn pillager_charged_deadline_survives_restart_and_shoots_once() {
    let (mut state, mob, player, _) = fixture("pillager", 7.0);
    for tick in 0..=29 {
        advance(&mut state, mob, tick, GoalKind::Crossbow, player);
    }
    let slot = state.entities.slot_of(mob).unwrap();
    let Some(&Retained::Crossbow(Some(crossbow))) = state.entities.row(slot).special_mob.as_deref()
    else {
        panic!("charge")
    };
    assert_eq!(crossbow.phase, CrossbowPhase::Charged);
    assert!((49..=68).contains(&crossbow.deadline));
    restore(&mut state, mob);
    advance(
        &mut state,
        mob,
        crossbow.deadline - 1,
        GoalKind::Crossbow,
        player,
    );
    assert!(missiles(&state, ProjectileKind::Arrow).is_empty());
    advance(
        &mut state,
        mob,
        crossbow.deadline,
        GoalKind::Crossbow,
        player,
    );
    assert_eq!(missiles(&state, ProjectileKind::Arrow).len(), 1);
    restore(&mut state, mob);
    advance(
        &mut state,
        mob,
        crossbow.deadline + 1,
        GoalKind::Crossbow,
        player,
    );
    assert_eq!(missiles(&state, ProjectileKind::Arrow).len(), 1);
}
#[test]
fn witch_selects_real_potion_payload_and_drinks_without_throwing() {
    let (mut state, witch, player, _) = fixture("witch", 9.0);
    advance(&mut state, witch, 0, GoalKind::Alchemist, player);
    advance(&mut state, witch, 59, GoalKind::Alchemist, player);
    assert!(missiles(&state, ProjectileKind::Potion).is_empty());
    advance(&mut state, witch, 60, GoalKind::Alchemist, player);
    let potion = missiles(&state, ProjectileKind::Potion)[0];
    let row = state.entities.row(state.entities.slot_of(potion).unwrap());
    assert_eq!(
        row.projectile
            .as_ref()
            .unwrap()
            .stack
            .extended()
            .potion
            .as_ref()
            .unwrap()
            .path(),
        "slowness"
    );
    restore(&mut state, witch);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(witch).unwrap())
            .special_mob
            .as_deref()
            .cloned(),
        Some(Retained::Witch(Potion::Slowness))
    );
    let slot = state.entities.slot_of(witch).unwrap();
    state.entities.row_mut(slot).health = 18.0;
    advance(&mut state, witch, 100, GoalKind::Alchemist, player);
    advance(&mut state, witch, 131, GoalKind::Alchemist, player);
    assert_eq!(state.entities.row(slot).health, 18.0);
    advance(&mut state, witch, 132, GoalKind::Alchemist, player);
    assert_eq!(state.entities.row(slot).health, 22.0);
    assert_eq!(missiles(&state, ProjectileKind::Potion).len(), 1);
}

#[test]
fn witch_finishing_drink_restores_native_movement_attribute_not_blueprint_default() {
    let (mut state, witch, player, _) = fixture("witch", 9.0);
    let slot = state.entities.slot_of(witch).unwrap();
    state
        .entities
        .row_mut(slot)
        .loot
        .attributes
        .push(crate::entity_loot::EntityAttribute {
            id: "minecraft:movement_speed".into(),
            base: 0.4,
            modifiers: Vec::new(),
        });
    crate::entity_loot::restore(&mut state, slot);
    assert_eq!(state.entities.row(slot).speed, 4.0);
    state.entities.row_mut(slot).health = 18.0;
    advance(&mut state, witch, 100, GoalKind::Alchemist, player);
    assert_eq!(state.entities.row(slot).speed, 1.0);
    advance(&mut state, witch, 132, GoalKind::Alchemist, player);
    assert_eq!(state.entities.row(slot).health, 22.0);
    assert_eq!(state.entities.row(slot).speed, 4.0);
}
#[test]
fn breeze_charges_fifteen_then_recovery_jump_with_point_seven_speed() {
    let (mut state, breeze, player, _) = fixture("breeze", 8.0);
    let slot = state.entities.slot_of(breeze).unwrap();
    state.entities.set_ground(slot, true);
    advance(&mut state, breeze, 0, GoalKind::WindCharge, player);
    advance(&mut state, breeze, 14, GoalKind::WindCharge, player);
    assert!(missiles(&state, ProjectileKind::BreezeWindCharge).is_empty());
    advance(&mut state, breeze, 15, GoalKind::WindCharge, player);
    let charge = missiles(&state, ProjectileKind::BreezeWindCharge)[0];
    let speed = state
        .entities
        .vel(state.entities.slot_of(charge).unwrap())
        .iter()
        .map(|v| v * v)
        .sum::<f64>()
        .sqrt()
        / 20.0;
    assert!((speed - 0.7).abs() < 1e-10);
    advance(&mut state, breeze, 19, GoalKind::WindCharge, player);
    assert_eq!(state.entities.vel(slot)[1], 14.0);
    assert!(!state.entities.ground(slot));
}
#[test]
fn all_visible_shooters_refuse_walls_and_out_of_range() {
    for (name, kind, projectile, range) in [
        (
            "blaze",
            GoalKind::BlazeBurst,
            ProjectileKind::SmallFireball,
            48.0,
        ),
        ("ghast", GoalKind::SiegeShot, ProjectileKind::Fireball, 64.0),
        ("pillager", GoalKind::Crossbow, ProjectileKind::Arrow, 8.0),
        ("witch", GoalKind::Alchemist, ProjectileKind::Potion, 10.0),
        (
            "breeze",
            GoalKind::WindCharge,
            ProjectileKind::BreezeWindCharge,
            16.0,
        ),
        (
            "shulker",
            GoalKind::HomingShot,
            ProjectileKind::ShulkerBullet,
            20.0,
        ),
    ] {
        let (mut state, mob, player, pos) = fixture(name, 7.0);
        wall(&mut state, pos);
        for tick in 0..100 {
            advance(&mut state, mob, tick, kind, player);
        }
        assert!(missiles(&state, projectile).is_empty(), "{name} LOS");
        state.entities.set_pos(
            state.entities.slot_of(player).unwrap(),
            [pos[0] + range + 1.0, pos[1], pos[2]],
        );
        for tick in 100..200 {
            advance(&mut state, mob, tick, kind, player);
        }
        assert!(missiles(&state, projectile).is_empty(), "{name} range");
    }
}
#[test]
fn guardian_beam_warmup_duration_damage_minimum_and_los() {
    for (name, kind, duration, amount) in [
        ("guardian", GoalKind::Beam, 80, 7.0),
        ("elder_guardian", GoalKind::ElderBeam, 60, 11.0),
    ] {
        let (mut state, mob, player, pos) = fixture(name, 8.0);
        assert!(advance(&mut state, mob, 0, kind, player).is_empty());
        assert!(advance(&mut state, mob, 10, kind, player).is_empty());
        assert!(
            state
                .hostile
                .events
                .iter()
                .any(|event| matches!(event, EntityEvent::Status { event: 21, .. }))
        );
        assert!(advance(&mut state, mob, 10 + duration - 1, kind, player).is_empty());
        let hits = advance(&mut state, mob, 10 + duration, kind, player);
        assert!(
            matches!(hits[0].kind, IntentKind::DamageEntity { amount: value, .. } if value == amount)
        );
        state.hostile.native = Runtime::default();
        wall(&mut state, pos);
        assert!(advance(&mut state, mob, 200, kind, player).is_empty());
        assert_eq!(state.hostile.native.attacks[&mob].stage, 0);
        state.entities.set_pos(
            state.entities.slot_of(player).unwrap(),
            [pos[0], pos[1], pos[2] + 2.0],
        );
        assert!(advance(&mut state, mob, 201, kind, player).is_empty());
        assert_eq!(state.hostile.native.attacks[&mob].stage, 0);
    }
}
#[test]
fn elder_aura_mining_fatigue_is_wall_independent_and_radius_limited() {
    let (mut state, elder, player, pos) = fixture("elder_guardian", 8.0);
    wall(&mut state, pos);
    let tick = 1200 - u64::from(elder.0.unsigned_abs()) % 1200;
    advance(&mut state, elder, tick, GoalKind::ElderBeam, player);
    let effect = state.players[&player]
        .active_effects
        .get(EffectId::MiningFatigue)
        .unwrap();
    assert_eq!((effect.amplifier, effect.duration), (2, 6000));
}
#[test]
fn warden_sonic_pierces_walls_after_thirty_four_and_keeps_cooldown() {
    let (mut state, warden, player, pos) = fixture("warden", 8.0);
    wall(&mut state, pos);
    advance(&mut state, warden, 0, GoalKind::SonicBoom, player);
    assert!(advance(&mut state, warden, 33, GoalKind::SonicBoom, player).is_empty());
    let hits = advance(&mut state, warden, 34, GoalKind::SonicBoom, player);
    assert!(matches!(
        hits[0].kind,
        IntentKind::DamageEntity { amount: 10.0, .. }
    ));
    assert!(advance(&mut state, warden, 60, GoalKind::SonicBoom, player).is_empty());
    assert!(advance(&mut state, warden, 99, GoalKind::SonicBoom, player).is_empty());
    state.hostile.native = Runtime::default();
    state.entities.set_pos(
        state.entities.slot_of(player).unwrap(),
        [pos[0] + 15.01, pos[1], pos[2]],
    );
    advance(&mut state, warden, 101, GoalKind::SonicBoom, player);
    assert_eq!(state.hostile.native.attacks[&warden].stage, 0);
}
#[test]
fn evoker_casts_line_and_rings_with_owner_warmup_and_life_restart() {
    assert_eq!(summons::pattern([0.0; 3], [1.0, 0.0, 0.0]).len(), 13);
    let line = summons::pattern([0.0; 3], [8.0, 0.0, 0.0]);
    assert_eq!(line.len(), 16);
    assert_eq!(line[15].1, 15);
    assert_eq!(line[15].0[0], 20.0);
    let (mut state, evoker, player, _) = fixture("evoker", 8.0);
    advance(&mut state, evoker, 0, GoalKind::Fangs, player);
    advance(&mut state, evoker, 19, GoalKind::Fangs, player);
    assert!(!state.entities.slots().any(|slot| matches!(
        state.entities.row(slot).special_mob.as_deref().cloned(),
        Some(Retained::Fangs { .. })
    )));
    advance(&mut state, evoker, 20, GoalKind::Fangs, player);
    let fang = state
        .entities
        .slots()
        .find_map(|slot| {
            matches!(
                state.entities.row(slot).special_mob.as_deref().cloned(),
                Some(Retained::Fangs { .. })
            )
            .then_some(state.entities.body(slot).id)
        })
        .unwrap();
    let before = state
        .entities
        .row(state.entities.slot_of(fang).unwrap())
        .special_mob
        .clone();
    restore(&mut state, fang);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(fang).unwrap())
            .special_mob,
        before
    );
    assert_eq!(
        state
            .entities
            .slots()
            .filter(|slot| matches!(
                state.entities.row(*slot).special_mob.as_deref().cloned(),
                Some(Retained::Vex { .. })
            ))
            .count(),
        3
    );
    for tick in 21..60 {
        advance(&mut state, fang, tick, GoalKind::FangLife, player);
    }
    assert!(state.entities.slot_of(fang).is_none());
}
#[test]
fn fangs_damage_once_after_warmup_and_vex_summons_cap_at_six() {
    let (mut state, evoker, player, pos) = fixture("evoker", 1.0);
    let fang_type = summons::type_id(&state, "minecraft:evoker_fangs").unwrap();
    let player_pos = state.entities.pos(state.entities.slot_of(player).unwrap());
    let fang = state.spawn_mob(fang_type, player_pos, 1.0);
    let slot = state.entities.slot_of(fang).unwrap();
    state.entities.row_mut(slot).special_mob = Some(Box::new(Retained::Fangs {
        owner: Some(
            state
                .entities
                .row(state.entities.slot_of(evoker).unwrap())
                .uuid,
        ),
        warmup: 2,
        life: 22,
    }));
    let mut hit_times = Vec::new();
    for tick in 0..=26 {
        if !advance(&mut state, fang, tick, GoalKind::FangLife, player).is_empty() {
            hit_times.push(tick);
        }
    }
    assert_eq!(hit_times, [11]);
    for tick in [0, 20, 40, 100, 101, 121, 141, 201, 341, 361] {
        advance(&mut state, evoker, tick, GoalKind::Fangs, player);
    }
    assert!(
        state
            .entities
            .slots()
            .filter(|slot| matches!(
                state.entities.row(*slot).special_mob.as_deref().cloned(),
                Some(Retained::Vex { .. })
            ))
            .count()
            <= 6
    );
    let player_slot = state.entities.slot_of(player).unwrap();
    state
        .entities
        .set_pos(player_slot, [pos[0] - 8.0, pos[1], pos[2]]);
    wall(&mut state, pos);
    state.hostile.native = Runtime::default();
    advance(&mut state, evoker, 500, GoalKind::Fangs, player);
    assert_eq!(state.hostile.native.attacks[&evoker].stage, 0);
}
#[test]
fn shulker_twenty_delay_peek_homing_target_restart_and_levitation() {
    let (mut state, shulker, player, _) = fixture("shulker", 8.0);
    advance(&mut state, shulker, 0, GoalKind::HomingShot, player);
    advance(&mut state, shulker, 19, GoalKind::HomingShot, player);
    assert!(missiles(&state, ProjectileKind::ShulkerBullet).is_empty());
    advance(&mut state, shulker, 20, GoalKind::HomingShot, player);
    let bullet = missiles(&state, ProjectileKind::ShulkerBullet)[0];
    restore(&mut state, bullet);
    let slot = state.entities.slot_of(bullet).unwrap();
    let record = state.entities.row(slot).projectile.as_ref().unwrap();
    let target_uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    assert_eq!(
        record.data,
        ProjectileData::Homing {
            target: target_uuid
        }
    );
    let old_velocity = state.entities.vel(slot);
    let player_slot = state.entities.slot_of(player).unwrap();
    let at = state.entities.pos(player_slot);
    state
        .entities
        .set_pos(player_slot, [at[0], at[1], at[2] + 3.0]);
    let mut intents = Vec::new();
    crate::projectiles::run(
        &mut state,
        &mut intents,
        &[(bullet, crate::hostile::Special::Projectile)],
    );
    assert!(state.entities.vel(slot)[2] > old_velocity[2]);
    let mut record = state.entities.row(slot).projectile.clone().unwrap();
    let pos = state.entities.pos(player_slot);
    crate::projectiles::payloads::impact(
        &mut state,
        &mut intents,
        bullet,
        &mut record,
        Some(player),
        pos,
    );
    state.merge(&mut intents);
    state.commit_merge_effects();
    assert!(
        state.players[&player]
            .active_effects
            .has(EffectId::Levitation)
    );
}
#[test]
fn retained_checkpoint_projection_and_corruption_fail_closed() {
    let (mut state, mob, _, _) = fixture("wither", 8.0);
    let slot = state.entities.slot_of(mob).unwrap();
    state.entities.row_mut(slot).special_mob =
        Some(Box::new(Retained::Wither { invulnerable: 127 }));
    let (_, regions, _) = crate::core::checkpoint_data_for(&state, 0);
    let native = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.id == "minecraft:wither")
        .unwrap();
    let image = EntityStateImage::from_record(&state.registry, native).unwrap();
    assert_eq!(
        image.special_mob,
        Some(Retained::Wither { invulnerable: 127 })
    );
    let mut corrupt = native.clone();
    corrupt
        .extra_fields
        .retain(|(name, _)| name != "solaris:SpecialMob");
    corrupt
        .extra_fields
        .push(("Invul".into(), mc_nbt::Tag::Int(221)));
    assert!(
        EntityStateImage::from_record(&state.registry, &corrupt)
            .unwrap_err()
            .to_string()
            .contains("Wither.invulnerable")
    );
    for (value, diagnostic) in [
        (mc_nbt::Tag::Int(221), "Wither.invulnerable"),
        (mc_nbt::Tag::Byte(127), "expected int"),
        (mc_nbt::Tag::Int(126), "native/Invul mismatch"),
    ] {
        let mut corrupt = native.clone();
        let invul = corrupt
            .extra_fields
            .iter_mut()
            .find(|(key, _)| key == "Invul")
            .unwrap();
        invul.1 = value;
        assert!(
            EntityStateImage::from_record(&state.registry, &corrupt)
                .unwrap_err()
                .to_string()
                .contains(diagnostic)
        );
    }
    let mut json = serde_json::to_value(image).unwrap();
    json["special_mob"]["Wither"]["invulnerable"] = 221.into();
    assert!(
        serde_json::from_value::<EntityStateImage>(json)
            .unwrap_err()
            .to_string()
            .contains("Wither.invulnerable")
    );
}
#[test]
fn wither_ritual_spawn_phase_restart_three_heads_armor_grief_and_star() {
    let (mut state, original, player, pos) = fixture("wither", 8.0);
    state.remove_entity(state.entities.slot_of(original).unwrap());
    let block = |state: &TickState, name: &str| {
        state
            .registry
            .blocks
            .block(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap()
            .default
            .0
    };
    let soul = block(&state, "soul_sand");
    let skull = block(&state, "wither_skeleton_skull");
    let center = pos.map(|value| value.floor() as i32);
    let mut writes = Vec::new();
    for dx in -1..=1 {
        for (dy, block) in [(1, soul), (2, skull)] {
            let at = crate::boundary::BlockPos {
                x: center[0] + dx,
                y: center[1] + dy,
                z: center[2],
            };
            state.world.set_block(at.x, at.y, at.z, block);
            writes.push((at, block));
        }
    }
    state.world.set_block(center[0], center[1], center[2], soul);
    after_blocks(&mut state, &mut writes);
    let wither = state
        .entities
        .slots()
        .find_map(|slot| {
            matches!(
                state.entities.row(slot).special_mob.as_deref().cloned(),
                Some(Retained::Wither { .. })
            )
            .then_some(state.entities.body(slot).id)
        })
        .unwrap();
    assert_eq!(
        state
            .hostile
            .spawned
            .iter()
            .filter(|id| **id == wither)
            .count(),
        1
    );
    after_blocks(&mut state, &mut writes);
    assert_eq!(
        state
            .hostile
            .spawned
            .iter()
            .filter(|id| **id == wither)
            .count(),
        1
    );
    let mut pending = Vec::new();
    for tick in 0..100 {
        pending.extend(advance(
            &mut state,
            wither,
            tick,
            GoalKind::WitherBarrage,
            player,
        ));
    }
    assert!(missiles(&state, ProjectileKind::WitherSkull).is_empty());
    restore(&mut state, wither);
    for tick in 100..220 {
        pending.extend(advance(
            &mut state,
            wither,
            tick,
            GoalKind::WitherBarrage,
            player,
        ));
    }
    assert_eq!(
        pending
            .iter()
            .filter(|intent| matches!(intent.kind, IntentKind::Explode { power: 7.0, .. }))
            .count(),
        1
    );
    advance(&mut state, wither, 258, GoalKind::WitherBarrage, player);
    assert!(missiles(&state, ProjectileKind::WitherSkull).is_empty());
    advance(&mut state, wither, 259, GoalKind::WitherBarrage, player);
    assert_eq!(missiles(&state, ProjectileKind::WitherSkull).len(), 3);
    let slot = state.entities.slot_of(wither).unwrap();
    state.entities.row_mut(slot).health = 150.0;
    let piece = crate::tick::DamagePiece {
        physical: None,
        custom_armor: false,
        amount: 10.0,
        kind: DamageKind::Projectile,
        source: None,
    };
    assert!(!damage_allowed(state.entities.row(slot), &piece));
    state.entities.body_mut(slot).hurt_at = Some(270);
    let at = state.entities.pos(slot).map(|value| value.floor() as i32);
    state
        .world
        .set_block(at[0], at[1], at[2], state.registry.states.stone);
    assert!(
        advance(&mut state, wither, 290, GoalKind::WitherBarrage, player)
            .iter()
            .any(|intent| matches!(intent.kind, IntentKind::BreakBlock { .. }))
    );
    let seq = state.next_seq(player);
    let mut intents = vec![Intent {
        actor: player,
        seq,
        kind: IntentKind::DamageEntity {
            target: wither,
            amount: 1000.0,
            kind: DamageKind::Void,
            source: None,
        },
        footprint: Footprint::default(),
    }];
    state.merge(&mut intents);
    let (_, _, drops, _) = state.commit_merge_effects();
    let star = item_id(&state, "nether_star").unwrap();
    assert!(
        drops
            .iter()
            .any(|id| state.entities.slot_of(*id).is_some_and(|slot| state
                .entities
                .row(slot)
                .item
                .as_ref()
                .is_some_and(|stack| stack.item_id == star)))
    );
}

#[test]
fn transient_decision_timers_reset_on_actual_entity_restore() {
    for (name, kind, warmup) in [
        ("blaze", GoalKind::BlazeBurst, 60),
        ("breeze", GoalKind::WindCharge, 15),
        ("guardian", GoalKind::Beam, 10),
        ("elder_guardian", GoalKind::ElderBeam, 10),
        ("warden", GoalKind::SonicBoom, 34),
        ("evoker", GoalKind::Fangs, 20),
        ("shulker", GoalKind::HomingShot, 20),
        ("witch", GoalKind::Alchemist, 60),
    ] {
        let (mut state, mob, player, _) = fixture(name, 7.0);
        advance(&mut state, mob, 0, kind, player);
        advance(&mut state, mob, 1, kind, player);
        restore(&mut state, mob);
        assert!(state.hostile.native.attacks.is_empty());
        advance(&mut state, mob, 10, kind, player);
        assert_eq!(
            state.hostile.native.attacks[&mob].deadline,
            10 + warmup,
            "{name}"
        );
    }
    let (mut state, mob, player, _) = fixture("ghast", 7.0);
    for tick in 0..10 {
        advance(&mut state, mob, tick, GoalKind::SiegeShot, player);
    }
    restore(&mut state, mob);
    advance(&mut state, mob, 11, GoalKind::SiegeShot, player);
    assert_eq!(state.hostile.native.attacks[&mob].stage, 1);
}

#[test]
fn special_shooters_deliver_existing_payload_damage_not_placeholder_missiles() {
    for (name, kind, damage_amount) in [
        ("blaze", ProjectileKind::SmallFireball, 5.0),
        ("ghast", ProjectileKind::Fireball, 6.0),
        ("wither", ProjectileKind::WitherSkull, 8.0),
        ("breeze", ProjectileKind::BreezeWindCharge, 1.0),
        ("shulker", ProjectileKind::ShulkerBullet, 4.0),
    ] {
        let (mut state, owner, player, pos) = fixture(name, 7.0);
        let victim_pos = state.entities.pos(state.entities.slot_of(player).unwrap());
        let projectile = shooters::shoot(
            &mut state,
            owner,
            kind,
            [pos[0], pos[1] + 1.0, pos[2]],
            victim_pos,
            0.5,
            None,
        )
        .unwrap();
        let mut record = state
            .entities
            .row(state.entities.slot_of(projectile).unwrap())
            .projectile
            .clone()
            .unwrap();
        let mut intents = Vec::new();
        crate::projectiles::payloads::impact(
            &mut state,
            &mut intents,
            projectile,
            &mut record,
            Some(player),
            victim_pos,
        );
        assert!(intents.iter().any(|intent| matches!(intent.kind, IntentKind::DamageEntity { target, amount, .. } if target == player && amount == damage_amount)), "{name}");
    }
}

#[test]
fn witch_near_target_selects_poison_then_harming_and_harming_deals_six() {
    let (mut state, witch, player, pos) = fixture("witch", 6.0);
    advance(&mut state, witch, 0, GoalKind::Alchemist, player);
    advance(&mut state, witch, 60, GoalKind::Alchemist, player);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(witch).unwrap())
            .special_mob
            .as_deref()
            .cloned(),
        Some(Retained::Witch(Potion::Poison))
    );
    effect(
        &mut state,
        player,
        ActiveEffect::new(EffectId::Poison, 100, 0),
    );
    advance(&mut state, witch, 120, GoalKind::Alchemist, player);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(witch).unwrap())
            .special_mob
            .as_deref()
            .cloned(),
        Some(Retained::Witch(Potion::Harming))
    );
    let at = state.entities.pos(state.entities.slot_of(player).unwrap());
    let projectile = shooters::shoot(
        &mut state,
        witch,
        ProjectileKind::Potion,
        pos,
        at,
        0.75,
        Some(Potion::Harming),
    )
    .unwrap();
    let mut record = state
        .entities
        .row(state.entities.slot_of(projectile).unwrap())
        .projectile
        .clone()
        .unwrap();
    let mut intents = Vec::new();
    crate::projectiles::payloads::impact(
        &mut state,
        &mut intents,
        projectile,
        &mut record,
        Some(player),
        at,
    );
    assert!(intents.iter().any(|intent| matches!(intent.kind, IntentKind::DamageEntity { target, amount: 6.0, .. } if target == player)));
}

#[test]
fn every_retained_special_variant_round_trips_real_checkpoint_fields() {
    for (name, value) in [
        (
            "pillager",
            Retained::Crossbow(Some(Crossbow {
                phase: CrossbowPhase::Charged,
                deadline: 101,
            })),
        ),
        ("witch", Retained::Witch(Potion::Poison)),
        (
            "evoker_fangs",
            Retained::Fangs {
                owner: Some(Uuid::from_u128(33)),
                warmup: -1,
                life: 13,
            },
        ),
        (
            "vex",
            Retained::Vex {
                owner: Some(Uuid::from_u128(34)),
                life: 321,
            },
        ),
        ("wither", Retained::Wither { invulnerable: 123 }),
    ] {
        let (mut state, mob, _, _) = fixture(name, 7.0);
        let slot = state.entities.slot_of(mob).unwrap();
        state.entities.row_mut(slot).special_mob = Some(Box::new(value.clone()));
        let uuid = state.entities.row(slot).uuid;
        let (_, regions, _) = crate::core::checkpoint_data_for(&state, 0);
        let entity = regions
            .values()
            .flat_map(|chunk| &chunk.entities)
            .find(|entity| entity.uuid == uuid)
            .unwrap();
        let image = EntityStateImage::from_record(&state.registry, entity).unwrap();
        assert_eq!(image.special_mob, Some(value), "{name}");
        if name == "evoker_fangs" {
            for (key, tag, diagnostic) in [
                ("Warmup", mc_nbt::Tag::Int(-2), "Fangs"),
                ("Warmup", mc_nbt::Tag::Int(0), "native/Warmup mismatch"),
                ("Warmup", mc_nbt::Tag::Byte(0), "expected int"),
                ("Owner", mc_nbt::Tag::String("bad".into()), "UUID int array"),
                (
                    "Owner",
                    mc_nbt::Tag::IntArray(vec![1, 2, 3]),
                    "four UUID words",
                ),
                (
                    "Owner",
                    persistence::uuid_tag(Uuid::from_u128(35)),
                    "native/Owner mismatch",
                ),
            ] {
                let mut bad = entity.clone();
                bad.extra_fields
                    .iter_mut()
                    .find(|(name, _)| name == key)
                    .unwrap()
                    .1 = tag;
                assert!(
                    EntityStateImage::from_record(&state.registry, &bad)
                        .unwrap_err()
                        .to_string()
                        .contains(diagnostic),
                    "{key}"
                );
            }
            let mut bad = entity.clone();
            bad.extra_fields
                .push(("Owner".into(), persistence::uuid_tag(Uuid::from_u128(33))));
            assert!(
                EntityStateImage::from_record(&state.registry, &bad)
                    .unwrap_err()
                    .to_string()
                    .contains("Fangs.owner: duplicate")
            );
        }
        let mut bad = entity.clone();
        bad.id = "minecraft:cow".into();
        assert!(
            EntityStateImage::from_record(&state.registry, &bad)
                .unwrap_err()
                .to_string()
                .contains("incompatible")
        );
    }
}

#[test]
fn wither_los_range_spawn_immunity_and_grief_rule_are_enforced() {
    let (mut state, wither, player, pos) = fixture("wither", 8.0);
    let slot = state.entities.slot_of(wither).unwrap();
    state.entities.row_mut(slot).special_mob =
        Some(Box::new(Retained::Wither { invulnerable: 100 }));
    assert!(!damage_allowed(
        state.entities.row(slot),
        &crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: DamageKind::Generic,
            source: None
        }
    ));
    assert!(damage_allowed(
        state.entities.row(slot),
        &crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: DamageKind::Void,
            source: None
        }
    ));
    state.entities.row_mut(slot).special_mob = Some(Box::new(Retained::Wither { invulnerable: 0 }));
    wall(&mut state, pos);
    advance(&mut state, wither, 0, GoalKind::WitherBarrage, player);
    advance(&mut state, wither, 40, GoalKind::WitherBarrage, player);
    assert!(missiles(&state, ProjectileKind::WitherSkull).is_empty());
    state.entities.set_pos(
        state.entities.slot_of(player).unwrap(),
        [pos[0], pos[1], pos[2] + 21.0],
    );
    advance(&mut state, wither, 80, GoalKind::WitherBarrage, player);
    assert!(missiles(&state, ProjectileKind::WitherSkull).is_empty());
    state.entities.body_mut(slot).hurt_at = Some(80);
    state.world.set_block(
        pos[0] as i32,
        pos[1] as i32,
        pos[2] as i32,
        state.registry.states.stone,
    );
    state
        .gamerules
        .insert("mob_griefing".into(), "false".into());
    assert!(
        !advance(&mut state, wither, 100, GoalKind::WitherBarrage, player)
            .iter()
            .any(|intent| matches!(intent.kind, IntentKind::BreakBlock { .. }))
    );
}

#[test]
fn casting_and_wind_recovery_cooldowns_survive_target_loss() {
    for (name, kind, charge, recovery, cooldown) in [
        ("evoker", GoalKind::Fangs, 20, 20, 60),
        ("breeze", GoalKind::WindCharge, 15, 4, 10),
    ] {
        let (mut state, mob, player, pos) = fixture(name, 8.0);
        advance(&mut state, mob, 0, kind, player);
        advance(&mut state, mob, charge, kind, player);
        assert_eq!(state.hostile.native.attacks[&mob].stage, 2);
        wall(&mut state, pos);
        advance(&mut state, mob, charge + 1, kind, player);
        assert_eq!(state.hostile.native.attacks[&mob].stage, 2);
        advance(&mut state, mob, charge + recovery, kind, player);
        assert_eq!(state.hostile.native.attacks[&mob].stage, 3);
        advance(
            &mut state,
            mob,
            charge + recovery + cooldown - 1,
            kind,
            player,
        );
        assert_eq!(state.hostile.native.attacks[&mob].stage, 3);
        advance(&mut state, mob, charge + recovery + cooldown, kind, player);
        assert_eq!(state.hostile.native.attacks[&mob].stage, 0);
    }
}

#[test]
fn wither_custom_star_drop_respects_mob_drops_and_durable_once_guard() {
    let (mut state, wither, _, _) = fixture("wither", 8.0);
    let slot = state.entities.slot_of(wither).unwrap();
    state.entities.body_mut(slot).alive = false;
    state.gamerules.insert("mob_drops".into(), "false".into());
    let mut drops = Vec::new();
    let cause = crate::entity_loot::death_cause(&state, slot, None, DamageKind::Void);
    crate::entity_loot::commit_death(&mut state, slot, cause, &mut drops);
    assert!(drops.is_empty());
    state.gamerules.insert("mob_drops".into(), "true".into());
    let cause = crate::entity_loot::death_cause(&state, slot, None, DamageKind::Void);
    crate::entity_loot::commit_death(&mut state, slot, cause, &mut drops);
    assert!(drops.is_empty(), "a handled death cannot emit loot later");
    let type_id = summons::type_id(&state, "minecraft:wither").unwrap();
    let wither = state.spawn_mob(type_id, [0.0, 201.0, 0.0], 300.0);
    let slot = state.entities.slot_of(wither).unwrap();
    state.entities.body_mut(slot).alive = false;
    let cause = crate::entity_loot::death_cause(&state, slot, None, DamageKind::Void);
    crate::entity_loot::commit_death(&mut state, slot, cause, &mut drops);
    assert_eq!(drops.len(), 1);
    let stack = state
        .entities
        .row(state.entities.slot_of(drops[0]).unwrap())
        .item
        .as_ref()
        .unwrap();
    assert_eq!(stack.item_id, item_id(&state, "nether_star").unwrap());
    assert_eq!(stack.count, 1);
    let image = EntityStateImage::from_row(state.entities.body(slot), state.entities.row(slot));
    assert!(
        serde_json::from_slice::<EntityStateImage>(&serde_json::to_vec(&image).unwrap())
            .unwrap()
            .loot
            .death_handled
    );
    let cause = crate::entity_loot::death_cause(&state, slot, None, DamageKind::Void);
    crate::entity_loot::commit_death(&mut state, slot, cause, &mut drops);
    assert_eq!(drops.len(), 1);
}

#[test]
fn pillager_target_loss_clears_charged_payload_and_charging_metadata() {
    let (mut state, mob, player, pos) = fixture("pillager", 8.0);
    for tick in [0, 4, 29] {
        advance(&mut state, mob, tick, GoalKind::Crossbow, player);
    }
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(
        state.entities.row(slot).appearance.equipment[0]
            .as_ref()
            .unwrap()
            .extended()
            .charged_projectiles
            .len(),
        1
    );
    wall(&mut state, pos);
    advance(&mut state, mob, 30, GoalKind::Crossbow, player);
    assert_eq!(
        state.entities.row(slot).special_mob.as_deref().cloned(),
        Some(Retained::Crossbow(None))
    );
    assert!(
        state.entities.row(slot).appearance.equipment[0]
            .as_ref()
            .unwrap()
            .extended()
            .charged_projectiles
            .is_empty()
    );
    assert!(
        state
            .entities
            .row(slot)
            .appearance
            .extra
            .contains(&(17, EntityDataValue::Boolean(false)))
    );
}

#[test]
fn no_ai_keeps_evoker_runtime_with_or_without_another_active_special() {
    for companion in [false, true] {
        let (mut state, evoker, player, pos) = fixture("evoker", 8.0);
        state
            .gamerules
            .insert("mob_spawning".into(), "false".into());
        advance(&mut state, evoker, 0, GoalKind::Fangs, player);
        let slot = state.entities.slot_of(evoker).unwrap();
        let old = state.hostile.native.attacks[&evoker].clone();
        state.entities.row_mut(slot).controls.no_ai = true;
        if companion {
            let type_id = summons::type_id(&state, "minecraft:blaze").unwrap();
            state.spawn_mob(type_id, [pos[0] + 40.0, pos[1], pos[2]], 20.0);
        }
        for _ in 0..32 {
            crate::tick::run_tick(&mut state, &[]);
        }
        let current = &state.hostile.native.attacks[&evoker];
        assert_eq!(current.stage, old.stage);
        assert_eq!(current.deadline, old.deadline);
        assert_eq!(current.target, old.target);
        assert_eq!(current.summon_at, old.summon_at);
        assert_eq!(current.drink, old.drink);
        assert!(!state.entities.slots().any(|slot| matches!(
            state.entities.row(slot).special_mob.as_deref().cloned(),
            Some(Retained::Fangs { .. })
        )));
        state.entities.body_mut(slot).alive = false;
        crate::tick::run_tick(&mut state, &[]);
        assert!(!state.hostile.native.attacks.contains_key(&evoker));
    }
}

#[test]
fn no_ai_keeps_witch_drink_and_retained_checkpoint_state_without_population_dependence() {
    for companion in [false, true] {
        let (mut state, witch, player, pos) = fixture("witch", 9.0);
        state
            .gamerules
            .insert("mob_spawning".into(), "false".into());
        let slot = state.entities.slot_of(witch).unwrap();
        state.entities.row_mut(slot).health = 18.0;
        advance(&mut state, witch, 0, GoalKind::Alchemist, player);
        state.entities.row_mut(slot).controls.no_ai = true;
        let old = state.hostile.native.attacks[&witch].clone();
        let retained = state.entities.row(slot).special_mob.as_deref().cloned();
        let speed = state.entities.row(slot).speed;
        if companion {
            let type_id = summons::type_id(&state, "minecraft:blaze").unwrap();
            state.spawn_mob(type_id, [pos[0] + 40.0, pos[1], pos[2]], 20.0);
        }
        for _ in 0..32 {
            crate::tick::run_tick(&mut state, &[]);
        }
        assert_eq!(state.entities.row(slot).health, 18.0);
        assert_eq!(state.entities.row(slot).speed, speed);
        assert_eq!(
            state.entities.row(slot).special_mob.as_deref().cloned(),
            retained
        );
        assert_eq!(state.hostile.native.attacks[&witch].drink, old.drink);
        assert_eq!(state.hostile.native.attacks[&witch].deadline, old.deadline);
        assert!(missiles(&state, ProjectileKind::Potion).is_empty());
        let (_, regions, _) = crate::core::checkpoint_data_for(&state, state.tick);
        let native = regions
            .values()
            .flat_map(|chunk| &chunk.entities)
            .find(|entity| entity.id == "minecraft:witch")
            .unwrap();
        let image = EntityStateImage::from_record(&state.registry, native).unwrap();
        assert!(image.controls.no_ai);
        assert_eq!(image.special_mob, retained);
        assert_eq!(state.hostile.native.attacks[&witch].drink, old.drink);
        assert_eq!(state.hostile.native.attacks[&witch].deadline, old.deadline);
        restore(&mut state, witch);
        let restored = state.entities.slot_of(witch).unwrap();
        assert!(state.entities.row(restored).controls.no_ai);
        assert_eq!(
            state.entities.row(restored).special_mob.as_deref().cloned(),
            retained
        );
    }
}

#[test]
fn raider_names_resolve_by_protocol_id_lookup() {
    for short in [
        "vindicator",
        "ravager",
        "evoker",
        "pillager",
        "witch",
        "illusioner",
    ] {
        let full = format!("minecraft:{short}");
        let type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name(&full)
            .unwrap()
            .protocol_id;
        assert!(raiders::owns(crate::entities::EntityKind::Mob { type_id }));
    }
    for short in ["cow", "zombie"] {
        let type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name(&format!(
            "minecraft:{short}"
        ))
        .unwrap()
        .protocol_id;
        assert!(!raiders::owns(crate::entities::EntityKind::Mob { type_id }));
    }
    assert!(!raiders::owns(crate::entities::EntityKind::Player {
        connection: crate::boundary::ConnectionId(1),
    }));
    for type_id in [mc_data::entity_types::ENTITY_TYPE_COUNT as u32, u32::MAX] {
        assert!(!raiders::owns(crate::entities::EntityKind::Mob { type_id }));
    }
}
