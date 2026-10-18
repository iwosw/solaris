//! View-negotiation residency: transient per-connection radii bound the
//! per-player `desired_chunks` square independently of the configured view.

use crate::boundary::{ChunkPos, ConnectionId, PlayerInput};
use crate::horde_bench::flat_world;
use crate::tick::{TickState, run_tick};
use std::sync::Arc;

fn state() -> TickState {
    let registry = Arc::new(crate::tick_tests::full_registry());
    let world = flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    state.config_view_distance = 16;
    state.config_sim_distance = 2;
    state
}

fn join(state: &mut TickState, connection: u64, view: Option<u8>) -> crate::boundary::EntityId {
    let mut inputs = Vec::new();
    if let Some(view_distance) = view {
        inputs.push(PlayerInput::SetViewDistance {
            connection: ConnectionId(connection),
            view_distance,
        });
    }
    inputs.push(PlayerInput::Join {
        connection: ConnectionId(connection),
        uuid: uuid::Uuid::new_v4(),
        name: format!("viewer{connection}"),
        permission_level: 0,
    });
    run_tick(state, &inputs);
    state
        .connections
        .entity_of(ConnectionId(connection))
        .expect("joined")
}

fn centre(state: &TickState, entity: crate::boundary::EntityId) -> ChunkPos {
    let pos = state.entities.pos(state.entities.slot_of(entity).unwrap());
    ChunkPos {
        x: mc_physics::mth::floor_i32(pos[0]).div_euclid(16),
        z: mc_physics::mth::floor_i32(pos[2]).div_euclid(16),
    }
}

fn square(centre: ChunkPos, radius: i32) -> Vec<ChunkPos> {
    let mut chunks = Vec::new();
    for z in centre.z - radius..=centre.z + radius {
        for x in centre.x - radius..=centre.x + radius {
            chunks.push(ChunkPos { x, z });
        }
    }
    chunks
}

#[test]
fn negotiated_view_bounds_residency_below_config() {
    // Configures view16/sim2; one connection negotiates view4 in the same
    // burst before Join. Residency must be the negotiated 4+1 square, not
    // the configured 16+1 square.
    let mut state = state();
    let player = join(&mut state, 1, Some(4));
    let centre = centre(&state, player);
    let wanted = state.desired_chunks();
    for chunk in square(centre, 5) {
        assert!(wanted.contains_key(&chunk), "missing {chunk:?}");
    }
    assert_eq!(wanted.len(), 11 * 11, "residency must be exactly view4+1");
    assert!(!wanted.contains_key(&ChunkPos {
        x: centre.x + 6,
        z: centre.z
    }));
}

#[test]
fn negotiated_view_update_grows_residency() {
    let mut state = state();
    let player = join(&mut state, 1, Some(4));
    assert_eq!(state.desired_chunks().len(), 11 * 11);
    run_tick(
        &mut state,
        &[PlayerInput::SetViewDistance {
            connection: ConnectionId(1),
            view_distance: 8,
        }],
    );
    let centre = centre(&state, player);
    let wanted = state.desired_chunks();
    for chunk in square(centre, 9) {
        assert!(wanted.contains_key(&chunk), "missing {chunk:?}");
    }
    assert_eq!(wanted.len(), 19 * 19, "residency must be exactly view8+1");
}

#[test]
fn negotiated_view_caps_at_config_and_floors_at_simulation() {
    let mut state = state();
    // Above the configured view: capped at 16+1.
    let player = join(&mut state, 1, Some(32));
    let centre = centre(&state, player);
    let wanted = state.desired_chunks();
    assert_eq!(wanted.len(), 35 * 35);
    assert!(!wanted.contains_key(&ChunkPos {
        x: centre.x + 18,
        z: centre.z
    }));
    // Below the simulation radius: floored at 2+1 so gameplay never loses
    // chunks the simulation needs.
    run_tick(
        &mut state,
        &[PlayerInput::SetViewDistance {
            connection: ConnectionId(1),
            view_distance: 0,
        }],
    );
    let wanted = state.desired_chunks();
    for chunk in square(centre, 3) {
        assert!(wanted.contains_key(&chunk), "missing {chunk:?}");
    }
    assert_eq!(wanted.len(), 7 * 7);
}

#[test]
fn unnegotiated_player_uses_config_view() {
    // Existing core test callers never send SetViewDistance: they must keep
    // the configured 16+1 square.
    let mut state = state();
    join(&mut state, 1, None);
    assert_eq!(state.desired_chunks().len(), 35 * 35);
}

#[test]
fn two_clients_union_individual_radii() {
    let mut state = state();
    let first = join(&mut state, 1, Some(4));
    let second = join(&mut state, 2, Some(8));
    let far: [f64; 3] = [8008.0, 100.0, 8.0];
    state
        .entities
        .set_pos(state.entities.slot_of(second).unwrap(), far);
    let a = centre(&state, first);
    let b = centre(&state, second);
    assert!((a.x - b.x).abs() > 30, "fixtures must not overlap");
    let wanted = state.desired_chunks();
    for chunk in square(a, 5) {
        assert!(wanted.contains_key(&chunk), "missing {chunk:?}");
    }
    assert!(!wanted.contains_key(&ChunkPos { x: a.x + 6, z: a.z }));
    for chunk in square(b, 9) {
        assert!(wanted.contains_key(&chunk), "missing {chunk:?}");
    }
    assert!(!wanted.contains_key(&ChunkPos {
        x: b.x + 10,
        z: b.z
    }));
    assert_eq!(wanted.len(), 11 * 11 + 19 * 19);
}

#[test]
fn leave_clears_negotiated_view() {
    let mut state = state();
    join(&mut state, 1, Some(4));
    assert_eq!(state.connections.view_distance_of(ConnectionId(1)), Some(4));
    run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: ConnectionId(1),
        }],
    );
    assert_eq!(state.connections.view_distance_of(ConnectionId(1)), None);
    // Rejoining without a fresh negotiation falls back to the config view.
    join(&mut state, 1, None);
    assert_eq!(state.desired_chunks().len(), 35 * 35);
}

#[test]
fn connection_transfer_carries_negotiated_view() {
    // Dimension travel moves the connection registry entry; the radius goes
    // with it so residency survives the hop.
    let mut state = state();
    let player = join(&mut state, 1, Some(4));
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    assert_eq!(entity, player);
    let mut other = TickState::new(
        Arc::clone(&state.registry),
        flat_world(&state.registry),
        2,
        512,
    );
    state
        .connections
        .transfer_to(ConnectionId(1), &mut other.connections);
    assert_eq!(state.connections.view_distance_of(ConnectionId(1)), None);
    assert_eq!(other.connections.view_distance_of(ConnectionId(1)), Some(4));
}

#[test]
fn leave_clears_pre_join_negotiation() {
    let mut state = state();
    run_tick(
        &mut state,
        &[PlayerInput::SetViewDistance {
            connection: ConnectionId(1),
            view_distance: 4,
        }],
    );
    run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: ConnectionId(1),
        }],
    );
    assert_eq!(state.connections.view_distance_of(ConnectionId(1)), None);
}

#[test]
fn child_dimension_join_adopts_only_its_pending_negotiation() {
    use crate::dimensions::Dimension;

    for separate_tick in [false, true] {
        let mut root = state();
        let mut child = state();
        child.dimension = Dimension::Nether;
        join(&mut child, 91, None);
        let uuid = child.connections.uuid_of(ConnectionId(91)).unwrap();
        run_tick(
            &mut child,
            &[PlayerInput::Leave {
                connection: ConnectionId(91),
            }],
        );
        assert!(child.offline_players.contains_key(&uuid));
        root.other_dimensions
            .insert(Dimension::Nether, Box::new(child));
        let preference = PlayerInput::SetViewDistance {
            connection: ConnectionId(1),
            view_distance: 4,
        };
        let mut inputs = Vec::new();
        if separate_tick {
            crate::dimensions::run_tick(&mut root, &[preference]);
        } else {
            inputs.push(preference);
        }
        inputs.push(PlayerInput::Join {
            connection: ConnectionId(1),
            uuid,
            name: "viewer".into(),
            permission_level: 0,
        });
        crate::dimensions::run_tick(&mut root, &inputs);
        assert_eq!(root.connections.view_distance_of(ConnectionId(1)), None);
        let child = &root.other_dimensions[&Dimension::Nether];
        assert!(child.connections.entity_of(ConnectionId(1)).is_some());
        assert_eq!(child.connections.view_distance_of(ConnectionId(1)), Some(4));
        assert_eq!(child.desired_chunks().len(), 11 * 11);
        crate::dimensions::run_tick(
            &mut root,
            &[PlayerInput::Leave {
                connection: ConnectionId(1),
            }],
        );
        assert_eq!(
            root.other_dimensions[&Dimension::Nether]
                .connections
                .view_distance_of(ConnectionId(1)),
            None,
        );
    }
}
