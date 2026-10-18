use super::*;
use crate::boundary::{ConnectionId, Hand, PlayerInput};
use crate::mounts::tests::{fixture, hold, slot, spawn, use_entity};
use crate::tick::run_tick;
#[test]
fn leash_attach_secondary_detach_and_distance_break_transfer_one_lead() {
    let (mut state, actor, pos) = fixture();
    let cow = spawn(&mut state, "cow", [pos[0], pos[1], pos[2] + 1.0]);
    let c = slot(&state, cow);
    hold(&mut state, actor, "lead", 2);
    run_tick(&mut state, &[use_entity(cow)]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(state.entities.row(c).entity_life.leash.is_some());
    assert_eq!(state.entities.row(c).appearance.leash_holder, Some(actor));
    run_tick(
        &mut state,
        &[PlayerInput::InteractEntitySecondary {
            connection: ConnectionId(1),
            target: cow,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    assert!(state.entities.row(c).entity_life.leash.is_none());
    run_tick(&mut state, &[use_entity(cow)]);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    state.entities.set_pos(c, [pos[0] + 13.0, pos[1], pos[2]]);
    run_tick(&mut state, &[]);
    assert!(state.entities.row(c).entity_life.leash.is_none());
    assert!(state.entities.row(c).appearance.leash_holder.is_none());
    let leads = state
        .entities
        .slots()
        .filter_map(|s| state.entities.row(s).item.as_ref())
        .filter(|i| state.registry.items.name_of(i.item_id).unwrap().as_str() == "minecraft:lead")
        .map(|i| i.count)
        .sum::<i32>();
    assert_eq!(leads, 2);
}
#[test]
fn fence_knot_anchors_roundtrip_and_break_releases_lead() {
    let (mut state, actor, pos) = fixture();
    let cow = spawn(&mut state, "cow", pos);
    let c = slot(&state, cow);
    hold(&mut state, actor, "lead", 1);
    run_tick(&mut state, &[use_entity(cow)]);
    let p = BlockPos {
        x: pos[0] as i32 + 2,
        y: pos[1] as i32,
        z: pos[2] as i32,
    };
    let fence = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:oak_fence").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(p.x, p.y, p.z, fence);
    assert!(use_fence(&mut state, actor, p));
    run_tick(&mut state, &[]);
    assert_eq!(
        state.entities.row(c).entity_life.leash,
        Some(Holder::Fence([p.x, p.y, p.z]))
    );
    let fields = crate::entity_life::to_fields(&state.entities.row(c).entity_life, 0);
    assert!(fields.contains(&("leash".into(), mc_nbt::Tag::IntArray(vec![p.x, p.y, p.z]))));
    let mut life = crate::entity_life::EntityLifeState::default();
    crate::mounts::persistence::from_fields(&mut life, &fields).unwrap();
    assert_eq!(life.leash, state.entities.row(c).entity_life.leash);
    assert!(state.entities.row(c).appearance.leash_holder.is_some());
    let knot = state.entities.row(c).appearance.leash_holder.unwrap();
    let knot_slot = slot(&state, knot);
    assert_eq!(
        state.entities.body(knot_slot).movement,
        crate::blueprint::MovementClass::Stationary
    );
    let fixed = [
        f64::from(p.x) + 0.5,
        f64::from(p.y) + 0.375,
        f64::from(p.z) + 0.5,
    ];
    assert_eq!(state.entities.pos(knot_slot), fixed);
    for _ in 0..3 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.entities.pos(slot(&state, knot)), fixed);
    assert_eq!(
        state
            .entities
            .slots()
            .filter(|s| crate::mounts::name(&state, *s) == "minecraft:leash_knot")
            .count(),
        1
    );
    state
        .world
        .set_block(p.x, p.y, p.z, state.registry.states.air);
    run_tick(&mut state, &[]);
    assert!(state.entities.row(c).entity_life.leash.is_none());
}
#[test]
fn named_tag_consumes_once_retains_exact_name_and_marks_persistent() {
    let (mut state, actor, pos) = fixture();
    let cow = spawn(&mut state, "cow", pos);
    hold(&mut state, actor, "name_tag", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0].custom_name =
        Some("{\"text\":\"Bessie\"}".into());
    run_tick(&mut state, &[use_entity(cow)]);
    let row = state.entities.row(slot(&state, cow));
    assert_eq!(
        row.appearance.custom_name.as_deref(),
        Some("{\"text\":\"Bessie\"}")
    );
    assert!(row.persistent);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
}

#[test]
fn shared_fence_index_reuses_one_knot_and_removes_it_after_last_detach() {
    let (mut state, _, pos) = fixture();
    let p = [pos[0] as i32 + 2, pos[1] as i32, pos[2] as i32];
    let fence = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:oak_fence").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(p[0], p[1], p[2], fence);
    let animals: Vec<_> = (0..32)
        .map(|_| {
            let cow = spawn(&mut state, "cow", pos);
            let slot = slot(&state, cow);
            state.entities.set_leash(slot, Some(Holder::Fence(p)));
            cow
        })
        .collect();
    let mut changes = ChangeSet::default();
    tick(&mut state, &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    let knot = changes.entities_spawned[0];
    for cow in &animals {
        assert_eq!(
            state
                .entities
                .row(slot(&state, *cow))
                .appearance
                .leash_holder,
            Some(knot)
        );
    }
    let capacity = state.leash_knots.capacity();
    changes = ChangeSet::default();
    tick(&mut state, &mut changes);
    assert!(changes.entities_spawned.is_empty());
    assert_eq!(state.leash_knots.capacity(), capacity);
    for cow in animals {
        let slot = slot(&state, cow);
        state.entities.set_leash(slot, None);
    }
    tick(&mut state, &mut changes);
    assert_eq!(changes.entities_removed, vec![knot]);
    assert!(state.entities.slot_of(knot).is_none());
    changes = ChangeSet::default();
    tick(&mut state, &mut changes);
    assert!(changes.entities_removed.is_empty());
}
