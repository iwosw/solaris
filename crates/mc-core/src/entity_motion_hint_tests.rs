use super::*;
use crate::boundary::{EffectParticle, EntityDataValue, EntityId};
use crate::effects::{ActiveEffect, EffectId};
use crate::hostile::CREEPER_SWELL_DIR_INDEX;

fn registry() -> crate::registry::CoreRegistry {
    crate::registry::CoreRegistry::load(None).expect("registry loads")
}

fn type_id(registry: &crate::registry::CoreRegistry, name: &str) -> u32 {
    registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}

fn fields(pos: [f64; 3], half_width: f64) -> ColdFields {
    ColdFields {
        pos,
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
        half_width,
        height: 1.8,
        speed: 0.1,
        attack_damage: 1.0,
        sight_range: 16.0,
        attack_reach: 1.0,
    }
}

fn spawn_mob(store: &mut EntityStore, id: i32, type_id: u32) -> EntitySlot {
    store.spawn(
        EntityId(id),
        Uuid::from_u128(id as u128),
        EntityKind::Mob { type_id },
        fields([0.0, 64.0, 0.0], 0.3),
    )
}

fn phase_of(id: i32) -> u16 {
    ((id as u64 & 3) as u16) << MOTION_PHASE_SHIFT
}

#[test]
fn both_custody_paths_enqueue_and_steady_prepare_is_quiet() {
    let registry = registry();
    let cow = type_id(&registry, "minecraft:cow");
    let mut store = EntityStore::default();
    let slot = spawn_mob(&mut store, 7, cow);
    store.prepare_motion_hints(&registry);
    assert_eq!(store.motion_flags(slot) & MOTION_PHASE_MASK, phase_of(7));
    assert!(store.motion_flags(slot) & MOTION_OWNED != 0);
    // Body is the single owner: identity/dimensions read straight from it.
    let body = store.body(slot);
    assert_eq!(
        (body.id, body.half_width, body.hurt_at),
        (EntityId(7), 0.3, None)
    );
    assert!(store.motion_hint(slot).swell_metadata_neutral);
    let queued = store.dirty_ids_len_for_test();
    assert_eq!(queued, 0);
    // Unchanged steady-state tick: no new dirty work from a second prepare.
    store.prepare_motion_hints(&registry);
    assert_eq!(store.dirty_ids_len_for_test(), 0);

    // body_mut custody enqueues once even across repeated borrows.
    store.body_mut(slot).burn_ticks = 5;
    store.body_mut(slot).burn_ticks = 6;
    assert_eq!(store.dirty_ids_len_for_test(), 1);
    store.prepare_motion_hints(&registry);
    assert_eq!(store.motion_flags(slot) & MOTION_REST, 0);
    assert_eq!(store.dirty_ids_len_for_test(), 0);

    // Impulse writes (used by serial writebacks) also invalidate.
    store.body_mut(slot).burn_ticks = 0;
    store.prepare_motion_hints(&registry);
    assert!(store.motion_flags(slot) & MOTION_REST != 0);
    store.set_horizontal_impulse(slot, [1.0, 0.0]);
    assert_eq!(store.dirty_ids_len_for_test(), 1);
    store.prepare_motion_hints(&registry);
    assert_eq!(store.motion_flags(slot) & MOTION_REST, 0);

    // set_brain_impulse invalidates on change, stays quiet on no-op.
    store.set_horizontal_impulse(slot, [0.0; 2]);
    store.prepare_motion_hints(&registry);
    store.set_brain_impulse(slot, [0.0; 2]);
    assert_eq!(store.dirty_ids_len_for_test(), 0);
    store.set_brain_impulse(slot, [0.5, 0.0]);
    assert_eq!(store.dirty_ids_len_for_test(), 1);
}

#[test]
fn phase_bits_cover_slot_order_and_masked_scan() {
    let registry = registry();
    let cow = type_id(&registry, "minecraft:cow");
    let mut store = EntityStore::default();
    for id in [4, 5, 6, 7] {
        spawn_mob(&mut store, id, cow);
    }
    // Signed tail and high-water ids keep the ID-staggered periods 1/2/4.
    spawn_mob(&mut store, -1, cow);
    spawn_mob(&mut store, i32::MAX, cow);
    store.prepare_motion_hints(&registry);
    for id in [4, 5, 6, 7, -1, i32::MAX] {
        let slot = store.slot_of_any(EntityId(id)).unwrap();
        assert_eq!(store.motion_flags(slot) & MOTION_PHASE_MASK, phase_of(id));
    }
    // Exact-mask scan yields global slot order without Body reads.
    let owned: Vec<EntitySlot> = store.motion_slots(MOTION_OWNED).collect();
    assert_eq!(owned, store.slots().collect::<Vec<_>>());
    let rest: Vec<EntitySlot> = store.motion_slots(MOTION_REST).collect();
    assert_eq!(rest.len(), 6);
}

#[test]
fn remove_swap_compacts_columns_and_keeps_queue_valid() {
    let registry = registry();
    let cow = type_id(&registry, "minecraft:cow");
    let mut store = EntityStore::default();
    let first = spawn_mob(&mut store, 11, cow);
    spawn_mob(&mut store, 12, cow);
    let third = spawn_mob(&mut store, 13, cow);
    store.prepare_motion_hints(&registry);
    // Dirty both head and tail, then remove the head: swap compaction moves
    // the tail while the removed id still sits in the queue.
    store.body_mut(first).burn_ticks = 1;
    store.row_mut(third).love = 9;
    store.remove(first);
    // Prepare must ignore the removed id and land refreshed flags on the
    // compacted survivors; Body/impulse/cold stay aligned.
    store.prepare_motion_hints(&registry);
    assert!(store.slot_of_any(EntityId(11)).is_none());
    let moved = store.slot_of_any(EntityId(13)).unwrap();
    assert!(store.motion_flags(moved) & MOTION_LIFE != 0);
    assert_eq!(store.body(moved).id, EntityId(13));
    assert_eq!(store.body(moved).kind, EntityKind::Mob { type_id: cow });
    assert_eq!(store.row(moved).love, 9);
    assert_eq!(store.dirty_ids_len_for_test(), 0);
    assert_eq!(store.len(), 2);
}

#[test]
fn transfer_aligns_source_and_destination_hints() {
    let registry = registry();
    let cow = type_id(&registry, "minecraft:cow");
    let mut source = EntityStore::default();
    let mut destination = EntityStore::default();
    let keep = spawn_mob(&mut source, 21, cow);
    let moving = spawn_mob(&mut source, 22, cow);
    source.prepare_motion_hints(&registry);
    source.body_mut(moving).burn_ticks = 3;
    source.set_horizontal_impulse(moving, [0.25, 0.0]);
    source.prepare_motion_hints(&registry);
    let target = source.transfer_to(moving, &mut destination, [32.5, 70.0, 48.5], [90.0, 5.0]);
    // Destination recomputes on next prepare; source survivor stays aligned.
    destination.prepare_motion_hints(&registry);
    source.prepare_motion_hints(&registry);
    let moved = destination.slot_of_any(EntityId(22)).unwrap();
    assert_eq!(moved, target);
    assert_eq!(destination.motion_flags(moved) & MOTION_REST, 0);
    assert_eq!(destination.body(moved).id, EntityId(22));
    assert_eq!(destination.body(moved).burn_ticks, 3);
    assert_eq!(destination.horizontal_impulse(moved), [0.25, 0.0]);
    assert_eq!(
        destination.positions()[moved.0 as usize],
        [32.5, 70.0, 48.5]
    );
    assert!(source.slot_of_any(EntityId(22)).is_none());
    assert_eq!(source.body(keep).id, EntityId(21));
    assert_eq!(source.dirty_ids_len_for_test(), 0);
    assert_eq!(destination.dirty_ids_len_for_test(), 0);
}

#[test]
fn effect_and_life_flags_transition_on_row_edits() {
    let registry = registry();
    let cow = type_id(&registry, "minecraft:cow");
    let mut store = EntityStore::default();
    let slot = spawn_mob(&mut store, 31, cow);
    store.prepare_motion_hints(&registry);
    assert_eq!(store.motion_flags(slot) & MOTION_EFFECTS, 0);
    assert_eq!(store.motion_flags(slot) & MOTION_LIFE, 0);

    store
        .row_mut(slot)
        .active_effects
        .add(ActiveEffect::new(EffectId::Speed, 200, 0));
    store.prepare_motion_hints(&registry);
    assert!(store.motion_flags(slot) & MOTION_EFFECTS != 0);
    store.row_mut(slot).active_effects.clear(None);
    store.prepare_motion_hints(&registry);
    assert_eq!(store.motion_flags(slot) & MOTION_EFFECTS, 0);

    // Absorption and shared-flag invisibility/glowing bits also count.
    Arc::make_mut(&mut store.row_mut(slot).appearance).absorption = 2.0;
    store.prepare_motion_hints(&registry);
    assert!(store.motion_flags(slot) & MOTION_EFFECTS != 0);
    {
        let appearance = Arc::make_mut(&mut store.row_mut(slot).appearance);
        appearance.absorption = 0.0;
        appearance
            .effect_particles
            .push(EffectParticle::EntityEffect { argb: 1 });
    }
    store.prepare_motion_hints(&registry);
    assert!(store.motion_flags(slot) & MOTION_EFFECTS != 0);

    // Life clock: breed cooldown and stuck arrows each set the flag.
    store.row_mut(slot).breed_cooldown = 10;
    store.prepare_motion_hints(&registry);
    assert!(store.motion_flags(slot) & MOTION_LIFE != 0);
    store.row_mut(slot).breed_cooldown = 0;
    Arc::make_mut(&mut store.row_mut(slot).appearance).arrow_count = 2;
    store.prepare_motion_hints(&registry);
    assert!(store.motion_flags(slot) & MOTION_LIFE != 0);
}

#[test]
fn special_and_swell_inputs_refresh_from_row() {
    let registry = registry();
    let skeleton = type_id(&registry, "minecraft:skeleton");
    let cow = type_id(&registry, "minecraft:cow");
    let mut store = EntityStore::default();
    let ranged = spawn_mob(&mut store, 41, skeleton);
    let plain = spawn_mob(&mut store, 42, cow);
    store.prepare_motion_hints(&registry);
    assert!(store.motion_flags(ranged) & MOTION_SPECIAL != 0);
    assert!(store.motion_hint(ranged).special.is_some());
    assert_eq!(store.motion_flags(plain) & MOTION_SPECIAL, 0);
    // NoAI hides the blueprint special from gating.
    store.row_mut(ranged).controls.no_ai = true;
    store.prepare_motion_hints(&registry);
    assert_eq!(store.motion_flags(ranged) & MOTION_SPECIAL, 0);
    assert!(store.motion_hint(ranged).special.is_none());

    // Swell-metadata neutrality caches the canonical helper result.
    let creeper = type_id(&registry, "minecraft:creeper");
    let swell = spawn_mob(&mut store, 43, creeper);
    store.prepare_motion_hints(&registry);
    assert!(store.motion_hint(swell).swell_metadata_neutral);
    {
        let extra = &mut Arc::make_mut(&mut store.row_mut(swell).appearance).extra;
        extra.push((CREEPER_SWELL_DIR_INDEX, EntityDataValue::VarInt(1)));
    }
    store.prepare_motion_hints(&registry);
    assert!(!store.motion_hint(swell).swell_metadata_neutral);

    // Half width and hurt tick refresh with the body.
    store.set_half_width(plain, 0.9);
    store.body_mut(plain).hurt_at = Some(77);
    store.prepare_motion_hints(&registry);
    let body = store.body(plain);
    assert_eq!((body.half_width, body.hurt_at), (0.9, Some(77)));
}
