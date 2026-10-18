use super::*;

fn registry() -> crate::registry::CoreRegistry {
    crate::registry::CoreRegistry::load(None).expect("registry loads")
}

fn cow_type(registry: &crate::registry::CoreRegistry) -> u32 {
    registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap()
}

fn fields() -> ColdFields {
    ColdFields {
        pos: [0.0, 64.0, 0.0],
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

fn spawn(store: &mut EntityStore, id: i32, type_id: u32) -> EntitySlot {
    store.spawn(
        EntityId(id),
        Uuid::from_u128(id as u128),
        EntityKind::Mob { type_id },
        fields(),
    )
}

#[test]
fn body_layout_contact_set_in_first_cache_line() {
    assert_eq!(std::mem::size_of::<Body>(), 72);
    assert_eq!(std::mem::align_of::<Body>(), 8);
    let contact_end = [
        std::mem::offset_of!(Body, id) + std::mem::size_of::<EntityId>(),
        std::mem::offset_of!(Body, kind) + std::mem::size_of::<EntityKind>(),
        std::mem::offset_of!(Body, alive) + std::mem::size_of::<bool>(),
        std::mem::offset_of!(Body, half_width) + std::mem::size_of::<f64>(),
        std::mem::offset_of!(Body, height) + std::mem::size_of::<f64>(),
        std::mem::offset_of!(Body, burn_ticks) + std::mem::size_of::<u32>(),
    ]
    .into_iter()
    .max()
    .unwrap();
    assert!(contact_end <= 64);
    eprintln!(
        "Body={} ColdRow={} contact_end={contact_end}",
        std::mem::size_of::<Body>(),
        std::mem::size_of::<ColdRow>()
    );
}

#[test]
fn spawn_initializes_body_impulse_and_cold() {
    let registry = registry();
    let cow = cow_type(&registry);
    let mut store = EntityStore::default();
    let slot = spawn(&mut store, 51, cow);
    let body = store.body(slot);
    assert_eq!(body.id, EntityId(51));
    assert_eq!(body.kind, EntityKind::Mob { type_id: cow });
    assert!(body.alive);
    assert_eq!((body.half_width, body.height), (0.3, 1.8));
    assert_eq!(body.movement, MovementClass::Walker);
    assert_eq!(body.air_supply, 300);
    assert_eq!(
        (body.burn_ticks, body.pickup_delay, body.attack_cooldown),
        (0, 0, 0)
    );
    assert_eq!(body.hurt_at, None);
    assert_eq!(store.horizontal_impulse(slot), [0.0; 2]);
    assert_eq!(store.impulses()[slot.0 as usize], [0.0; 2]);
    assert_eq!(store.positions()[slot.0 as usize], [0.0, 64.0, 0.0]);
    assert_eq!(store.bodies().len(), 1);
    // Cold keeps the durable remainder.
    assert_eq!(store.row(slot).health, 20.0);
    assert_eq!(store.row(slot).uuid, Uuid::from_u128(51));
}

#[test]
fn body_swap_remove_compacts_all_columns() {
    let registry = registry();
    let cow = cow_type(&registry);
    let mut store = EntityStore::default();
    let first = spawn(&mut store, 61, cow);
    spawn(&mut store, 62, cow);
    let third = spawn(&mut store, 63, cow);
    store.body_mut(first).burn_ticks = 4;
    store.set_horizontal_impulse(third, [1.5, -0.5]);
    store.body_mut(third).air_supply = 299;
    store.remove(first);
    // Tail swapped into the head slot with Body + impulse + cold aligned.
    let moved = store.slot_of_any(EntityId(63)).unwrap();
    assert_eq!(moved, first);
    assert_eq!(store.body(moved).id, EntityId(63));
    assert_eq!(store.body(moved).burn_ticks, 0);
    assert_eq!(store.body(moved).air_supply, 299);
    assert_eq!(store.horizontal_impulse(moved), [1.5, -0.5]);
    assert_eq!(store.len(), 2);
    assert_eq!(store.bodies().len(), store.positions().len());
    assert_eq!(store.impulses().len(), store.positions().len());
    store.prepare_motion_hints(&registry);
    assert_eq!(store.dirty_ids_len_for_test(), 0);
}

#[test]
fn body_transfer_moves_all_columns() {
    let registry = registry();
    let cow = cow_type(&registry);
    let mut source = EntityStore::default();
    let mut destination = EntityStore::default();
    let moving = spawn(&mut source, 71, cow);
    let survivor = spawn(&mut source, 72, cow);
    source.body_mut(survivor).air_supply = 217;
    source.set_horizontal_impulse(survivor, [-3.0, 4.0]);
    let survivor_body = *source.body(survivor);
    source.body_mut(moving).burn_ticks = 7;
    source.body_mut(moving).hurt_at = Some(9);
    source.set_horizontal_impulse(moving, [2.0, 1.0]);
    let transferred_body = *source.body(moving);
    let target = source.transfer_to(moving, &mut destination, [8.0, 65.0, 8.0], [0.0, 0.0]);
    let moved = destination.slot_of_any(EntityId(71)).unwrap();
    assert_eq!(moved, target);
    assert_eq!(*destination.body(moved), transferred_body);
    assert_eq!(destination.horizontal_impulse(moved), [2.0, 1.0]);
    assert!(source.slot_of_any(EntityId(71)).is_none());
    let compacted = source.slot_of_any(EntityId(72)).unwrap();
    assert_eq!(compacted, moving);
    assert_eq!(*source.body(compacted), survivor_body);
    assert_eq!(source.horizontal_impulse(compacted), [-3.0, 4.0]);
    assert_eq!(source.row(compacted).uuid, Uuid::from_u128(72));
    assert_eq!(source.len(), 1);
    destination.prepare_motion_hints(&registry);
    source.prepare_motion_hints(&registry);
    assert_eq!(destination.dirty_ids_len_for_test(), 0);
    assert_eq!(source.dirty_ids_len_for_test(), 0);
}

#[test]
fn body_mut_and_parts_mut_carry_row_mut_marks() {
    let registry = registry();
    let cow = cow_type(&registry);
    let mut store = EntityStore::default();
    let slot = spawn(&mut store, 81, cow);
    store.prepare_motion_hints(&registry);
    store.take_touched();
    // body_mut marks motion-dirty + touched.
    store.body_mut(slot).burn_ticks = 2;
    assert_eq!(store.dirty_ids_len_for_test(), 1);
    assert_eq!(store.touched_ids(), &[EntityId(81)]);
    store.prepare_motion_hints(&registry);
    store.take_touched();
    // parts_mut marks once for mixed body+cold mutation.
    let (body, row) = store.parts_mut(slot);
    body.pickup_delay = 5;
    row.breed_cooldown = 6;
    assert_eq!(store.dirty_ids_len_for_test(), 1);
    assert_eq!(store.touched_ids(), &[EntityId(81)]);
    assert_eq!(store.body(slot).pickup_delay, 5);
    assert_eq!(store.row(slot).breed_cooldown, 6);
    // set_horizontal_impulse marks like row_mut.
    store.prepare_motion_hints(&registry);
    store.take_touched();
    store.set_horizontal_impulse(slot, [0.5, 0.0]);
    assert_eq!(store.dirty_ids_len_for_test(), 1);
    assert_eq!(store.touched_ids(), &[EntityId(81)]);
    // Even a no-op keeps the same conservative custody as row_mut.
    store.prepare_motion_hints(&registry);
    store.take_touched();
    store.set_horizontal_impulse(slot, [0.5, 0.0]);
    assert_eq!(store.dirty_ids_len_for_test(), 1);
    assert_eq!(store.touched_ids(), &[EntityId(81)]);
    store.prepare_motion_hints(&registry);
    store.take_touched();
    // The brain clock's projection deliberately invalidates only motion.
    store.set_brain_impulse(slot, [0.75, 0.0]);
    assert_eq!(store.dirty_ids_len_for_test(), 1);
    assert!(store.touched_ids().is_empty());
}

#[test]
fn land_hit_uses_body_hurt_at_and_marks() {
    let registry = registry();
    let cow = cow_type(&registry);
    let mut store = EntityStore::default();
    let slot = spawn(&mut store, 91, cow);
    store.prepare_motion_hints(&registry);
    store.take_touched();
    // First hit lands and records the tick in the Body column.
    assert_eq!(
        store.land_hit(slot, 100, 5.0, crate::survival::DamageKind::Melee),
        5.0
    );
    assert_eq!(store.body(slot).hurt_at, Some(100));
    assert_eq!(store.row(slot).last_hurt, 5.0);
    // A landing hit takes parts_mut custody: journal + motion queue.
    assert_eq!(store.touched_ids(), &[EntityId(91)]);
    assert_eq!(store.dirty_ids_len_for_test(), 1);
    store.prepare_motion_hints(&registry);
    store.take_touched();
    // Absorbed attempts retain the old row_mut custody; stronger hits land
    // only their excess without extending the original hurt window.
    assert_eq!(
        store.land_hit(slot, 105, 4.0, crate::survival::DamageKind::Melee),
        0.0
    );
    assert_eq!(store.touched_ids(), &[EntityId(91)]);
    assert_eq!(store.dirty_ids_len_for_test(), 1);
    assert_eq!(
        store.land_hit(slot, 105, 7.0, crate::survival::DamageKind::Melee),
        2.0
    );
    assert_eq!(store.row(slot).last_hurt, 7.0);
    assert_eq!(
        store.body(slot).hurt_at,
        Some(100),
        "stronger hits do not extend the window"
    );
    assert_eq!(
        store.land_hit(slot, 110, 1.0, crate::survival::DamageKind::Melee),
        1.0,
        "the original window expires even after a stronger excess hit"
    );
    assert_eq!(store.touched_ids(), &[EntityId(91)]);
    // Void bypasses invulnerability and still lands.
    assert_eq!(
        store.land_hit(slot, 105, 3.0, crate::survival::DamageKind::Void),
        3.0
    );
}
