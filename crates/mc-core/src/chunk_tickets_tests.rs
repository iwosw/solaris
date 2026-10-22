use super::*;
use crate::boundary::{ConnectionId, EntityId, PlayerInput};
use crate::projectiles::{ProjectileKind, ProjectileState, launch};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use mc_data::ItemStack;
use std::sync::Arc;

fn fixture() -> (TickState, EntityId, Uuid) {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    let uuid = Uuid::new_v4();
    join(&mut state, uuid);
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    (state, player, uuid)
}

fn join(state: &mut TickState, uuid: Uuid) {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid,
            name: "thrower".into(),
            permission_level: 0,
        }],
    );
    settle(state);
}

fn leave(state: &mut TickState) {
    run_tick(
        state,
        &[PlayerInput::Leave {
            connection: ConnectionId(1),
        }],
    );
}

/// Make everything the next tick demands resident, then run it.
fn settle(state: &mut TickState) {
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    run_tick(state, &[]);
}

fn pearl(state: &mut TickState, owner: Option<Uuid>, at: [f64; 3]) -> EntityId {
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:ender_pearl").unwrap())
        .unwrap();
    let record = ProjectileState::new(ProjectileKind::EnderPearl, owner, ItemStack::new(item, 1));
    launch(state, record, at, [0.0; 3]).expect("launch pearl")
}

fn height(state: &TickState, id: EntityId) -> f64 {
    state.entities.pos(state.entities.slot_of(id).unwrap())[1]
}

const FAR: [f64; 3] = [328.0, 250.0, 8.0]; // chunk (20, 0), far from the thrower

#[test]
fn pearl_ticket_loads_five_by_five_and_simulates_three_by_three_without_the_owner_near() {
    let (mut state, _, owner) = fixture();
    let id = pearl(&mut state, Some(owner), FAR);
    assert!(
        !state.world.has_chunk(20, 0),
        "no demand before the first refresh"
    );
    settle(&mut state);
    settle(&mut state);
    assert_eq!(
        state.chunk_tickets.pearl_chunks(),
        [ChunkPos { x: 20, z: 0 }]
    );
    for dz in -2..=2_i32 {
        for dx in -2..=2_i32 {
            assert!(state.world.has_chunk(20 + dx, dz), "loaded ({dx}, {dz})");
            assert_eq!(
                state.world.is_active(20 + dx, dz),
                dx.abs().max(dz.abs()) <= 1,
                "simulated ({dx}, {dz})"
            );
        }
    }
    let before = height(&state, id);
    for _ in 0..5 {
        run_tick(&mut state, &[]);
    }
    assert!(
        height(&state, id) < before,
        "the ticketed pearl keeps flying with the owner far away"
    );
    assert!(
        crate::core::checkpoint_tickets(&state, 1).is_empty(),
        "pearl tickets are derived and never persisted"
    );
}

#[test]
fn pearl_ticket_follows_the_pearl_and_lingers_forty_ticks() {
    let (mut state, _, owner) = fixture();
    let id = pearl(&mut state, Some(owner), FAR);
    settle(&mut state);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, [360.0, 250.0, 8.0]); // chunk (22, 0)
    refresh(&mut state);
    assert_eq!(
        state.chunk_tickets.pearl_chunks(),
        [ChunkPos { x: 20, z: 0 }, ChunkPos { x: 22, z: 0 }]
    );
    // Only the pearl's own chunk is renewed; the old ticket times out.
    for _ in 0..38 {
        state.tick += 1;
        refresh(&mut state);
    }
    assert_eq!(state.chunk_tickets.pearl_chunks().len(), 2, "still held");
    state.tick += 1;
    refresh(&mut state);
    state.tick += 1;
    refresh(&mut state);
    assert_eq!(
        state.chunk_tickets.pearl_chunks(),
        [ChunkPos { x: 22, z: 0 }]
    );
    // Removing the pearl lets its last ticket expire the same way.
    let slot = state.entities.slot_of(id).unwrap();
    state.remove_entity(slot);
    for _ in 0..39 {
        state.tick += 1;
        refresh(&mut state);
    }
    assert_eq!(state.chunk_tickets.pearl_chunks().len(), 1);
    state.tick += 1;
    refresh(&mut state);
    assert!(state.chunk_tickets.pearl_chunks().is_empty());
}

#[test]
fn pearls_without_an_online_player_owner_hold_no_ticket() {
    let (mut state, _, _) = fixture();
    pearl(&mut state, None, FAR);
    pearl(&mut state, Some(Uuid::new_v4()), [-328.0, 250.0, 8.0]);
    refresh(&mut state);
    assert!(state.chunk_tickets.pearl_chunks().is_empty());
}

#[test]
fn an_owner_online_in_another_dimension_keeps_the_ticket() {
    let (mut state, _, _) = fixture();
    let stranger = Uuid::new_v4();
    pearl(&mut state, Some(stranger), FAR);
    refresh(&mut state);
    assert!(state.chunk_tickets.pearl_chunks().is_empty());
    state
        .chunk_tickets
        .set_remote_online(&[stranger].into_iter().collect());
    refresh(&mut state);
    assert_eq!(state.chunk_tickets.pearl_chunks().len(), 1);
}

#[test]
fn one_owner_cannot_ticket_more_than_the_cap_but_pearls_share_a_chunk() {
    let (mut state, _, owner) = fixture();
    for index in 0..40 {
        pearl(
            &mut state,
            Some(owner),
            [328.0 + 16.0 * index as f64, 250.0, 8.0],
        );
    }
    for _ in 0..5 {
        pearl(&mut state, Some(owner), FAR);
    }
    refresh(&mut state);
    assert_eq!(
        state.chunk_tickets.pearl_chunks().len(),
        MAX_CHUNKS_PER_OWNER
    );
    // Another owner has its own allowance.
    let other = Uuid::new_v4();
    state
        .chunk_tickets
        .set_remote_online(&[other].into_iter().collect());
    pearl(&mut state, Some(other), [-328.0, 250.0, 8.0]);
    refresh(&mut state);
    assert_eq!(
        state.chunk_tickets.pearl_chunks().len(),
        MAX_CHUNKS_PER_OWNER + 1
    );
}

#[test]
fn offline_owner_parks_the_pearl_until_the_owner_joins_again() {
    let (mut state, _, owner) = fixture();
    let id = pearl(&mut state, Some(owner), FAR);
    settle(&mut state);
    settle(&mut state);
    let airborne = height(&state, id);
    run_tick(&mut state, &[]);
    assert!(
        height(&state, id) < airborne,
        "flying while the owner is online"
    );
    leave(&mut state);
    let parked_at = state.entities.pos(state.entities.slot_of(id).unwrap());
    let age = |state: &TickState| {
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .projectile
            .as_ref()
            .unwrap()
            .age
    };
    let parked_age = age(&state);
    // Long enough for the ticket (40 ticks) to lapse and the chunk to unload.
    for _ in 0..60 {
        run_tick(&mut state, &[]);
    }
    assert!(state.chunk_tickets.pearl_chunks().is_empty());
    assert_eq!(
        state.entities.pos(state.entities.slot_of(id).unwrap()),
        parked_at,
        "the saved pearl neither flies nor lands"
    );
    assert_eq!(age(&state), parked_age);
    join(&mut state, owner);
    for _ in 0..3 {
        settle(&mut state);
    }
    assert_eq!(
        state.chunk_tickets.pearl_chunks(),
        [ChunkPos { x: 20, z: 0 }],
        "joining restores the ticket"
    );
    assert!(
        state.entities.pos(state.entities.slot_of(id).unwrap())[1] < parked_at[1],
        "the pearl resumes its flight"
    );
}

#[test]
fn pearls_never_expire_by_age_like_other_projectiles() {
    let (mut state, player, owner) = fixture();
    let slot = state.entities.slot_of(player).unwrap();
    let at = state.entities.pos(slot);
    let id = pearl(&mut state, Some(owner), [at[0], 250.0, at[2]]);
    let slot = state.entities.slot_of(id).unwrap();
    state
        .entities
        .row_mut(slot)
        .projectile
        .as_mut()
        .unwrap()
        .age = 5_000;
    for _ in 0..3 {
        run_tick(&mut state, &[]);
    }
    let slot = state
        .entities
        .slot_of(id)
        .expect("pearl outlived the 1200-tick projectile limit");
    assert!(state.entities.row(slot).projectile.as_ref().unwrap().age > 5_000);
}

#[test]
fn ticket_areas_expand_forced_and_pearl_tickets_but_not_generic_ones() {
    let (mut state, _, owner) = fixture();
    state
        .tickets
        .insert("minecraft:forced:1:2".into(), (ChunkPos { x: 1, z: 2 }, 0));
    state
        .tickets
        .insert("plugin:area".into(), (ChunkPos { x: 9, z: 9 }, 3));
    pearl(&mut state, Some(owner), FAR);
    refresh(&mut state);
    let mut areas: Vec<_> = state.ticket_areas().collect();
    areas.sort_by_key(|(chunk, _, _)| (chunk.x, chunk.z));
    assert_eq!(
        areas,
        [
            (ChunkPos { x: 1, z: 2 }, 2, 1),
            (ChunkPos { x: 9, z: 9 }, 3, 3),
            (ChunkPos { x: 20, z: 0 }, 2, 1),
        ]
    );
}
