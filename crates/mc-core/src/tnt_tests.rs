use super::*;
use crate::boundary::ChunkPos;
use crate::journal::{Replay, replay_batches};
use crate::tick::run_tick;

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            BlockStateId(self.0),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}

fn fixture() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        BlockStateId(registry.states.air),
        [0, 201, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    state
        .tickets
        .insert("tnt-test".into(), (ChunkPos { x: 0, z: 0 }, 1));
    state.world.ensure_blocking(&state.desired_chunks());
    let stone = state.registry.states.stone;
    for x in -8..=8 {
        for z in -8..=8 {
            state.world.set_block(x, 200, z, stone);
        }
    }
    state.world.fold_overlay();
    state
        .gamerules
        .insert("random_tick_speed".into(), "0".into());
    state
}

#[test]
fn block_prime_is_exactly_once_and_item_prime_never_removes_a_block() {
    let mut state = fixture();
    let tnt = default_block(&state.registry).unwrap();
    let pos = BlockPos { x: 1, y: 201, z: 1 };
    state.world.set_block(pos.x, pos.y, pos.z, tnt);
    let first = prime_block(&mut state, pos, DEFAULT_FUSE).unwrap();
    assert!(prime_block(&mut state, pos, DEFAULT_FUSE).is_none());
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(state.registry.states.air)
    );
    let stone = state.registry.states.stone;
    state.world.set_block(3, 201, 3, stone);
    assert!(prime_block(&mut state, BlockPos { x: 3, y: 201, z: 3 }, 80).is_none());
    let item = prime_at(&mut state, [3.5, 202.0, 3.5], 80).unwrap();
    assert_eq!(state.world.block_at(3, 201, 3), Some(stone));
    let (output, _, _) = run_tick(&mut state, &[]);
    assert_eq!(
        output
            .changes
            .entities_spawned
            .iter()
            .filter(|id| **id == first)
            .count(),
        1
    );
    assert_eq!(
        output
            .changes
            .entities_spawned
            .iter()
            .filter(|id| **id == item)
            .count(),
        1
    );
}

#[test]
fn fuse_motion_metadata_and_expiration_produce_one_blast_and_removal() {
    let mut state = fixture();
    let id = prime_at(&mut state, [0.5, 201.0, 0.5], 3).unwrap();
    let initial = state.entities.pos(state.entities.slot_of(id).unwrap());
    for remaining in [2, 1] {
        let (output, _, _) = run_tick(&mut state, &[]);
        assert!(output.changes.explosions.is_empty());
        let slot = state.entities.slot_of(id).unwrap();
        assert_eq!(state.entities.row(slot).tnt.unwrap().fuse, remaining);
        assert!(
            state
                .entities
                .row(slot)
                .appearance
                .extra
                .contains(&(FUSE_INDEX, EntityDataValue::VarInt(remaining as i32)))
        );
    }
    assert_ne!(
        state.entities.pos(state.entities.slot_of(id).unwrap()),
        initial
    );
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(state.entities.slot_of_any(id).is_none());
    assert_eq!(output.changes.explosions.len(), 1);
    assert_eq!(
        output
            .changes
            .entities_removed
            .iter()
            .filter(|removed| **removed == id)
            .count(),
        1
    );
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(output.changes.explosions.is_empty());
}

#[test]
fn finite_blast_stock_expires_once_per_tick_with_1024_husks() {
    let mut state = fixture();
    let husk = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:husk").unwrap())
        .unwrap();
    for n in 0..1024 {
        state.spawn_mob(
            husk,
            [
                f64::from(n % 16) - 7.5,
                201.0,
                f64::from(n / 16) / 4.0 - 7.5,
            ],
            20.0,
        );
    }
    let mut ids = Vec::new();
    for n in 0..96 {
        let id = prime_at(&mut state, [f64::from(n) / 10.0 - 4.5, 203.0, 0.5], 3).unwrap();
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.set_vel(slot, [0.0; 3]);
        ids.push(id);
    }
    for remaining in [2, 1] {
        let (output, _, _) = run_tick(&mut state, &[]);
        assert!(output.changes.explosions.is_empty());
        for id in &ids {
            let slot = state.entities.slot_of(*id).unwrap();
            assert_eq!(state.entities.row(slot).tnt.unwrap().fuse, remaining);
        }
    }
    let (output, _, _) = run_tick(&mut state, &[]);
    assert_eq!(output.changes.explosions.len(), 96);
    let centers: std::collections::BTreeSet<_> = output
        .changes
        .explosions
        .iter()
        .map(|event| event.center.map(f64::to_bits))
        .collect();
    assert_eq!(centers.len(), 96);
    for id in ids {
        assert!(state.entities.slot_of_any(id).is_none());
        assert_eq!(
            output
                .changes
                .entities_removed
                .iter()
                .filter(|gone| **gone == id)
                .count(),
            1
        );
    }
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(output.changes.explosions.is_empty());
}

#[test]
fn explosion_chains_tnt_with_shortened_fuse_instead_of_loot() {
    let mut state = fixture();
    let pos = BlockPos { x: 1, y: 201, z: 0 };
    let tnt = default_block(&state.registry).unwrap();
    state.world.set_block(pos.x, pos.y, pos.z, tnt);
    crate::explosion::apply(
        &mut state,
        Some(EntityId(123)),
        [0.5, 201.5, 0.5],
        4.0,
        false,
        None,
    );
    let primed: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).tnt)
        .collect();
    assert_eq!(primed.len(), 1);
    assert!((10..=29).contains(&primed[0].fuse));
    assert_eq!(primed[0].block_state, tnt);
    let item_id = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:tnt").unwrap())
        .unwrap();
    assert!(!state.entities.slots().any(|slot| {
        state
            .entities
            .row(slot)
            .item
            .as_ref()
            .is_some_and(|item| item.item_id == item_id)
    }));
    assert!(prime_block(&mut state, pos, 80).is_none());
}

#[test]
fn disabled_explosion_rule_preserves_block_and_expired_entity_does_not_blast() {
    let mut state = fixture();
    let tnt = default_block(&state.registry).unwrap();
    state.world.set_block(1, 201, 1, tnt);
    let id = prime_at(&mut state, [0.5, 201.0, 0.5], 1).unwrap();
    state
        .gamerules
        .insert("tnt_explodes".into(), "false".into());
    assert!(prime_block(&mut state, BlockPos { x: 1, y: 201, z: 1 }, 80).is_none());
    assert_eq!(state.world.block_at(1, 201, 1), Some(tnt));
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(state.entities.slot_of_any(id).is_none());
    assert!(output.changes.explosions.is_empty());
}

#[test]
fn generic_damage_never_kills_or_loots_nonliving_primed_tnt() {
    let mut state = fixture();
    let id = prime_at(&mut state, [0.5, 201.0, 0.5], 80).unwrap();
    let mut intents = vec![Intent {
        actor: EntityId(987),
        seq: 0,
        kind: IntentKind::DamageEntity {
            target: id,
            amount: 100.0,
            kind: crate::survival::DamageKind::Explosion,
            source: None,
        },
        footprint: Footprint {
            entity_writes: vec![id],
            ..Default::default()
        },
    }];
    state.merge(&mut intents);
    let (deaths, events, drops, removed) = state.commit_merge_effects();
    assert!(deaths.is_empty());
    assert!(drops.is_empty());
    assert!(removed.is_empty());
    assert!(!events.iter().any(
        |event| matches!(event, crate::boundary::EntityEvent::Death { entity } if *entity == id)
    ));
    let row = state.entities.row(state.entities.slot_of(id).unwrap());
    assert_eq!(row.tnt.unwrap().fuse, 80);
    assert!(
        state
            .entities
            .body(state.entities.slot_of(id).unwrap())
            .alive
    );
}

#[test]
fn checkpoint_nbt_and_replayed_tail_preserve_fuse_block_state_and_motion() {
    let mut state = fixture();
    let name = mc_data::Identifier::parse("minecraft:tnt").unwrap();
    let tnt = state
        .registry
        .blocks
        .by_name_and_props(&name, &[("unstable".into(), "true".into())])
        .unwrap();
    state.world.set_block(0, 201, 0, tnt.0);
    let id = prime_block(&mut state, BlockPos { x: 0, y: 201, z: 0 }, 9).unwrap();
    let (birth, _, _) = run_tick(&mut state, &[]);
    let first = crate::core::journal_entries_for_tick(&mut state, &birth);
    let (_, regions, _) = crate::core::checkpoint_data_for(&state, birth.tick);
    let dir = std::env::temp_dir().join(format!("solaris-tnt-state-{}", uuid::Uuid::new_v4()));
    mc_world::anvil::entity_regions::write_entity_regions(
        &dir,
        &state.registry.items,
        &regions,
        u32::try_from(birth.tick).unwrap(),
    )
    .unwrap();
    let regions =
        mc_world::anvil::entity_regions::read_entity_regions(&dir, &state.registry.items).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    let native = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|record| record.wire_id == id.0)
        .unwrap();
    assert!(
        native
            .extra_fields
            .contains(&("fuse".into(), Tag::Short(8)))
    );
    let retained = from_record(&state.registry, native).unwrap().unwrap();
    assert_eq!(retained.block_state, tnt.0);
    assert_eq!(retained.fuse, 8);
    assert_eq!(
        to_fields(&retained, &state.registry.blocks)
            .iter()
            .find(|(key, _)| key == "block_state"),
        native
            .extra_fields
            .iter()
            .find(|(key, _)| key == "block_state")
    );
    let mut checkpoint = replay_batches(Replay::default(), &[(birth.tick, birth.tick, first)]);
    let saved_base = checkpoint.entities.get_mut(&id.0).unwrap();
    saved_base.state =
        crate::journal::EntityStateImage::from_record(&state.registry, native).unwrap();
    saved_base.pos = native.pos;
    saved_base.vel = native.motion.map(|value| value * 20.0);
    saved_base.ground = native.on_ground;
    let (tail, _, _) = run_tick(&mut state, &[]);
    let second = crate::core::journal_entries_for_tick(&mut state, &tail);
    let batches = vec![(tail.tick, tail.tick, second)];
    let replay = replay_batches(checkpoint, &batches);
    let replay = replay_batches(replay, &batches);
    let saved = &replay.entities[&id.0];
    let live_slot = state.entities.slot_of(id).unwrap();
    assert_eq!(saved.state.tnt.unwrap().fuse, 7);
    assert_eq!(saved.pos, state.entities.pos(live_slot));
    assert_eq!(saved.vel, state.entities.vel(live_slot));
    assert_eq!(saved.ground, state.entities.ground(live_slot));
    let mut restarted = fixture();
    crate::core::restore_entity(&mut restarted, id.0, saved).unwrap();
    let slot = restarted.entities.slot_of(id).unwrap();
    assert_eq!(
        restarted.entities.row(slot).tnt,
        state.entities.row(live_slot).tnt
    );
    assert_eq!(restarted.entities.pos(slot), saved.pos);
    assert_eq!(restarted.entities.vel(slot), saved.vel);
    assert_eq!(restarted.entities.ground(slot), saved.ground);
    assert_eq!(restarted.entities.body(slot).height, HEIGHT);
    let mut blasts = 0;
    for _ in 0..8 {
        let (output, _, _) = run_tick(&mut restarted, &[]);
        blasts += output.changes.explosions.len();
    }
    assert_eq!(blasts, 1);
    assert!(restarted.entities.slot_of_any(id).is_none());
}

#[test]
fn tnt_drop_decay_rule_changes_loot_not_blast_footprint() {
    let explode = |decay: bool| {
        let mut state = fixture();
        state
            .gamerules
            .insert("tnt_explosion_drop_decay".into(), decay.to_string());
        crate::explosion::apply(
            &mut state,
            Some(EntityId(123)),
            [0.5, 201.0, 0.5],
            4.0,
            false,
            None,
        );
        let stone = state.registry.states.stone;
        let destroyed_stone = state
            .merge_block_writes
            .iter()
            .filter(|(pos, _)| state.world.block_at(pos.x, pos.y, pos.z) == Some(stone))
            .count() as i32;
        let cobblestone = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:cobblestone").unwrap())
            .unwrap();
        let drops: i32 = state
            .entities
            .slots()
            .filter_map(|slot| {
                state
                    .entities
                    .row(slot)
                    .item
                    .as_ref()
                    .filter(|item| item.item_id == cobblestone)
                    .map(|item| item.count)
            })
            .sum();
        (destroyed_stone, drops)
    };
    let (without_footprint, without_decay) = explode(false);
    let (with_footprint, with_decay) = explode(true);
    assert!(without_footprint > 0);
    assert_eq!(with_footprint, without_footprint);
    assert_eq!(without_decay, without_footprint);
    assert!(with_decay < without_decay);
}

#[test]
fn powered_tnt_hydration_and_placement_prime_once_with_current_tick_air() {
    for hydration in [true, false] {
        let mut state = fixture();
        if !hydration {
            let _ = run_tick(&mut state, &[]);
        }
        let tnt = default_block(&state.registry).unwrap();
        let redstone = state
            .registry
            .blocks
            .block(&mc_data::Identifier::parse("minecraft:redstone_block").unwrap())
            .unwrap()
            .default
            .0;
        let pos = BlockPos { x: 1, y: 201, z: 1 };
        state.world.set_block(0, 201, 1, redstone);
        state.world.set_block(pos.x, pos.y, pos.z, tnt);
        if hydration {
            state.world.fold_overlay();
        } else {
            state.merge_block_writes.push((pos, tnt));
        }
        let (output, _, _) = run_tick(&mut state, &[]);
        let primed: Vec<_> = state
            .entities
            .slots()
            .filter(|slot| state.entities.row(*slot).tnt.is_some())
            .collect();
        assert_eq!(primed.len(), 1);
        assert_eq!(
            state.world.block_at(pos.x, pos.y, pos.z),
            Some(state.registry.states.air)
        );
        let id = state.entities.body(primed[0]).id;
        assert_eq!(
            output
                .changes
                .entities_spawned
                .iter()
                .filter(|spawn| **spawn == id)
                .count(),
            1
        );
        assert!(
            output
                .changes
                .blocks
                .iter()
                .any(|change| change.position == pos && change.state == state.registry.states.air)
        );
        let (_, _, _) = run_tick(&mut state, &[]);
        assert_eq!(
            state
                .entities
                .slots()
                .filter(|slot| state.entities.row(*slot).tnt.is_some())
                .count(),
            1
        );
    }
}

#[test]
fn sparse_tnt_index_tracks_spawn_restore_and_swap_removal() {
    let mut state = fixture();
    assert_eq!(state.entities.next_tnt(None), None);
    let first = prime_at(&mut state, [0.5, 201.0, 0.5], 1).unwrap();
    let second = prime_at(&mut state, [2.5, 201.0, 0.5], 4).unwrap();
    assert_eq!(state.entities.next_tnt(None), Some(first));
    assert_eq!(state.entities.next_tnt(Some(first)), Some(second));
    assert_eq!(state.entities.last_tnt(), Some(second));
    let (output, _, _) = run_tick(&mut state, &[]);
    assert_eq!(output.changes.explosions.len(), 1);
    assert_eq!(state.entities.next_tnt(None), Some(second));
    assert_eq!(state.entities.next_tnt(Some(second)), None);
    let slot = state.entities.slot_of(second).unwrap();
    assert_eq!(state.entities.row(slot).tnt.unwrap().fuse, 3);
    state.remove_entity(slot);
    assert_eq!(state.entities.next_tnt(None), None);
    assert_eq!(state.entities.last_tnt(), None);
}

#[test]
fn generic_tnt_spawn_initializes_default_fuse_without_item_launch_or_publication() {
    let mut state = fixture();
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:tnt").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, [0.5, 201.0, 0.5], 1.0);
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row(slot);
    let record = row.tnt.unwrap();
    assert_eq!(record.fuse, 80);
    assert_eq!(record.block_state, default_block(&state.registry).unwrap());
    assert_eq!(state.entities.vel(slot), [0.0; 3]);
    assert!(
        row.appearance
            .extra
            .contains(&(8, EntityDataValue::VarInt(80)))
    );
    assert!(
        row.appearance
            .extra
            .contains(&(9, EntityDataValue::BlockState(record.block_state)))
    );
    assert_eq!(state.entities.next_tnt(None), Some(id));
    assert!(state.hostile.spawned.is_empty());
    let _ = run_tick(&mut state, &[]);
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(state.entities.row(slot).tnt.unwrap().fuse, 79);
}
