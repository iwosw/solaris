//! Water-cauldron washing: dye, banner layers and shulker color.
use std::sync::Arc;

use crate::boundary::{BlockPos, ConnectionId, EntityId, Hand, PlayerInput};
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;
use mc_data::{Identifier, ItemStack};
use mc_world::BlockStateId;

const POS: BlockPos = BlockPos { x: 1, y: 200, z: 0 };

struct EmptyWorld(u32);
impl mc_world::ChunkGenerator for EmptyWorld {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(
            pos,
            BlockStateId(self.0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        chunk.status = "minecraft:full".into();
        chunk
    }
}

fn fixture(level: u8) -> (TickState, EntityId) {
    let registry = Arc::new(crate::tick_tests::full_registry());
    let world = ChunkWorld::new(
        42,
        Arc::new(EmptyWorld(registry.states.air)),
        &registry,
        BlockStateId(registry.states.air),
        [0, 200, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 1, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "cauldron".into(),
            permission_level: 0,
        }],
    );
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [0.5, 200.0, 0.5]);
    for x in -3..=3 {
        for z in -3..=3 {
            state
                .world
                .set_block(x, 199, z, state.registry.states.stone);
        }
    }
    let cauldron = if level == 0 {
        state
            .registry
            .blocks
            .block(&Identifier::parse("minecraft:cauldron").unwrap())
            .unwrap()
            .default
            .0
    } else {
        state
            .registry
            .blocks
            .by_name_and_props(
                &Identifier::parse("minecraft:water_cauldron").unwrap(),
                &[("level".into(), level.to_string())],
            )
            .unwrap()
            .0
    };
    state.world.set_block(POS.x, POS.y, POS.z, cauldron);
    state.world.fold_overlay();
    (state, entity)
}

fn give(state: &mut TickState, entity: EntityId, stack: ItemStack) {
    let slot =
        crate::tick::held_slot_index(state.players.get(&entity).expect("player"), Hand::Main);
    state.players.get_mut(&entity).unwrap().inventory.slots[slot] = stack;
}

fn use_input() -> PlayerInput {
    PlayerInput::UseItemOn {
        connection: ConnectionId(1),
        position: POS,
        hand: Hand::Main,
        face: 1,
        cursor: [0.5; 3],
        sequence: 1,
    }
}

fn level(state: &TickState) -> u8 {
    let block = state
        .registry
        .blocks
        .by_id(BlockStateId(
            state.world.block_at(POS.x, POS.y, POS.z).unwrap(),
        ))
        .unwrap();
    if block.block.id.as_str() == "minecraft:cauldron" {
        return 0;
    }
    block
        .properties
        .iter()
        .find(|(key, _)| key == "level")
        .unwrap()
        .1
        .parse()
        .unwrap()
}

fn dyed_chestplate(state: &TickState) -> ItemStack {
    let id = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:leather_chestplate").unwrap())
        .unwrap();
    let mut stack = ItemStack::new(id, 1);
    let mut value = Vec::new();
    mc_nbt::write_network_value(&mut value, &mc_nbt::Tag::Int(0xFF0000)).unwrap();
    stack
        .extended_mut()
        .preserved_components
        .push(mc_data::item_stack::PreservedItemComponent {
            name: "minecraft:dyed_color".into(),
            value,
            baseline: None,
        });
    stack
}

#[test]
fn dyed_chestplate_loses_its_color_for_one_water_level() {
    let (mut state, entity) = fixture(3);
    let dirty = dyed_chestplate(&state);
    give(&mut state, entity, dirty);
    let _ = run_tick(&mut state, &[use_input()]);
    assert_eq!(level(&state), 2);
    let held = state.players[&entity].inventory.slots[0].clone();
    assert!(
        !held
            .extended()
            .preserved_components
            .iter()
            .any(|component| component.name == "minecraft:dyed_color"),
        "dye washed off"
    );
}

#[test]
fn banner_loses_only_its_last_layer_per_use() {
    let (mut state, entity) = fixture(3);
    let id = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:white_banner").unwrap())
        .unwrap();
    let mut stack = ItemStack::new(id, 1);
    for (pattern, color) in [("stripe_bottom", 14), ("creeper", 11)] {
        stack
            .extended_mut()
            .banner_patterns
            .push(mc_data::item_stack::BannerPattern {
                pattern: Identifier::parse(format!("minecraft:{pattern}")).unwrap(),
                color,
            });
    }
    give(&mut state, entity, stack);
    let _ = run_tick(&mut state, &[use_input()]);
    assert_eq!(level(&state), 2);
    let held = state.players[&entity].inventory.slots[0].clone();
    assert_eq!(held.extended().banner_patterns.len(), 1);
    assert_eq!(
        held.extended().banner_patterns[0].pattern.as_str(),
        "minecraft:stripe_bottom"
    );
}

#[test]
fn colored_shulker_washes_to_plain_with_contents_kept() {
    let (mut state, entity) = fixture(2);
    let id = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:red_shulker_box").unwrap())
        .unwrap();
    let mut stack = ItemStack::new(id, 1);
    stack.custom_name = Some("Treasure".into());
    give(&mut state, entity, stack);
    let _ = run_tick(&mut state, &[use_input()]);
    assert_eq!(level(&state), 1);
    let held = state.players[&entity].inventory.slots[0].clone();
    assert_eq!(
        state.registry.items.name_of(held.item_id).unwrap().as_str(),
        "minecraft:shulker_box"
    );
    assert_eq!(held.custom_name.as_deref(), Some("Treasure"));
}

#[test]
fn empty_cauldron_and_undyeable_items_consume_nothing() {
    let (mut state, entity) = fixture(0);
    let dirty = dyed_chestplate(&state);
    give(&mut state, entity, dirty);
    let _ = run_tick(&mut state, &[use_input()]);
    assert_eq!(level(&state), 0);
    assert!(
        state.players[&entity].inventory.slots[0]
            .extended()
            .preserved_components
            .iter()
            .any(|component| component.name == "minecraft:dyed_color"),
        "no water, no wash"
    );
    let (mut state, entity) = fixture(3);
    let stone = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    give(&mut state, entity, ItemStack::new(stone, 1));
    let _ = run_tick(&mut state, &[use_input()]);
    assert_eq!(level(&state), 3, "plain stone consumes no water");
}
