use super::flat_player_fixture;
use crate::boundary::{BlockPos, ConnectionId, DigAction, PlayerInput};
use crate::tick::{TickState, run_tick};
use mc_data::{Identifier, ItemStack};
use mc_physics::{Aabb, BlockMaterial, EntityBody, Vec3};

struct Material(BlockMaterial);
impl mc_physics::BlockSampler for Material {
    fn material_at(&self, _: i32, _: i32, _: i32) -> BlockMaterial {
        self.0
    }
}
fn body(velocity: Vec3) -> EntityBody {
    EntityBody {
        position: Vec3::new(0.5, 4.0, 0.5),
        velocity,
        aabb: Aabb {
            half_width: 0.125,
            height: 0.25,
        },
        on_ground: false,
    }
}
fn stack(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap(),
        count,
    )
}

#[test]
fn item_motion_air_preserves_launch_velocity_and_integrates_gravity() {
    let next = crate::item_motion::step(
        body(Vec3::new(6.0, 2.0, -4.0)),
        &Material(BlockMaterial::Air),
    );
    assert!((next.position.x - 0.8).abs() < 1e-8);
    assert!((next.position.y - 4.06).abs() < 1e-8);
    assert!((next.position.z - 0.3).abs() < 1e-8);
    assert!((next.velocity.x - 5.88).abs() < 1e-8);
    assert!((next.velocity.y - 1.176).abs() < 1e-8);
}

#[test]
fn item_motion_water_retains_pull_momentum_and_only_lifts_slow_items() {
    let water = Material(BlockMaterial::Water);
    let launched = crate::item_motion::step(body(Vec3::new(8.0, 4.0, -2.0)), &water);
    assert!((launched.position.x - 0.896).abs() < 1e-8);
    assert!((launched.position.y - 4.2).abs() < 1e-8);
    assert!((launched.velocity.x - 8.0 * 0.99 * 0.98).abs() < 1e-8);
    assert!((launched.velocity.y - 3.92).abs() < 1e-8);
    let floating = crate::item_motion::step(body(Vec3::ZERO), &water);
    assert!(floating.position.y > 4.0 && floating.velocity.y > 0.0);
}

#[test]
fn item_motion_q_throw_travels_forward_lands_then_can_be_picked_up() {
    let (mut state, actor, pos) = flat_player_fixture();
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let item = stack(&state, "dirt", 2);
    let actor_slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_rot(actor_slot, 0.0, 0.0);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = item.clone();
    let (out, _, _) = run_tick(
        &mut state,
        &[PlayerInput::DropSelected {
            connection: ConnectionId(1),
            whole_stack: false,
        }],
    );
    let id = out
        .changes
        .entities_spawned
        .iter()
        .copied()
        .find(|id| {
            state
                .entities
                .slot_of(*id)
                .is_some_and(|slot| state.entities.row(slot).item.is_some())
        })
        .unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    assert!(state.entities.pos(slot)[2] > pos[2]);
    assert!(state.entities.vel(slot)[2] > 5.0);
    assert!(state.entities.body(slot).pickup_delay >= 39);
    for _ in 0..30 {
        run_tick(&mut state, &[]);
    }
    let slot = state
        .entities
        .slot_of(id)
        .expect("not picked up before forty ticks");
    let at = state.entities.pos(slot);
    assert!(at[2] > pos[2] + 2.0, "landed at {at:?}, owner {pos:?}");
    assert!((at[1] - pos[1]).abs() < 1e-6 && state.entities.ground(slot));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    let actor_slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(actor_slot, at);
    for _ in 0..12 {
        run_tick(&mut state, &[]);
    }
    assert!(state.entities.slot_of(id).is_none());
    assert_eq!(
        state.players[&actor]
            .inventory
            .slots
            .iter()
            .filter(|s| s.item_id == item.item_id)
            .map(|s| s.count)
            .sum::<i32>(),
        2
    );
}

#[test]
fn item_motion_block_drop_lands_near_its_block_and_keeps_custody() {
    let (mut state, actor, pos) = flat_player_fixture();
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.registry = std::sync::Arc::new(
        crate::registry::CoreRegistry::load(Some(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
        ))
        .unwrap(),
    );
    let block = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default
        .0;
    let at = BlockPos {
        x: pos[0] as i32 + 3,
        y: pos[1] as i32 + 1,
        z: pos[2] as i32,
    };
    state.world.set_block(at.x, at.y, at.z, block);
    state.world.fold_overlay();
    let (out, _, _) = run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: at,
            action: DigAction::Finish,
            sequence: 1,
        }],
    );
    let id = out
        .changes
        .entities_spawned
        .iter()
        .copied()
        .find(|id| {
            state
                .entities
                .slot_of(*id)
                .is_some_and(|slot| state.entities.row(slot).item.is_some())
        })
        .expect("block loot");
    for _ in 0..30 {
        run_tick(&mut state, &[]);
    }
    let slot = state
        .entities
        .slot_of(id)
        .expect("nearby drop not in owner pickup range");
    let landed = state.entities.pos(slot);
    assert!((landed[0] - (f64::from(at.x) + 0.5)).abs() < 0.5);
    assert!((landed[2] - (f64::from(at.z) + 0.5)).abs() < 0.5);
    assert!((landed[1] - pos[1]).abs() < 1e-6 && state.entities.ground(slot));
    assert_eq!(state.entities.row(slot).item.as_ref().unwrap().count, 1);
    assert_eq!(
        state.players[&actor]
            .inventory
            .slots
            .iter()
            .map(|s| s.count)
            .sum::<i32>(),
        0
    );
}

#[test]
fn built_base_downward_drop_stocks_nonfull_hopper_pair() {
    let (mut state, actor, _) = flat_player_fixture();
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let chunk = crate::boundary::ChunkPos { x: -4, z: -4 };
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    state.tickets.insert("built-base".into(), (chunk, 0));
    let stone = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    for x in -64..=-60 {
        for z in -64..=-62 {
            state.world.set_block(x, 63, z, stone);
            for y in 64..=70 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    let dirt = stack(&state, "dirt", 64);
    for (x, facing, count) in [(-62, "east", 63), (-61, "west", 1)] {
        let id = state
            .registry
            .blocks
            .by_name_and_props(
                &Identifier::parse("minecraft:hopper").unwrap(),
                &[
                    ("facing".into(), facing.into()),
                    ("enabled".into(), "true".into()),
                ],
            )
            .unwrap()
            .0;
        state.world.set_block(x, 64, -64, id);
        let mut slots = vec![ItemStack::EMPTY; 5];
        slots[0] = ItemStack::new(dirt.item_id, count);
        state.block_entities.insert(
            (x, 64, -64),
            crate::tick::BlockEntityState {
                slots,
                furnace: None,
            },
        );
    }
    state.world.fold_overlay();
    let actor_slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(actor_slot, [-61.5, 66.0, -63.5]);
    state.entities.set_rot(actor_slot, 0.0, 90.0);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = dirt;
    let (output, _, _) = run_tick(
        &mut state,
        &[PlayerInput::DropSelected {
            connection: ConnectionId(1),
            whole_stack: true,
        }],
    );
    assert_eq!(output.changes.entities_spawned.len(), 1);
    let drop = output.changes.entities_spawned[0];
    let drop_slot = state.entities.slot_of(drop).unwrap();
    assert_eq!(state.entities.body(drop_slot).pickup_delay, 39);
    assert!(state.entities.vel(drop_slot)[1] < 0.0);
    assert!(state.entities.pos(drop_slot)[1] > 66.0);
    for tick in 1..=60 {
        if tick == 15 {
            let actor_slot = state.entities.slot_of(actor).unwrap();
            state.entities.set_pos(actor_slot, [-62.5, 64.0, -63.5]);
        }
        run_tick(&mut state, &[]);
    }
    let hopper_dirt: i32 = [-62, -61]
        .into_iter()
        .flat_map(|x| &state.block_entities[&(x, 64, -64)].slots)
        .filter(|item| item.item_id == stack(&state, "dirt", 1).item_id)
        .map(|item| item.count)
        .sum();
    assert_eq!(
        hopper_dirt, 128,
        "seeded plus thrown stock must reach hoppers"
    );
    assert!(state.entities.slot_of(drop).is_none());
    assert!(
        state.players[&actor]
            .inventory
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
}
