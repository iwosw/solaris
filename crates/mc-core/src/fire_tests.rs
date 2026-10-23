use std::sync::{Arc, LazyLock};

use super::*;
use crate::boundary::{ChunkPos, ConnectionId, PlayerInput};
use crate::journal::{
    CheckpointManifest, JournalEntry, JournalWriter, Replay, read_complete_batches_after,
    replay_batches,
};
use crate::native_checkpoint::tests::{read_manifest, write_manifest};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;

const POS: BlockPos = BlockPos { x: 4, y: 201, z: 4 };
const FLOOR: BlockPos = BlockPos { x: 4, y: 200, z: 4 };

// Fire rain cases require a rainy biome, independent of the terrain seed.
struct RainyTerrain(mc_worldgen::TerrainGenerator);

impl mc_world::ChunkGenerator for RainyTerrain {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = self.0.generate(pos);
        for biome in &mut chunk.biomes {
            *biome = mc_world::chunk::BiomeSection::filled(
                Identifier::parse("minecraft:plains").unwrap(),
            );
        }
        chunk
    }
}

fn state(dir: Option<std::path::PathBuf>) -> TickState {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let generator = Arc::new(RainyTerrain(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    )));
    let mut world = if let Some(dir) = dir {
        let config = crate::CoreConfig::new(dir, 42);
        ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&config).unwrap(),
            0,
            generator,
            &registry,
            BlockStateId(registry.states.air),
            [4, 201, 6],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            42,
            generator,
            &registry,
            BlockStateId(registry.states.air),
            [4, 201, 6],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .tickets
        .insert("fire-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    let stone = state.registry.states.stone;
    state.world.set_block(FLOOR.x, FLOOR.y, FLOOR.z, stone);
    state.world.fold_overlay();
    state
}

fn block(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
        .0
}

fn set(state: &mut TickState, pos: BlockPos, name: &str) {
    let id = block(state, name);
    state.world.set_block(pos.x, pos.y, pos.z, id);
}

fn join(state: &mut TickState) -> EntityId {
    let _ = run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "igniter".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [4.0, 201.0, 6.0]);
    entity
}

fn give(
    state: &mut TickState,
    entity: EntityId,
    hand: Hand,
    name: &str,
    count: i32,
    damage: Option<i32>,
) {
    let item = state
        .registry
        .items
        .id_of(&Identifier::parse(name).unwrap())
        .unwrap();
    let slot = crate::tick::held_slot_index(&state.players[&entity], hand);
    let mut held = ItemStack::new(item, count);
    held.damage = damage;
    state.players.get_mut(&entity).unwrap().inventory.slots[slot] = held;
}

fn reserve_ignition_stock(state: &mut TickState, name: &str, count: u64) {
    use mc_script::{
        ScriptInventoryEndpoint, ScriptInventoryMaterial, ScriptInventoryResourcePlan,
        ScriptInventoryWorkPortion, ScriptOperation, ScriptOperationRequest,
        ScriptOwnedInventoryOperation,
    };
    let endpoint = ScriptInventoryEndpoint::PlayerInventory { player_id: 1 };
    let image = crate::plugin_inventories::load(state, "ignition-test", &endpoint).unwrap();
    let request = ScriptOperationRequest::try_new(
        "reserve-ignition",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Reserve {
                operation_id: "reserve-ignition".into(),
                endpoint,
                resource_plan: ScriptInventoryResourcePlan::new(vec![
                    ScriptInventoryWorkPortion::new(
                        1,
                        vec![ScriptInventoryMaterial::new(name.into(), count)],
                    ),
                ]),
                expected_revision: image.fence,
            },
        },
    )
    .unwrap();
    assert_eq!(
        crate::plugin_inventories::execute(state, "ignition-test", &request).failure(),
        None
    );
}

fn input(hand: Hand, position: BlockPos) -> PlayerInput {
    PlayerInput::UseItemOn {
        connection: ConnectionId(1),
        position,
        hand,
        face: 1,
        cursor: [0.5, 0.5, 0.5],
        sequence: 1,
    }
}

fn due(state: &mut TickState) -> Vec<(BlockPos, u32)> {
    let deadline = state
        .block_ticks
        .image()
        .chunks
        .iter()
        .flat_map(|chunk| &chunk.ticks)
        .map(|tick| tick.trigger_tick)
        .min()
        .unwrap();
    state.tick = deadline;
    let mut writes = Vec::new();
    crate::redstone::run(state, &mut writes, &mut Vec::new());
    writes
}

fn age(state: &TickState, pos: BlockPos) -> Option<u8> {
    at(state, pos).and_then(|block| property(block, "age")?.parse().ok())
}

#[test]
fn native_use_ignites_debits_only_success_and_publishes_block_and_inventory() {
    let mut state = state(None);
    let entity = join(&mut state);
    give(
        &mut state,
        entity,
        Hand::Main,
        "minecraft:fire_charge",
        2,
        None,
    );
    let (output, drains, _) = run_tick(&mut state, &[input(Hand::Main, FLOOR)]);
    assert_eq!(age(&state, POS), Some(0));
    assert_eq!(state.players[&entity].inventory.slots[0].count, 1);
    assert!(
        output
            .changes
            .blocks
            .iter()
            .any(|change| change.position == POS)
    );
    // Vanilla ServerPlayer$1.sendSlotChange sends SetSlot, not the
    // initial full image (local 26.1.2 client.jar javap).
    assert!(
        drains
            .iter()
            .filter(|(connection, _)| *connection == ConnectionId(1))
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(
                change,
                crate::boundary::PlayerChange::MenuSlot {
                    window: 0,
                    slot: 36,
                    item: Some(item),
                    ..
                } if *item == state.players[&entity].inventory.slots[0]
            ))
    );
    let image = state.block_ticks.image();
    assert_eq!(image.chunks[0].ticks.len(), 1);
    let _ = run_tick(&mut state, &[input(Hand::Main, FLOOR)]);
    assert_eq!(state.players[&entity].inventory.slots[0].count, 1);
    assert_eq!(state.block_ticks.image(), image);
}

#[test]
fn flint_wears_either_hand_breaks_at_max_and_creative_never_pays() {
    for hand in [Hand::Main, Hand::Off] {
        let mut state = state(None);
        let entity = join(&mut state);
        give(
            &mut state,
            entity,
            hand,
            "minecraft:flint_and_steel",
            1,
            Some(62),
        );
        let _ = run_tick(&mut state, &[input(hand, FLOOR)]);
        let slot = crate::tick::held_slot_index(&state.players[&entity], hand);
        assert_eq!(
            state.players[&entity].inventory.slots[slot].damage,
            Some(63)
        );
        let _ = run_tick(&mut state, &[input(hand, FLOOR)]);
        assert_eq!(
            state.players[&entity].inventory.slots[slot].damage,
            Some(63)
        );
        set(&mut state, POS, "minecraft:air");
        let _ = run_tick(&mut state, &[input(hand, FLOOR)]);
        assert!(state.players[&entity].inventory.slots[slot].is_empty());
        state.players.get_mut(&entity).unwrap().game_mode = crate::commands::GameMode::Creative;
        give(
            &mut state,
            entity,
            hand,
            "minecraft:flint_and_steel",
            1,
            Some(4),
        );
        set(&mut state, POS, "minecraft:air");
        let _ = run_tick(&mut state, &[input(hand, FLOOR)]);
        assert_eq!(state.players[&entity].inventory.slots[slot].damage, Some(4));
        give(&mut state, entity, hand, "minecraft:fire_charge", 2, None);
        set(&mut state, POS, "minecraft:air");
        let _ = run_tick(&mut state, &[input(hand, FLOOR)]);
        assert_eq!(state.players[&entity].inventory.slots[slot].count, 2);
    }
}

#[test]
fn reserved_ignition_costs_refuse_before_fire_or_tnt_effects() {
    for (name, damage) in [
        ("minecraft:fire_charge", None),
        ("minecraft:flint_and_steel", Some(63)),
    ] {
        for tnt in [false, true] {
            let mut state = state(None);
            let entity = join(&mut state);
            give(&mut state, entity, Hand::Main, name, 1, damage);
            reserve_ignition_stock(&mut state, name, 1);
            if tnt {
                set(&mut state, POS, "minecraft:tnt");
            }
            let held = state.players[&entity].inventory.slots[0].clone();
            let before = state.world.block_at(POS.x, POS.y, POS.z);
            let _ = run_tick(
                &mut state,
                &[input(Hand::Main, if tnt { POS } else { FLOOR })],
            );
            assert_eq!(state.players[&entity].inventory.slots[0], held);
            assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), before);
            assert!(
                state
                    .entities
                    .slots()
                    .all(|slot| state.entities.row(slot).tnt.is_none())
            );
            assert!(age(&state, POS).is_none());
        }
    }
}

#[test]
fn ignition_can_spend_surplus_stock_above_aggregate_floor() {
    for (name, damage) in [
        ("minecraft:fire_charge", None),
        ("minecraft:flint_and_steel", Some(63)),
    ] {
        let mut state = state(None);
        let entity = join(&mut state);
        give(&mut state, entity, Hand::Main, name, 1, damage);
        let held = state.players[&entity].inventory.slots[0].clone();
        state.players.get_mut(&entity).unwrap().inventory.slots[9] = held.clone();
        reserve_ignition_stock(&mut state, name, 1);
        let _ = run_tick(&mut state, &[input(Hand::Main, FLOOR)]);
        assert_eq!(age(&state, POS), Some(0));
        assert!(state.players[&entity].inventory.slots[0].is_empty());
        assert_eq!(state.players[&entity].inventory.slots[9], held);
    }
}

#[test]
fn reserved_flint_can_wear_without_breaking_and_creative_pays_nothing() {
    let mut state = state(None);
    let entity = join(&mut state);
    give(
        &mut state,
        entity,
        Hand::Main,
        "minecraft:flint_and_steel",
        1,
        Some(62),
    );
    reserve_ignition_stock(&mut state, "minecraft:flint_and_steel", 1);
    let _ = run_tick(&mut state, &[input(Hand::Main, FLOOR)]);
    assert_eq!(age(&state, POS), Some(0));
    assert_eq!(state.players[&entity].inventory.slots[0].damage, Some(63));
    set(&mut state, POS, "minecraft:air");
    let _ = run_tick(&mut state, &[input(Hand::Main, FLOOR)]);
    assert!(age(&state, POS).is_none());
    assert_eq!(state.players[&entity].inventory.slots[0].damage, Some(63));
    state.players.get_mut(&entity).unwrap().game_mode = crate::commands::GameMode::Creative;
    let _ = run_tick(&mut state, &[input(Hand::Main, FLOOR)]);
    assert_eq!(age(&state, POS), Some(0));
    assert_eq!(state.players[&entity].inventory.slots[0].damage, Some(63));
}

#[test]
fn helper_refuses_unloaded_solid_wet_and_unsupported_cells_without_queueing() {
    let mut state = state(None);
    assert!(!ignite_block(&mut state, BlockPos { x: 1000, ..POS }));
    assert!(!ignite_block(&mut state, FLOOR));
    assert!(!ignite_block(&mut state, BlockPos { y: 220, ..POS }));
    set(&mut state, POS, "minecraft:water");
    assert!(!ignite_block(&mut state, POS));
    assert!(state.block_ticks.image().chunks.is_empty());
}

#[test]
fn campfires_and_candles_light_in_place_and_refuse_second_change() {
    let mut state = state(None);
    for name in [
        "minecraft:campfire",
        "minecraft:soul_campfire",
        "minecraft:candle",
        "minecraft:red_candle_cake",
    ] {
        let definition = state
            .registry
            .blocks
            .block(&Identifier::parse(name).unwrap())
            .unwrap();
        let unlit = definition
            .states
            .iter()
            .copied()
            .find(|id| {
                let candidate = state.registry.blocks.by_id(*id).unwrap();
                property(candidate, "lit") == Some("false")
                    && property(candidate, "waterlogged") != Some("true")
            })
            .unwrap();
        state.world.set_block(POS.x, POS.y, POS.z, unlit.0);
        assert!(ignite_block(&mut state, POS), "{name}");
        assert_eq!(property(at(&state, POS).unwrap(), "lit"), Some("true"));
        assert!(!ignite_block(&mut state, POS));
    }
}

#[test]
fn radius_unlimited_zero_strict_distance_and_spectators_gate_age_and_spread() {
    let mut state = state(None);
    let entity = join(&mut state);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [5.0, 201.0, 4.0]);
    assert!(ignite_block(&mut state, POS));
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "1".into());
    assert!(!spread_enabled(&state, POS));
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), Some(0));
    state.entities.set_pos(slot, [4.99, 201.0, 4.0]);
    assert!(spread_enabled(&state, POS));
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), Some(1));
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "0".into());
    state.entities.set_pos(slot, [4.0, 201.0, 4.0]);
    assert!(!spread_enabled(&state, POS));
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), Some(1));
    state.gamerules.insert(
        "fire_spread_radius_around_player".into(),
        "2147483647".into(),
    );
    assert!(spread_enabled(&state, POS));
    state.players.get_mut(&entity).unwrap().game_mode = crate::commands::GameMode::Spectator;
    assert!(!spread_enabled(&state, POS));
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "-1".into());
    assert!(spread_enabled(&state, POS));
    state.players.clear();
    assert!(spread_enabled(&state, POS));
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), Some(2));
}

#[test]
fn scheduled_legacy_age_spreads_to_one_fuel_and_extinguishes_at_fifteen() {
    let mut state = state(None);
    let _ = join(&mut state);
    let fuel = BlockPos { x: 5, ..POS };
    set(&mut state, fuel, "minecraft:oak_planks");
    assert!(ignite_block(&mut state, POS));
    let tick = (1..100)
        .find(|tick| {
            state.tick = *tick;
            seed(&state, POS).is_multiple_of(3)
        })
        .unwrap();
    state.tick = tick;
    let mut writes = Vec::new();
    scheduled_tick(&mut state, POS, &mut writes, &mut Vec::new());
    assert_eq!(age(&state, POS), Some(1));
    assert_eq!(age(&state, fuel), Some(0));
    assert_eq!(writes.len(), 2);
    let last = normal_state(&state, POS, 15);
    state.world.set_block(POS.x, POS.y, POS.z, last);
    scheduled_tick(&mut state, POS, &mut writes, &mut Vec::new());
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
}

#[test]
fn fire_never_spreads_into_a_protected_zone_but_still_ages() {
    let mut state = state(None);
    let _ = join(&mut state);
    let fuel = BlockPos { x: 5, ..POS };
    set(&mut state, fuel, "minecraft:oak_planks");
    state.plugins.set_domain(
        "towns",
        "player-zones",
        serde_json::json!({
            "claim": {
                "dimension": "minecraft:overworld",
                "minimum": [5.0, 0.0, 4.0],
                "maximum": [5.0, 300.0, 4.0],
                "allowed_actors": [uuid::Uuid::new_v4().simple().to_string()],
                "public_use": false
            }
        }),
    );
    crate::plugin_player::restore(&mut state).unwrap();
    assert!(ignite_block(&mut state, POS));
    let tick = (1..100)
        .find(|tick| {
            state.tick = *tick;
            seed(&state, POS).is_multiple_of(3)
        })
        .unwrap();
    state.tick = tick;
    let mut writes = Vec::new();
    scheduled_tick(&mut state, POS, &mut writes, &mut Vec::new());
    assert_eq!(age(&state, POS), Some(1), "the fire outside still ages");
    assert_eq!(age(&state, fuel), None, "the claimed planks never caught");
    assert_eq!(writes.len(), 1);
}

#[test]
fn support_water_and_exposed_rain_extinguish_but_roof_and_eternal_support_survive() {
    let mut state = state(None);
    let _ = join(&mut state);
    assert!(ignite_block(&mut state, POS));
    set(&mut state, FLOOR, "minecraft:air");
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), None);
    set(&mut state, FLOOR, "minecraft:stone");
    assert!(ignite_block(&mut state, POS));
    set(&mut state, BlockPos { x: 5, ..POS }, "minecraft:water");
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), None);
    set(&mut state, BlockPos { x: 5, ..POS }, "minecraft:air");
    assert!(ignite_block(&mut state, POS));
    state.rain_level = 1.0;
    assert!(raining_at(&state, POS));
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), None);
    for x in 3..=5 {
        for z in 3..=5 {
            set(&mut state, BlockPos { x, y: 204, z }, "minecraft:stone");
        }
    }
    assert!(ignite_block(&mut state, POS));
    assert!(!raining_at(&state, POS));
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), Some(1));
    for x in 3..=5 {
        for z in 3..=5 {
            set(&mut state, BlockPos { x, y: 204, z }, "minecraft:air");
        }
    }
    set(&mut state, FLOOR, "minecraft:netherrack");
    let last = normal_state(&state, POS, 15);
    state.world.set_block(POS.x, POS.y, POS.z, last);
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), Some(15));
    set(&mut state, POS, "minecraft:air");
    set(&mut state, FLOOR, "minecraft:soul_soil");
    assert!(ignite_block(&mut state, POS));
    assert_eq!(at(&state, POS).unwrap().block.id.path(), "soul_fire");
    let _ = due(&mut state);
    assert_eq!(at(&state, POS).unwrap().block.id.path(), "soul_fire");
    set(&mut state, FLOOR, "minecraft:stone");
    let _ = due(&mut state);
    assert_eq!(age(&state, POS), None);
    assert_eq!(at(&state, POS).unwrap().block.id.path(), "air");
}

#[test]
fn successful_ignition_and_debit_share_wal_queue_survives_replay() {
    let dir = std::env::temp_dir().join(format!("fire-wal-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    let entity = join(&mut original);
    give(
        &mut original,
        entity,
        Hand::Main,
        "minecraft:fire_charge",
        2,
        None,
    );
    let (output, _, _) = run_tick(&mut original, &[input(Hand::Main, FLOOR)]);
    let entries = crate::core::journal_entries_for_tick(&mut original, &output);
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, JournalEntry::Block { pos, .. } if *pos == [4,201,4]))
    );
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, JournalEntry::BlockTicks { .. }))
    );
    assert!(entries.iter().any(|entry| matches!(entry,
        JournalEntry::Player { inventory, .. } if inventory[0].count == 1
            && original.registry.items.name_of(inventory[0].item_id)
                .is_some_and(|name| name.as_str() == "minecraft:fire_charge"))));
    let image = original.block_ticks.image();
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    let replay = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&dir.join("journal"), 0).unwrap(),
    );
    assert_eq!(replay.block_ticks, image);
    let mut restored = state(Some(dir.clone()));
    for ((x, y, z), id) in replay.blocks {
        restored.world.set_block(x, y, z, id);
    }
    restored.block_ticks = crate::block_ticks::BlockTicks::from_image(replay.block_ticks, None);
    let _ = join(&mut restored);
    let _ = due(&mut restored);
    assert_eq!(age(&restored, POS), Some(1));
    drop(writer);
    drop(original);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checkpoint_fire_block_and_vanilla_queue_restore_deadline_without_duplicates() {
    let dir = std::env::temp_dir().join(format!("fire-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    assert!(ignite_block(&mut original, POS));
    original.world.fold_overlay();
    let before = original.block_ticks.image();
    let deadline = before.chunks[0].ticks[0].trigger_tick;
    let absent = original
        .block_ticks
        .checkpoint(&mut original.world, original.tick);
    let mut capture = original.world.capture_dirty(&Default::default());
    original
        .world
        .capture_scheduled_block_ticks(&mut capture, absent);
    assert!(
        !crate::world::world_tests::write_capture(&capture)
            .unwrap()
            .is_empty()
    );
    write_manifest(
        &crate::CoreConfig::new(dir.clone(), 42),
        &CheckpointManifest {
            tick: original.tick,
            block_ticks: before.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(original);
    let mut restored = state(Some(dir.clone()));
    restored.block_ticks = crate::block_ticks::BlockTicks::from_image(
        read_manifest(&crate::CoreConfig::new(dir.clone(), 42))
            .unwrap()
            .block_ticks,
        None,
    );
    let chunk = restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    assert_eq!(
        chunk.scheduled_block_ticks()[0].block.as_str(),
        "minecraft:fire"
    );
    assert_eq!(chunk.scheduled_block_ticks()[0].trigger_tick, deadline);
    restored.block_ticks.hydrate_chunk(chunk, 0);
    assert_eq!(restored.block_ticks.image(), before);
    assert!(
        restored
            .block_ticks
            .drain_loaded(deadline - 1, 1024, &restored.world)
            .is_empty()
    );
    let _ = join(&mut restored);
    let _ = due(&mut restored);
    assert_eq!(age(&restored, POS), Some(1));
    assert_eq!(restored.block_ticks.image().chunks[0].ticks.len(), 1);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn positive_radius_bounds_the_spread_target_and_unlimited_restores_spread() {
    let mut state = state(None);
    let entity = join(&mut state);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [4.0, 201.0, 4.0]);
    let fuel = BlockPos { x: 5, ..POS };
    set(&mut state, fuel, "minecraft:oak_planks");
    let original_fuel = state.world.block_at(fuel.x, fuel.y, fuel.z).unwrap();
    assert!(ignite_block(&mut state, POS));
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "1".into());
    let tick = (1..100)
        .find(|tick| {
            state.tick = *tick;
            seed(&state, POS).is_multiple_of(3)
        })
        .unwrap();
    state.tick = tick;
    let mut writes = Vec::new();
    scheduled_tick(&mut state, POS, &mut writes, &mut Vec::new());
    assert_eq!(age(&state, POS), Some(1));
    assert_eq!(
        state.world.block_at(fuel.x, fuel.y, fuel.z),
        Some(original_fuel)
    );
    assert_eq!(writes.len(), 1);
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "-1".into());
    state.players.clear();
    scheduled_tick(&mut state, POS, &mut writes, &mut Vec::new());
    assert_eq!(age(&state, fuel), Some(0));
}

#[test]
fn queued_fire_cannot_resurrect_a_replaced_block_after_restart() {
    let mut original = state(None);
    assert!(ignite_block(&mut original, POS));
    let image = original.block_ticks.image();
    let mut restored = state(None);
    restored.block_ticks = crate::block_ticks::BlockTicks::from_image(image, None);
    set(&mut restored, POS, "minecraft:stone");
    let writes = due(&mut restored);
    assert!(writes.is_empty());
    assert_eq!(
        restored.world.block_at(POS.x, POS.y, POS.z),
        Some(restored.registry.states.stone)
    );
    assert!(
        restored
            .block_ticks
            .image()
            .chunks
            .iter()
            .all(|chunk| chunk.ticks.is_empty())
    );
}

#[test]
fn lava_ignition_keeps_seed_independent_fuel_admission_and_success_only_writes() {
    let mut state = state(None);
    let lava = BlockPos { x: 2, ..POS };
    let target = BlockPos { x: 3, ..POS };
    set(&mut state, lava, "minecraft:lava");
    assert!(!has_lava_fuel(&state, lava));
    set(&mut state, POS, "minecraft:oak_planks");
    assert!(has_lava_fuel(&state, lava));
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "-1".into());
    let refused = (1..100)
        .find(|tick| {
            state.tick = *tick;
            !seed(&state, lava).is_multiple_of(3)
        })
        .unwrap();
    state.tick = refused;
    let mut writes = Vec::new();
    assert!(!lava_tick(&mut state, lava, &mut writes));
    assert!(writes.is_empty());
    assert!(has_lava_fuel(&state, lava));
    let admitted = (1..100)
        .find(|tick| {
            state.tick = *tick;
            seed(&state, lava) != 0 && seed(&state, lava).is_multiple_of(3)
        })
        .unwrap();
    state.tick = admitted;
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "0".into());
    assert!(!lava_tick(&mut state, lava, &mut writes));
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "-1".into());
    state.rain_level = 1.0;
    assert!(!lava_tick(&mut state, lava, &mut writes));
    state.rain_level = 0.0;
    assert!(lava_tick(&mut state, lava, &mut writes));
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].0, target);
    assert_eq!(age(&state, target), Some(0));
    assert_eq!(
        state.block_ticks.image().chunks[0].ticks[0].block.as_str(),
        "minecraft:fire"
    );
}

#[test]
fn ignition_items_prime_tnt_and_pay_once_without_placing_fire() {
    for item in ["minecraft:flint_and_steel", "minecraft:fire_charge"] {
        let mut state = state(None);
        let entity = join(&mut state);
        give(
            &mut state,
            entity,
            Hand::Main,
            item,
            if item == "minecraft:fire_charge" {
                2
            } else {
                1
            },
            None,
        );
        set(&mut state, POS, "minecraft:tnt");
        let (output, _, _) = run_tick(&mut state, &[input(Hand::Main, POS)]);
        assert_eq!(
            state.world.block_at(POS.x, POS.y, POS.z),
            Some(state.registry.states.air)
        );
        assert!(!output.changes.entities_spawned.is_empty());
        let held = &state.players[&entity].inventory.slots[0];
        if item == "minecraft:flint_and_steel" {
            assert_eq!(held.damage, Some(1));
        } else {
            assert_eq!(held.count, 1);
        }
        assert!(
            state
                .block_ticks
                .image()
                .chunks
                .iter()
                .all(|chunk| chunk.ticks.is_empty())
        );
    }
}

#[test]
fn scheduled_fire_primes_tnt_with_same_block_phase_air_postimage() {
    let mut state = state(None);
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "-1".into());
    let target = BlockPos { x: 5, ..POS };
    set(&mut state, target, "minecraft:tnt");
    assert!(ignite_block(&mut state, POS));
    let tick = (50..100)
        .find(|tick| {
            state.tick = *tick;
            seed(&state, POS).is_multiple_of(3)
        })
        .unwrap();
    state.tick = tick;
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(
        output.changes.blocks.iter().any(|change| {
            change.position == target && change.state == state.registry.states.air
        })
    );
    assert!(!output.changes.entities_spawned.is_empty());
    assert!(
        !state
            .merge_block_writes
            .iter()
            .any(|(cell, _)| *cell == target)
    );
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(state.registry.states.air)
    );
}

#[test]
fn open_world_ceiling_fire_is_rained_on_like_vanilla() {
    let mut state = state(None);
    let _ = join(&mut state);
    let pos = BlockPos { y: 319, ..POS };
    let stone = state.registry.states.stone;
    state.world.set_block(pos.x, pos.y - 1, pos.z, stone);
    state.world.fold_overlay();
    state.rain_level = 1.0;
    assert!(
        raining_at(&state, pos),
        "an open column remains exposed at y319"
    );
    assert!(ignite_block(&mut state, pos));
    let mut writes = Vec::new();
    // Rain's probabilistic extinction eventually reaches this exact ceiling cell.
    for tick in 0..128 {
        state.tick = tick;
        scheduled_tick(&mut state, pos, &mut writes, &mut Vec::new());
        if age(&state, pos).is_none() {
            break;
        }
    }
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(state.registry.states.air)
    );
}

#[test]
fn native_flint_and_steel_spreads_into_air_over_wool_within_client_window() {
    for wool in [false, true] {
        let mut state = state(None);
        let entity = join(&mut state);
        state.gamerules.insert("spawn_mobs".into(), "false".into());
        state
            .gamerules
            .insert("random_tick_speed".into(), "0".into());
        state
            .gamerules
            .insert("advance_weather".into(), "false".into());
        state.rain_level = 0.0;
        for x in 2..=6 {
            for z in 2..=6 {
                set(&mut state, BlockPos { x, y: 200, z }, "minecraft:stone");
            }
        }
        if wool {
            // Same W/E wool and southern stone arrangement as Java QA.
            for (x, z) in [(3, 4), (4, 4), (5, 4), (3, 5), (5, 5)] {
                set(
                    &mut state,
                    BlockPos { x, y: 200, z },
                    "minecraft:white_wool",
                );
            }
        }
        state.world.fold_overlay();
        let neighbours = [
            BlockPos { x: 3, ..POS },
            BlockPos { x: 5, ..POS },
            BlockPos { z: 5, ..POS },
        ];
        assert!(
            neighbours
                .iter()
                .all(|p| state.world.block_at(p.x, p.y, p.z) == Some(state.registry.states.air))
        );
        assert_eq!(
            crate::command_rules::integer(&state, "fire_spread_radius_around_player"),
            128
        );
        give(
            &mut state,
            entity,
            Hand::Main,
            "minecraft:flint_and_steel",
            1,
            None,
        );
        let placed_at = state.tick;
        let _ = run_tick(&mut state, &[input(Hand::Main, FLOOR)]);
        assert_eq!(age(&state, POS), Some(0));
        assert_eq!(state.players[&entity].inventory.slots[0].damage, Some(1));
        let queue = state.block_ticks.image();
        let fire_tick = queue
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.ticks)
            .find(|tick| {
                tick.pos == [POS.x, POS.y, POS.z] && tick.block.as_str() == "minecraft:fire"
            })
            .unwrap();
        assert!((30..40).contains(&(fire_tick.trigger_tick - placed_at)));
        let mut first = [None; 3];
        let mut publications = 0;
        for _ in 0..900 {
            let (output, _, _) = run_tick(&mut state, &[]);
            for change in &output.changes.blocks {
                if state.registry.fire_states.fact(change.state).age.is_none() {
                    continue;
                }
                if let Some(index) = neighbours.iter().position(|pos| *pos == change.position) {
                    first[index].get_or_insert(output.tick - placed_at);
                    publications += 1;
                }
            }
        }
        eprintln!(
            "fire-client-air-spread wool={wool} initial_delay={} first_w_e_s={first:?} fire_postimages={publications} window_ticks=900",
            fire_tick.trigger_tick - placed_at
        );
        if wool {
            assert!(
                first.iter().any(Option::is_some),
                "native use must ignite a previously-air neighbour"
            );
        } else {
            assert_eq!(
                first, [None; 3],
                "sturdy nonflammable support alone does not admit air spread"
            );
        }
    }
}

#[test]
fn vanilla_stairs_and_carpet_catch_and_burn_but_stone_does_not() {
    for (name, burns) in [
        ("minecraft:oak_stairs", true),
        ("minecraft:white_carpet", true),
        ("minecraft:stone", false),
    ] {
        let mut state = state(None);
        state
            .gamerules
            .insert("fire_spread_radius_around_player".into(), "-1".into());
        let target = BlockPos { x: 5, ..POS };
        set(
            &mut state,
            BlockPos {
                y: target.y - 1,
                ..target
            },
            "minecraft:stone",
        );
        set(&mut state, target, name);
        let original = state.world.block_at(target.x, target.y, target.z).unwrap();
        assert_eq!(fact(&state, target).fuel, burns, "{name} ignition");
        assert_eq!(fact(&state, target).burn, burns, "{name} combustion");
        assert!(ignite_block(&mut state, POS));
        let mut caught = false;
        for _ in 0..16 {
            let writes = due(&mut state);
            if writes.iter().any(|(pos, id)| {
                *pos == target && state.registry.fire_states.fact(*id).age.is_some()
            }) {
                caught = true;
                break;
            }
            if age(&state, POS).is_none() {
                break;
            }
        }
        assert_eq!(caught, burns, "{name} scheduled spread");
        if burns {
            assert_eq!(age(&state, target), Some(0));
            assert_ne!(
                state.world.block_at(target.x, target.y, target.z),
                Some(original),
                "{name} fuel was consumed"
            );
        } else {
            assert_eq!(
                state.world.block_at(target.x, target.y, target.z),
                Some(original)
            );
        }
    }
}

#[test]
fn vanilla_flammability_still_excludes_waterlogged_states() {
    let state = state(None);
    let name = Identifier::parse("minecraft:oak_stairs").unwrap();
    let block = state.registry.blocks.block(&name).unwrap();
    let mut dry = 0;
    let mut wet = 0;
    for id in &block.states {
        let block = state.registry.blocks.by_id(*id).unwrap();
        let fact = state.registry.fire_states.fact(id.0);
        let waterlogged = property(block, "waterlogged") == Some("true");
        assert_eq!(fact.fuel, !waterlogged);
        assert_eq!(fact.burn, !waterlogged);
        if waterlogged {
            wet += 1;
        } else {
            dry += 1;
        }
    }
    assert!(dry > 0 && wet > 0);
}
