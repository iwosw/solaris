use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use mc_data::Identifier;
use mc_data::item_components::{CustomItemDefinition, ItemFacts};
use mc_data::item_profiles::{AttackShape, DamageKind, ItemConsumption, ProjectileProfile};
use std::sync::Arc;

fn id(value: &str) -> Identifier {
    Identifier::parse(value).unwrap()
}
fn melee() -> AttackProfile {
    AttackProfile {
        mode: AttackMode::Melee,
        damage_kind: DamageKind::Cut,
        base_damage_hp: 5.0,
        windup_ticks: 6,
        active_ticks: 2,
        recovery_ticks: 10,
        min_range_blocks: 0.0,
        reach_blocks: 1.9,
        stamina_cost: 12.0,
        shape: AttackShape::Cone {
            half_angle_degrees: 45.0,
        },
        projectile: None,
        consumption: None,
        brace: None,
        charge: None,
        two_handed: false,
        wear: 1,
    }
}
fn bow() -> AttackProfile {
    AttackProfile {
        mode: AttackMode::Projectile,
        damage_kind: DamageKind::Pierce,
        base_damage_hp: 5.0,
        windup_ticks: 16,
        active_ticks: 1,
        recovery_ticks: 20,
        min_range_blocks: 4.0,
        reach_blocks: 24.0,
        stamina_cost: 18.0,
        shape: AttackShape::Thrust {
            radius_blocks: 0.12,
        },
        projectile: Some(ProjectileProfile {
            carrier: id("minecraft:arrow"),
            speed_blocks_per_tick: 1.5,
            gravity_blocks_per_tick_squared: 0.035,
            collision_radius_blocks: 0.12,
            lifetime_ticks: 160,
            recoverable: true,
            impulse: 0.18,
        }),
        consumption: Some(ItemConsumption {
            item: id("minecraft:arrow"),
            count: 1,
        }),
        brace: None,
        charge: None,
        two_handed: true,
        wear: 1,
    }
}
fn profile(attack: AttackProfile) -> PhysicalItemProfile {
    PhysicalItemProfile {
        attacks: vec![attack],
        armor: None,
        shield: None,
        stamina_capacity: 100.0,
        stamina_recovery_per_tick: 0.5,
    }
}
fn fixture(attack: AttackProfile) -> (TickState, EntitySlot, EntitySlot) {
    let mut registry = CoreRegistry::load(None).unwrap();
    registry.item_facts = registry
        .item_facts
        .clone()
        .with_custom_items(
            [CustomItemDefinition {
                id: id("example:weapon"),
                carrier: id("minecraft:paper"),
                name: "Authored weapon".into(),
                crafting_ingredient: None,
                facts: ItemFacts {
                    max_stack_size: Some(1),
                    max_damage: Some(64),
                    physical_profile: Some(Box::new(profile(attack))),
                    ..ItemFacts::default()
                },
            }],
            &registry.items,
        )
        .unwrap();
    let registry = Arc::new(registry);
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().unwrap();
    let world = ChunkWorld::new(
        42,
        Arc::new(terrain),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: Uuid::from_u128(1),
            name: "attacker".into(),
            permission_level: 0,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let actor_id = state.connections.entity_of(ConnectionId(1)).unwrap();
    let actor = state.entities.slot_of(actor_id).unwrap();
    let at = state.entities.pos(actor);
    let origin = [at[0].floor() + 0.5, 201.0, at[2].floor() + 0.5];
    for x in -2..=2 {
        for z in -2..=26 {
            state.world.set_block(
                origin[0] as i32 + x,
                200,
                origin[2] as i32 + z,
                state.registry.states.stone,
            );
            for y in 201..=204 {
                state.world.set_block(
                    origin[0] as i32 + x,
                    y,
                    origin[2] as i32 + z,
                    state.registry.states.air,
                );
            }
        }
    }
    state.world.fold_overlay();
    state.entities.set_pos(actor, origin);
    state.entities.set_rot(actor, 0.0, 0.0);
    state.entities.row_mut(actor).protected_until = 0;
    let carrier = state.registry.items.id_of(&id("minecraft:paper")).unwrap();
    let mut held = ItemStack::new(carrier, 1);
    held.item_model = Some(std::sync::Arc::new(id("example:weapon")));
    state.players.get_mut(&actor_id).unwrap().inventory.slots[0] = held;
    let cow = state
        .registry
        .entity_types
        .id_of(&id("minecraft:cow"))
        .unwrap();
    let victim = state.spawn_mob(cow, [origin[0], origin[1], origin[2] + 1.5], 20.0);
    let target = state.entities.slot_of(victim).unwrap();
    state.entities.row_mut(target).controls.no_ai = true;
    state.entities.row_mut(target).speed = 0.0;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    (state, actor, target)
}

#[test]
fn absolute_phase_claims_once_and_never_catches_up_a_missed_window() {
    let profile = melee();
    let mut phase = PhysicalAttackPhase::new(&profile, 100);
    assert_eq!(phase.ready_tick, 118);
    assert!(!phase.claim_active(&profile, 105));
    assert!(phase.claim_active(&profile, 106));
    assert!(!phase.claim_active(&profile, 107));
    let mut missed = PhysicalAttackPhase::new(&profile, 100);
    assert!(!missed.claim_active(&profile, 108));
    assert!(!missed.claim_active(&profile, 118));
}

#[test]
fn stamina_uses_elapsed_absolute_ticks_and_exact_milli_rounding() {
    let stamina = PhysicalStamina {
        available_milli: 1_000,
        last_tick: 10,
    };
    let spent = stamina.reserve(100.0, 0.5, 20, 5.9996).unwrap();
    assert_eq!(spent.available_milli, 0);
    assert_eq!(spent.last_tick, 20);
    assert!(spent.reserve(100.0, 0.5, 20, 0.001).is_none());
    assert_eq!(spent.available(100.0, 0.5, u64::MAX), 100_000);
}

#[test]
fn authored_melee_waits_for_active_phase_and_charges_exactly_once() {
    let (mut state, actor, target) = fixture(melee());
    let actor_id = state.entities.body(actor).id;
    let start = state.tick;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Accepted
    );
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Rejected
    );
    state.tick = start + 5;
    assert!(commit_attack(&mut state, actor_id, start).is_none());
    assert_eq!(state.players[&actor_id].inventory.slots[0].damage, None);
    state.tick = start + 6;
    let (_, hit, _) = commit_attack(&mut state, actor_id, start).unwrap();
    assert_eq!(hit.damage_hp, 5.0);
    assert_eq!(hit.kind, DamageKind::Cut);
    assert_eq!(state.players[&actor_id].inventory.slots[0].damage, Some(1));
    assert_eq!(stamina(&state, actor.0 as usize, 100.0, 0.5), 88.0);
    assert!(commit_attack(&mut state, actor_id, start).is_none());
    state.tick = start + 18;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Accepted
    );
}

#[test]
fn resident_inactive_npc_attack_does_not_dispatch_an_effect() {
    let (mut state, player, npc) = fixture(melee());
    let player_id = state.entities.body(player).id;
    let npc_id = state.entities.body(npc).id;
    let pos = state.entities.pos(npc);
    let chunk = crate::boundary::ChunkPos {
        x: (pos[0].floor() as i32).div_euclid(16),
        z: (pos[2].floor() as i32).div_euclid(16),
    };
    let action = Pending {
        actor: state.entities.row(npc).uuid,
        target: Some(state.entities.row(player).uuid),
        held_slot: 0,
        held: state.players[&player_id].inventory.slots[0].clone(),
        attack_index: 0,
        phase: PhysicalAttackPhase::new(&melee(), state.tick),
        order: None,
    };
    state.tick = action.phase.active_tick;
    save_pending(&mut state, &action);
    state
        .world
        .set_active_chunks(Default::default(), state.tick);
    let mut intents = Vec::new();
    advance(&mut state, &mut intents);
    assert!(state.world.has_chunk(chunk.x, chunk.z));
    assert!(intents.is_empty());
    state
        .world
        .set_active_chunks([chunk].into_iter().collect(), state.tick);
    advance(&mut state, &mut intents);
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].actor, npc_id);
}

#[test]
fn native_tick_applies_authored_damage_not_paper_carrier_melee() {
    let (mut state, actor, target) = fixture(melee());
    let target_id = state.entities.body(target).id;
    let start = state.tick;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Accepted
    );
    state.tick = start + 6;
    run_tick(&mut state, &[]);
    let slot = state.entities.slot_of(target_id).unwrap();
    assert_eq!(state.entities.row(slot).health, 15.0);
}

#[test]
fn current_pose_and_collision_are_revalidated_at_impact() {
    let (mut state, actor, target) = fixture(melee());
    let actor_id = state.entities.body(actor).id;
    let start = state.tick;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Accepted
    );
    let mut at = state.entities.pos(target);
    at[2] += 8.0;
    state.entities.set_pos(target, at);
    state.tick = start + 6;
    assert!(commit_attack(&mut state, actor_id, start).is_none());
    assert_eq!(state.players[&actor_id].inventory.slots[0].damage, None);
    at[2] -= 8.0;
    state.entities.set_pos(target, at);
    assert!(
        commit_attack(&mut state, actor_id, start).is_none(),
        "missed effect is still claimed"
    );
}

#[test]
fn exact_registry_wall_blocks_shape_even_inside_authored_reach() {
    let (mut state, actor, target) = fixture(melee());
    let origin = state.entities.pos(actor);
    state.world.set_block(
        origin[0].floor() as i32,
        201,
        origin[2].floor() as i32 + 1,
        state.registry.states.stone,
    );
    state.world.set_block(
        origin[0].floor() as i32,
        202,
        origin[2].floor() as i32 + 1,
        state.registry.states.stone,
    );
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Rejected
    );
}

#[test]
fn thrust_min_range_and_cone_facing_use_real_body_footprint() {
    let mut attack = melee();
    attack.min_range_blocks = 1.25;
    attack.reach_blocks = 3.2;
    attack.shape = AttackShape::Thrust {
        radius_blocks: 0.18,
    };
    let (mut state, actor, target) = fixture(attack.clone());
    let origin = state.entities.pos(actor);
    state
        .entities
        .set_pos(target, [origin[0], origin[1], origin[2] + 0.5]);
    assert!(impact_point(&state, actor, target, &attack).is_none());
    state
        .entities
        .set_pos(target, [origin[0], origin[1], origin[2] + 3.4]);
    assert!(
        impact_point(&state, actor, target, &attack).is_some(),
        "body edge reaches even when center does not"
    );
    state
        .entities
        .set_pos(target, [origin[0] + 1.0, origin[1], origin[2] + 2.0]);
    assert!(impact_point(&state, actor, target, &attack).is_none());
    state
        .entities
        .set_pos(target, [origin[0], origin[1], origin[2] - 1.5]);
    assert!(impact_point(&state, actor, target, &melee()).is_none());
}

#[test]
fn held_identity_and_two_handed_offhand_gate_costs_again_at_merge() {
    let mut attack = melee();
    attack.two_handed = true;
    let (mut state, actor, target) = fixture(attack);
    let actor_id = state.entities.body(actor).id;
    let start = state.tick;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Accepted
    );
    let item = state.registry.items.id_of(&id("minecraft:arrow")).unwrap();
    state.players.get_mut(&actor_id).unwrap().inventory.slots[crate::inventory::OFF_HAND] =
        ItemStack::new(item, 1);
    state.tick = start + 6;
    assert!(commit_attack(&mut state, actor_id, start).is_none());
    assert_eq!(state.players[&actor_id].inventory.slots[0].damage, None);
}

#[test]
fn exact_wear_breaks_custom_weapon_without_vanilla_unbreaking_reduction() {
    let (mut state, actor, target) = fixture(melee());
    let actor_id = state.entities.body(actor).id;
    let player = state.players.get_mut(&actor_id).unwrap();
    player.inventory.slots[0].damage = Some(63);
    player.inventory.slots[0] = player.inventory.slots[0]
        .clone()
        .with_enchantment(id("minecraft:unbreaking"), 3);
    let start = state.tick;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Accepted
    );
    state.tick = start + 6;
    assert!(commit_attack(&mut state, actor_id, start).is_some());
    assert!(state.players[&actor_id].inventory.slots[0].is_empty());
}

#[test]
fn native_bow_launch_debits_real_ammunition_and_retains_full_components() {
    let (mut state, actor, _) = fixture(bow());
    let actor_id = state.entities.body(actor).id;
    let arrow_id = state.registry.items.id_of(&id("minecraft:arrow")).unwrap();
    let arrows = ItemStack::new(arrow_id, 2).with_enchantment(id("minecraft:power"), 2);
    state.players.get_mut(&actor_id).unwrap().inventory.slots[9] = arrows.clone();
    let start = state.tick;
    assert_eq!(
        try_use(&mut state, actor, Hand::Main),
        AttackAdmission::Accepted
    );
    state.tick = start + 16;
    assert!(
        commit_attack(&mut state, actor_id, start).is_none(),
        "launch never fabricates immediate damage"
    );
    let slot = state
        .entities
        .slots()
        .find(|s| state.entities.row(*s).projectile.is_some())
        .unwrap();
    let record = state.entities.row(slot).projectile.as_ref().unwrap();
    assert_eq!(record.stack.enchantments, arrows.enchantments);
    assert!(record.pickup);
    assert_eq!(state.players[&actor_id].inventory.slots[9].count, 1);
    assert_eq!(state.players[&actor_id].inventory.slots[0].damage, Some(1));
    let runtime = projectile_profile(&state, slot).unwrap();
    assert_eq!(runtime.gravity, 0.035);
    assert_eq!(runtime.collision_radius, 0.12);
    assert_eq!(runtime.lifetime_ticks, 160);
    assert!((state.entities.vel(slot)[2] / 20.0 - 1.5).abs() < 1.0e-9);
    assert!(commit_attack(&mut state, actor_id, start).is_none());
    assert_eq!(state.players[&actor_id].inventory.slots[9].count, 1);
}

#[test]
fn insufficient_ammo_refuses_without_stamina_or_wear() {
    let (mut state, actor, _) = fixture(bow());
    let actor_id = state.entities.body(actor).id;
    assert_eq!(
        try_use(&mut state, actor, Hand::Main),
        AttackAdmission::Rejected
    );
    assert_eq!(state.players[&actor_id].inventory.slots[0].damage, None);
    assert_eq!(stamina(&state, actor.0 as usize, 100.0, 0.5), 100.0);
}

#[test]
fn replay_image_retains_claim_and_stamina_without_idle_writes() {
    let (mut state, actor, target) = fixture(melee());
    let actor_id = state.entities.body(actor).id;
    let start = state.tick;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Accepted
    );
    state.tick = start + 6;
    assert!(commit_attack(&mut state, actor_id, start).is_some());
    let image = serde_json::to_vec(&state.plugins).unwrap();
    state.plugins = serde_json::from_slice(&image).unwrap();
    assert!(!state.plugins.dirty);
    assert!(commit_attack(&mut state, actor_id, start).is_none());
    let mut intents = Vec::new();
    advance(&mut state, &mut intents);
    assert!(intents.is_empty());
    assert!(!state.plugins.dirty);
    state.tick += 4;
    assert_eq!(stamina(&state, actor.0 as usize, 100.0, 0.5), 90.0);
    assert!(!state.plugins.dirty);
}

#[test]
fn skipped_active_window_never_emits_a_late_hit() {
    let (mut state, actor, target) = fixture(melee());
    let start = state.tick;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Accepted
    );
    state.tick = start + 8;
    let mut intents = Vec::new();
    advance(&mut state, &mut intents);
    assert!(intents.is_empty());
    state.tick = start + 18;
    advance(&mut state, &mut intents);
    assert!(pending(&state, state.entities.row(actor).uuid).is_none());
}

fn launch_custom_bow(state: &mut TickState, actor: EntitySlot) -> EntityId {
    let actor_id = state.entities.body(actor).id;
    let arrow_id = state.registry.items.id_of(&id("minecraft:arrow")).unwrap();
    state.players.get_mut(&actor_id).unwrap().inventory.slots[9] = ItemStack::new(arrow_id, 1);
    let start = state.tick;
    assert_eq!(try_use(state, actor, Hand::Main), AttackAdmission::Accepted);
    state.tick = start + 16;
    assert!(commit_attack(state, actor_id, start).is_none());
    let slot = state
        .entities
        .slots()
        .find(|slot| state.entities.row(*slot).projectile.is_some())
        .unwrap();
    assert!(matches!(
        state.entities.row(slot).projectile.as_ref().unwrap().data,
        crate::projectiles::ProjectileData::Physical { .. }
    ));
    assert!(projectile_profile(state, slot).is_some());
    state.entities.body(slot).id
}

#[test]
fn real_flight_preserves_speed_applies_gravity_and_merges_exact_authored_hit_once() {
    let (mut state, actor, target) = fixture(bow());
    let origin = state.entities.pos(actor);
    let target_id = state.entities.body(target).id;
    state
        .entities
        .set_pos(target, [origin[0], origin[1], origin[2] + 6.0]);
    let arrow = launch_custom_bow(&mut state, actor);
    run_tick(&mut state, &[]);
    let slot = state.entities.slot_of(arrow).unwrap();
    assert!((state.entities.vel(slot)[2] / 20.0 - 1.5).abs() < 1.0e-9);
    assert!((state.entities.vel(slot)[1] / 20.0 + 0.035).abs() < 1.0e-9);
    run_tick(&mut state, &[]);
    let slot = state.entities.slot_of(arrow).unwrap();
    assert!((state.entities.vel(slot)[2] / 20.0 - 1.5).abs() < 1.0e-9);
    assert!((state.entities.vel(slot)[1] / 20.0 + 0.070).abs() < 1.0e-9);
    assert_eq!(state.entities.row(target).health, 20.0);
    run_tick(&mut state, &[]);
    run_tick(&mut state, &[]);
    assert!(state.entities.slot_of(arrow).is_none());
    let slot = state.entities.slot_of(target_id).unwrap();
    assert_eq!(
        state.entities.row(slot).health,
        15.0,
        "not vanilla ceil(speed * damage)"
    );
    run_tick(&mut state, &[]);
    let slot = state.entities.slot_of(target_id).unwrap();
    assert_eq!(state.entities.row(slot).health, 15.0);
}

#[test]
fn real_flight_contact_inside_min_range_never_falls_back_to_vanilla_damage() {
    let (mut state, actor, target) = fixture(bow());
    let target_id = state.entities.body(target).id;
    let arrow = launch_custom_bow(&mut state, actor);
    let slot = state.entities.slot_of(arrow).unwrap();
    let at = state.entities.pos(slot);
    let body = state.entities.pos(target);
    assert!(
        (body[1]..=body[1] + state.entities.body(target).height).contains(&at[1]),
        "fixture places the actual native body across the launch path"
    );
    run_tick(&mut state, &[]);
    assert!(state.entities.slot_of(arrow).is_none());
    let slot = state.entities.slot_of(target_id).unwrap();
    assert_eq!(state.entities.row(slot).health, 20.0);
}

#[test]
fn authored_contact_revalidates_the_current_native_sweep_without_a_radius_halo() {
    let mut attack = bow();
    attack.min_range_blocks = 0.0;
    let (mut state, actor, target) = fixture(attack);
    let arrow = launch_custom_bow(&mut state, actor);
    let slot = state.entities.slot_of(arrow).unwrap();
    let origin = state.entities.pos(slot);
    let step = state.entities.vel(slot).map(|value| value / 20.0);
    let pos = state.entities.pos(target);
    let row = state.entities.body(target);
    let entry = crate::projectiles::flight::swept_sphere_enters(
        origin,
        step,
        [pos[0] - row.half_width, pos[1], pos[2] - row.half_width],
        [
            pos[0] + row.half_width,
            pos[1] + row.height,
            pos[2] + row.half_width,
        ],
        projectile_profile(&state, slot).unwrap().collision_radius,
    )
    .expect("real fixture body intersects the authored ballistic step");
    let point = std::array::from_fn(|axis| origin[axis] + step[axis] * entry);
    let impact = projectile_hit(&state, slot, target, point).unwrap();
    assert_eq!(impact.damage_hp, 5.0);
    assert_eq!(impact.point, point);
    state
        .entities
        .set_pos(target, [pos[0] + 0.65, pos[1], pos[2]]);
    assert!(
        projectile_hit(&state, slot, target, point).is_none(),
        "a retained old contact never authenticates a moved native body"
    );
    state.entities.set_pos(target, pos);
    state.entities.row_mut(target).health = 0.0;
    assert!(
        projectile_hit(&state, slot, target, point).is_none(),
        "geometry never bypasses live target authorization"
    );
}

#[test]
fn real_flight_honors_authored_airborne_lifetime() {
    let mut profile = bow();
    profile.projectile.as_mut().unwrap().lifetime_ticks = 2;
    let (mut state, actor, target) = fixture(profile);
    let mut at = state.entities.pos(target);
    at[0] += 2.0;
    state.entities.set_pos(target, at);
    let arrow = launch_custom_bow(&mut state, actor);
    run_tick(&mut state, &[]);
    assert!(state.entities.slot_of(arrow).is_some());
    run_tick(&mut state, &[]);
    assert!(state.entities.slot_of(arrow).is_some());
    run_tick(&mut state, &[]);
    assert!(state.entities.slot_of(arrow).is_none());
}

#[test]
fn real_stuck_arrow_pickup_obeys_authored_recoverability() {
    for recoverable in [false, true] {
        let mut profile = bow();
        profile.projectile.as_mut().unwrap().recoverable = recoverable;
        let (mut state, actor, target) = fixture(profile);
        let actor_id = state.entities.body(actor).id;
        let origin = state.entities.pos(actor);
        state
            .entities
            .set_pos(target, [origin[0] + 2.0, origin[1], origin[2] + 6.0]);
        state.world.set_block(
            origin[0].floor() as i32,
            201,
            origin[2].floor() as i32 + 6,
            state.registry.states.stone,
        );
        state.world.set_block(
            origin[0].floor() as i32,
            202,
            origin[2].floor() as i32 + 6,
            state.registry.states.stone,
        );
        let arrow = launch_custom_bow(&mut state, actor);
        for _ in 0..4 {
            run_tick(&mut state, &[]);
        }
        let slot = state.entities.slot_of(arrow).unwrap();
        assert!(state.entities.row(slot).projectile.as_ref().unwrap().stuck);
        let at = state.entities.pos(slot);
        state.entities.set_pos(actor, [at[0], at[1] - 0.7, at[2]]);
        for _ in 0..4 {
            run_tick(&mut state, &[]);
        }
        assert_eq!(state.entities.slot_of(arrow).is_none(), recoverable);
        let arrow_id = state.registry.items.id_of(&id("minecraft:arrow")).unwrap();
        let count: i32 = state.players[&actor_id]
            .inventory
            .slots
            .iter()
            .filter(|s| s.item_id == arrow_id && !s.is_empty())
            .map(|s| s.count)
            .sum();
        assert_eq!(count, i32::from(recoverable));
    }
}

#[test]
fn held_player_reserve_is_rechecked_after_windup_before_ammunition_debit() {
    use mc_script::{
        ScriptInventoryEndpoint, ScriptInventoryMaterial, ScriptInventoryResourcePlan,
        ScriptInventoryWorkPortion, ScriptOperation, ScriptOperationRequest,
        ScriptOwnedInventoryOperation,
    };
    let (mut state, actor, _) = fixture(bow());
    let actor_id = state.entities.body(actor).id;
    let arrow_id = state.registry.items.id_of(&id("minecraft:arrow")).unwrap();
    state.players.get_mut(&actor_id).unwrap().inventory.slots[9] = ItemStack::new(arrow_id, 1);
    let start = state.tick;
    assert_eq!(
        try_use(&mut state, actor, Hand::Main),
        AttackAdmission::Accepted
    );
    let endpoint = ScriptInventoryEndpoint::PlayerInventory { player_id: 1 };
    let image = crate::plugin_inventories::load(&state, "example", &endpoint).unwrap();
    let request = ScriptOperationRequest::try_new(
        "reserve-arrow",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Reserve {
                operation_id: "reserve-arrow".into(),
                endpoint,
                resource_plan: ScriptInventoryResourcePlan::new(vec![
                    ScriptInventoryWorkPortion::new(
                        1,
                        vec![ScriptInventoryMaterial::new("minecraft:arrow".into(), 1)],
                    ),
                ]),
                expected_revision: image.fence,
            },
        },
    )
    .unwrap();
    assert_eq!(
        crate::plugin_inventories::execute(&mut state, "example", &request).failure(),
        None
    );
    state.tick = start + 16;
    assert!(commit_attack(&mut state, actor_id, start).is_none());
    assert!(
        !state
            .entities
            .slots()
            .any(|slot| state.entities.row(slot).projectile.is_some())
    );
    assert_eq!(state.players[&actor_id].inventory.slots[9].count, 1);
    assert_eq!(state.players[&actor_id].inventory.slots[0].damage, None);
    assert_eq!(stamina(&state, actor.0 as usize, 100.0, 0.5), 100.0);
}

#[test]
fn recovery_rejects_malformed_native_projectile_instead_of_selecting_vanilla() {
    let (mut state, actor, target) = fixture(bow());
    let mut at = state.entities.pos(target);
    at[0] += 2.0;
    state.entities.set_pos(target, at);
    let projectile = launch_custom_bow(&mut state, actor);
    assert!(validate_restore(&state).is_ok());
    let uuid = state
        .entities
        .row(state.entities.slot_of(projectile).unwrap())
        .uuid;
    let key = NativeKey::new(b"projectile/", uuid);
    let mut value = state
        .plugins
        .get_domain(NATIVE, key.as_str())
        .unwrap()
        .clone();
    value["profile"]["projectile"]["gravity_blocks_per_tick_squared"] = serde_json::json!(0.001);
    state.plugins.set_domain(NATIVE, key.as_str(), value);
    assert!(
        validate_restore(&state).is_err(),
        "replayed mechanics must equal canonical authored definition"
    );
}

#[test]
fn recovery_rejects_future_stamina_and_inconsistent_pending_absolute_phase() {
    let (mut state, actor, target) = fixture(melee());
    let uuid = state.entities.row(actor).uuid;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Accepted
    );
    assert!(validate_restore(&state).is_ok());
    let mut action = pending(&state, uuid).unwrap();
    action.phase.ready_tick += 1;
    save_pending(&mut state, &action);
    assert!(validate_restore(&state).is_err());
    remove_pending(&mut state, uuid);
    state.plugins.set_domain(
        NATIVE,
        NativeKey::new(b"stamina/", uuid).as_str(),
        serde_json::to_value(PhysicalStamina {
            available_milli: 1,
            last_tick: state.tick + 1,
        })
        .unwrap(),
    );
    assert!(validate_restore(&state).is_err());
}

#[test]
fn insufficient_stamina_and_spectator_actor_refuse_native_action() {
    let (mut state, actor, target) = fixture(melee());
    let uuid = state.entities.row(actor).uuid;
    state.plugins.set_domain(
        NATIVE,
        NativeKey::new(b"stamina/", uuid).as_str(),
        serde_json::to_value(PhysicalStamina {
            available_milli: 11_999,
            last_tick: state.tick,
        })
        .unwrap(),
    );
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Rejected
    );
    state
        .plugins
        .delete_domain(NATIVE, NativeKey::new(b"stamina/", uuid).as_str());
    let actor_id = state.entities.body(actor).id;
    state.players.get_mut(&actor_id).unwrap().game_mode = crate::commands::GameMode::Spectator;
    assert_eq!(
        try_attack(&mut state, actor, target),
        AttackAdmission::Rejected
    );
}

#[test]
fn marked_native_projectile_with_missing_metadata_refuses_recovery() {
    let (mut state, actor, target) = fixture(bow());
    let mut at = state.entities.pos(target);
    at[0] += 2.0;
    state.entities.set_pos(target, at);
    let projectile = launch_custom_bow(&mut state, actor);
    let slot = state.entities.slot_of(projectile).unwrap();
    assert!(matches!(
        state.entities.row(slot).projectile.as_ref().unwrap().data,
        crate::projectiles::ProjectileData::Physical { .. }
    ));
    let uuid = state.entities.row(slot).uuid;
    drop_projectile(&mut state, uuid);
    assert!(
        validate_restore(&state).is_err(),
        "marker must never lose native authored authority"
    );
}

#[test]
fn actual_authored_projectile_radius_hits_real_graze_but_not_vanilla_halo() {
    for (lateral, expected_hp) in [(0.55, 15.0), (0.65, 20.0)] {
        let (mut state, actor, target) = fixture(bow());
        let origin = state.entities.pos(actor);
        let target_id = state.entities.body(target).id;
        state
            .entities
            .set_pos(target, [origin[0] + lateral, origin[1], origin[2] + 6.0]);
        let arrow = launch_custom_bow(&mut state, actor);
        for _ in 0..5 {
            run_tick(&mut state, &[]);
        }
        let target = state.entities.slot_of(target_id).unwrap();
        assert_eq!(
            state.entities.row(target).health,
            expected_hp,
            "collision uses authored swept sphere radius"
        );
        assert_eq!(state.entities.slot_of(arrow).is_none(), expected_hp == 15.0);
    }
}

#[test]
fn unsupported_authored_projectile_carrier_is_explicitly_refused() {
    let mut attack = bow();
    attack.projectile.as_mut().unwrap().carrier = id("minecraft:snowball");
    assert!(validate_profile(&profile(attack)).is_err());
}

#[test]
fn sparse_resource_costs_preserve_unrelated_accumulated_inventory_changes() {
    let (mut state, actor, _) = fixture(melee());
    let actor_id = state.entities.body(actor).id;
    let held = state.players[&actor_id].inventory.slots[0].clone();
    let costs = plan_costs(&state, actor, &melee(), 0, &held, true).unwrap();
    assert!(costs.debits.is_empty());
    let item = state.registry.items.id_of(&id("minecraft:arrow")).unwrap();
    let unrelated = ItemStack::new(item, 7).with_enchantment(id("minecraft:power"), 3);
    state.players.get_mut(&actor_id).unwrap().inventory.slots[9] = unrelated.clone();
    apply_costs(&mut state, actor, costs);
    assert_eq!(state.players[&actor_id].inventory.slots[9], unrelated);
    assert_eq!(state.players[&actor_id].inventory.slots[0].damage, Some(1));
}

#[test]
fn ammunition_free_projectile_does_not_synthesize_recoverable_inventory() {
    let mut attack = bow();
    attack.consumption = None;
    let (mut state, actor, _) = fixture(attack);
    let actor_id = state.entities.body(actor).id;
    let start = state.tick;
    assert_eq!(
        try_use(&mut state, actor, Hand::Main),
        AttackAdmission::Accepted
    );
    state.tick = start + 16;
    commit_attack(&mut state, actor_id, start);
    let slot = state
        .entities
        .slots()
        .find(|slot| state.entities.row(*slot).projectile.is_some())
        .unwrap();
    let projectile = state.entities.row(slot).projectile.as_ref().unwrap();
    assert!(projectile.stack.is_empty());
    assert!(!projectile.pickup);
    assert!(validate_restore(&state).is_ok());
}

#[test]
fn actual_authored_longsword_and_spear_execute_native_phase_damage_and_costs() {
    let mut longsword = melee();
    longsword.base_damage_hp = 7.0;
    longsword.windup_ticks = 10;
    longsword.active_ticks = 3;
    longsword.recovery_ticks = 16;
    longsword.reach_blocks = 2.8;
    longsword.stamina_cost = 22.0;
    longsword.shape = AttackShape::Cone {
        half_angle_degrees: 55.0,
    };
    longsword.two_handed = true;
    let mut spear = melee();
    spear.damage_kind = DamageKind::Pierce;
    spear.windup_ticks = 8;
    spear.recovery_ticks = 14;
    spear.min_range_blocks = 1.25;
    spear.reach_blocks = 3.2;
    spear.stamina_cost = 16.0;
    spear.shape = AttackShape::Thrust {
        radius_blocks: 0.18,
    };
    spear.two_handed = true;
    for attack in [longsword, spear] {
        let (mut state, actor, target) = fixture(attack.clone());
        let actor_id = state.entities.body(actor).id;
        let target_id = state.entities.body(target).id;
        let origin = state.entities.pos(actor);
        state
            .entities
            .set_pos(target, [origin[0], origin[1], origin[2] + 2.5]);
        let start = state.tick;
        assert_eq!(
            try_attack(&mut state, actor, target),
            AttackAdmission::Accepted
        );
        state.tick = start + u64::from(attack.windup_ticks);
        run_tick(&mut state, &[]);
        let target = state.entities.slot_of(target_id).unwrap();
        assert_eq!(
            state.entities.row(target).health,
            20.0 - attack.base_damage_hp
        );
        assert_eq!(state.players[&actor_id].inventory.slots[0].damage, Some(1));
        assert_eq!(
            stamina_record(&state, actor.0 as usize, 100.0)
                .unwrap()
                .available_milli,
            milli(100.0) - milli(attack.stamina_cost)
        );
    }
}
