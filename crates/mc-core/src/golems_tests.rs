use super::*;
use crate::boundary::{ChunkPos, ConnectionId, EntityId, PlayerInput};
use crate::journal::{
    EntityStateImage, JournalWriter, Replay, read_complete_batches_after, replay_batches,
};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use std::sync::{Arc, LazyLock};

/// Pattern origin: the head lands at `HEAD`, the body hangs below it.
const HEAD: BlockPos = BlockPos { x: 4, y: 203, z: 3 };

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            BlockStateId(self.0),
            Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}

fn fixture() -> TickState {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let mut world = ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        BlockStateId(registry.states.air),
        [1, 201, 3],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    for x in 0..12 {
        for z in 0..12 {
            world.set_block(x, 200, z, registry.states.stone);
        }
    }
    world.fold_overlay();
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .tickets
        .insert("golem-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    state
}

fn block(state: &TickState, path: &str) -> u32 {
    let name = Identifier::parse(format!("minecraft:{path}")).unwrap();
    state.registry.blocks.block(&name).unwrap().default.0
}

fn put(state: &mut TickState, pos: BlockPos, path: &str) {
    let id = block(state, path);
    state.world.set_block(pos.x, pos.y, pos.z, id);
}

fn cell(dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos {
        x: HEAD.x + dx,
        y: HEAD.y + dy,
        z: HEAD.z + dz,
    }
}

/// The full iron T below `HEAD`, arms along `(dx, dz)`.
fn iron_t(state: &mut TickState, (dx, dz): (i32, i32)) {
    for step in -1..=1 {
        put(state, cell(dx * step, -1, dz * step), "iron_block");
    }
    put(state, cell(0, -2, 0), "iron_block");
}

fn place_head(state: &mut TickState, path: &str) -> Vec<(BlockPos, u32)> {
    put(state, HEAD, path);
    let head = state.world.block_at(HEAD.x, HEAD.y, HEAD.z).unwrap();
    let mut writes = vec![(HEAD, head)];
    after_blocks(state, &mut writes);
    writes
}

fn golems(state: &TickState, entity: &str) -> Vec<EntityId> {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(entity).unwrap())
        .unwrap();
    state
        .entities
        .slots()
        .filter(|&slot| {
            state.entities.body(slot).kind == crate::entities::EntityKind::Mob { type_id }
        })
        .map(|slot| state.entities.body(slot).id)
        .collect()
}

fn is_air(state: &TickState, pos: BlockPos) -> bool {
    state.world.block_at(pos.x, pos.y, pos.z) == Some(state.registry.states.air)
}

#[test]
fn snow_golem_consumes_two_snow_blocks_and_head() {
    for head in ["carved_pumpkin", "jack_o_lantern"] {
        let mut state = fixture();
        put(&mut state, cell(0, -1, 0), "snow_block");
        put(&mut state, cell(0, -2, 0), "snow_block");
        let writes = place_head(&mut state, head);
        for dy in 0..=2 {
            assert!(is_air(&state, cell(0, -dy, 0)), "{head} {dy}");
        }
        assert_eq!(writes.len(), 3, "air post-images are journaled");
        let found = golems(&state, "minecraft:snow_golem");
        assert_eq!(found.len(), 1);
        let slot = state.entities.slot_of(found[0]).unwrap();
        assert_eq!(state.entities.pos(slot), [4.5, 201.05, 3.5]);
        assert!(state.entities.row(slot).special_mob.is_none());
        assert_eq!(state.hostile.spawned, found);
    }
}

#[test]
fn iron_golem_consumes_t_in_both_orientations_and_is_player_created() {
    for arms in [(1, 0), (0, 1)] {
        for head in ["carved_pumpkin", "jack_o_lantern"] {
            let mut state = fixture();
            iron_t(&mut state, arms);
            let writes = place_head(&mut state, head);
            for pos in [
                cell(0, 0, 0),
                cell(0, -1, 0),
                cell(0, -2, 0),
                cell(arms.0, -1, arms.1),
                cell(-arms.0, -1, -arms.1),
            ] {
                assert!(is_air(&state, pos), "{head} {arms:?} {pos:?}");
            }
            assert_eq!(writes.len(), 5);
            let found = golems(&state, "minecraft:iron_golem");
            assert_eq!(found.len(), 1);
            let slot = state.entities.slot_of(found[0]).unwrap();
            assert_eq!(state.entities.pos(slot), [4.5, 201.05, 3.5]);
            assert_eq!(state.entities.row(slot).health, 100.0);
            assert_eq!(
                state.entities.row(slot).special_mob.as_deref(),
                Some(&Retained::PlayerIronGolem)
            );
        }
    }
}

#[test]
fn broken_patterns_leave_blocks_and_spawn_nothing() {
    type Setup = fn(&mut TickState);
    let cases: [(&str, Setup); 6] = [
        ("one snow block", |s| {
            put(s, cell(0, -1, 0), "snow_block");
        }),
        ("snow over stone", |s| {
            put(s, cell(0, -1, 0), "snow_block");
            put(s, cell(0, -2, 0), "stone");
        }),
        ("lower corner not air", |s| {
            iron_t(s, (1, 0));
            put(s, cell(1, -2, 0), "stone");
        }),
        ("upper corner not air", |s| {
            iron_t(s, (0, 1));
            put(s, cell(0, 0, -1), "stone");
        }),
        ("gold arm", |s| {
            iron_t(s, (1, 0));
            put(s, cell(-1, -1, 0), "gold_block");
        }),
        ("no stem", |s| {
            for step in -1..=1 {
                put(s, cell(step, -1, 0), "iron_block");
            }
        }),
    ];
    for (name, setup) in cases {
        let mut state = fixture();
        setup(&mut state);
        let writes = place_head(&mut state, "carved_pumpkin");
        assert_eq!(writes.len(), 1, "{name}");
        assert!(!is_air(&state, HEAD), "{name}");
        assert!(!is_air(&state, cell(0, -1, 0)), "{name}");
        assert!(golems(&state, "minecraft:iron_golem").is_empty(), "{name}");
        assert!(golems(&state, "minecraft:snow_golem").is_empty(), "{name}");
    }
}

#[test]
fn plain_pumpkin_does_not_build_a_golem() {
    let mut state = fixture();
    iron_t(&mut state, (1, 0));
    place_head(&mut state, "pumpkin");
    assert!(golems(&state, "minecraft:iron_golem").is_empty());
    assert!(!is_air(&state, cell(0, -1, 0)));
}

#[test]
fn player_created_flag_survives_checkpoint_fields_and_journal_replay() {
    let dir = std::env::temp_dir().join(format!("golem-replay-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut state = fixture();
    iron_t(&mut state, (0, 1));
    place_head(&mut state, "carved_pumpkin");
    let id = golems(&state, "minecraft:iron_golem")[0];
    let uuid = state.entities.row(state.entities.slot_of(id).unwrap()).uuid;

    // Vanilla NBT carries PlayerCreated and restores to the same native state.
    let (_, regions, _) = crate::core::checkpoint_data_for(&state, 0);
    let record = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == uuid)
        .unwrap();
    assert!(
        record
            .extra_fields
            .iter()
            .any(|(key, tag)| key == "PlayerCreated" && *tag == mc_nbt::Tag::Byte(1))
    );
    let image = EntityStateImage::from_record(&state.registry, record).unwrap();
    assert_eq!(image.special_mob, Some(Retained::PlayerIronGolem));

    // WAL: the spawn tick carries the flag, and replay is idempotent.
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer.gate(0, output.tick).unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let once = replay_batches(Replay::default(), &batches);
    assert_eq!(
        once.entities[&id.0].state.special_mob,
        Some(Retained::PlayerIronGolem)
    );
    let twice = replay_batches(once, &batches);
    assert_eq!(
        twice.entities[&id.0].state.special_mob,
        Some(Retained::PlayerIronGolem)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn player_created_iron_golem_spares_the_player_who_hit_it() {
    let mut health = [0.0; 2];
    for (index, player_created) in [false, true].into_iter().enumerate() {
        let mut state = fixture();
        run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(1),
                uuid: uuid::Uuid::new_v4(),
                name: "builder".into(),
                permission_level: 0,
            }],
        );
        let player = state.connections.entity_of(ConnectionId(1)).unwrap();
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.set_pos(slot, [8.5, 201.0, 8.5]);
        // Spawn protection would swallow the golem's hits.
        state.entities.row_mut(slot).protected_until = 0;
        let type_id = state
            .registry
            .entity_types
            .id_of(&Identifier::parse("minecraft:iron_golem").unwrap())
            .unwrap();
        let golem = state.spawn_mob(type_id, [8.5, 201.0, 7.0], 100.0);
        let golem_slot = state.entities.slot_of(golem).unwrap();
        let row = state.entities.row_mut(golem_slot);
        row.last_player_hit = Some((player, state.tick));
        if player_created {
            row.set_special_mob(Retained::PlayerIronGolem);
        }
        for _ in 0..40 {
            run_tick(&mut state, &[]);
        }
        health[index] = state.players[&player].vitals.health;
    }
    assert!(health[0] < 20.0, "an ordinary golem retaliates: {health:?}");
    assert_eq!(
        health[1], 20.0,
        "a player-created golem never attacks players"
    );
}

#[test]
fn iron_ingot_repairs_golem_from_either_hand_without_overhealing() {
    use crate::boundary::Hand;
    for (hand, creative) in [(Hand::Main, false), (Hand::Off, false), (Hand::Main, true)] {
        let mut state = fixture();
        run_tick(&mut state, &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::from_u128(1),
            name: "repairer".into(),
            permission_level: 0,
        }]);
        let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
        let actor_slot = state.entities.slot_of(actor).unwrap();
        state.entities.set_pos(actor_slot, [8.5, 201.0, 8.5]);
        let type_id = state.registry.entity_types
            .id_of(&Identifier::parse("minecraft:iron_golem").unwrap()).unwrap();
        let target = state.spawn_mob(type_id, [8.5, 201.0, 7.0], 100.0);
        let slot = state.entities.slot_of(target).unwrap();
        state.entities.row_mut(slot).health = 60.0;
        let item = state.registry.items
            .id_of(&Identifier::parse("minecraft:iron_ingot").unwrap()).unwrap();
        let index = if hand == Hand::Main { 0 } else { crate::inventory::OFF_HAND };
        let player = state.players.get_mut(&actor).unwrap();
        if creative {
            player.game_mode = crate::commands::GameMode::Creative;
        }
        player.inventory.slots[index] = mc_data::ItemStack::new(item, 3);
        let input = PlayerInput::InteractEntity {
            connection: ConnectionId(1), target, hand, location: [0.0, 1.0, 0.0],
        };
        for (health, spent) in [(85.0, 1), (100.0, 2), (100.0, 2)] {
            run_tick(&mut state, std::slice::from_ref(&input));
            assert_eq!(state.entities.row(slot).health, health);
            assert_eq!(state.players[&actor].inventory.slots[index].count,
                if creative { 3 } else { 3 - spent });
        }
    }
}

#[test]
fn natural_golem_player_damage_scales_with_difficulty() {
    use crate::survival::Difficulty;
    let mut damage = Vec::new();
    for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
        let mut state = fixture();
        run_tick(&mut state, &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::from_u128(1),
            name: "target".into(),
            permission_level: 0,
        }]);
        state.difficulty = difficulty;
        let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
        let actor_slot = state.entities.slot_of(actor).unwrap();
        state.entities.set_pos(actor_slot, [8.5, 201.0, 8.5]);
        state.players.get_mut(&actor).unwrap().vitals.health = 100.0;
        state.entities.row_mut(actor_slot).health = 100.0;
        state.entities.row_mut(actor_slot).protected_until = 0;
        let type_id = state.registry.entity_types
            .id_of(&Identifier::parse("minecraft:iron_golem").unwrap()).unwrap();
        let target = state.spawn_mob(type_id, [8.5, 201.0, 7.0], 100.0);
        let slot = state.entities.slot_of(target).unwrap();
        state.entities.row_mut(slot).last_player_hit = Some((actor, state.tick));
        for _ in 0..40 {
            run_tick(&mut state, &[]);
            if state.players[&actor].vitals.health < 100.0 {
                break;
            }
        }
        damage.push(100.0 - state.players[&actor].vitals.health);
    }
    assert!(damage[0] > 0.0 && damage[0] < damage[1] && damage[1] < damage[2],
        "difficulty must affect a natural golem's player hit: {damage:?}");
}
