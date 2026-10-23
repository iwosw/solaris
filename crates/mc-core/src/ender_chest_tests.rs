use super::*;
use crate::boundary::{Hand, PlayerInput};
use crate::tick::run_tick;
use mc_data::Identifier;

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.0),
            Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}
fn fixture() -> (TickState, EntityId, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 65, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    for index in 1..=2 {
        run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(index),
                uuid: uuid::Uuid::new_v4(),
                name: format!("owner{index}"),
                permission_level: 0,
            }],
        );
    }
    state.world.ensure_blocking(&state.desired_chunks());
    let chest = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:ender_chest").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(8, 65, 8, chest);
    state.world.set_block(9, 65, 8, chest);
    state.world.fold_overlay();
    let first = state.connections.entity_of(ConnectionId(1)).unwrap();
    let second = state.connections.entity_of(ConnectionId(2)).unwrap();
    (state, first, second)
}
fn open_at(state: &mut TickState, connection: u64, x: i32) {
    run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(connection),
            hand: Hand::Main,
            position: BlockPos { x, y: 65, z: 8 },
            face: 1,
            cursor: [0.5, 1.0, 0.5],
            sequence: 1,
        }],
    );
}
#[test]
fn ender_chest_same_player_shares_chests_but_never_other_players_or_block_storage() {
    let (mut state, first, second) = fixture();
    let item = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:diamond").unwrap())
            .unwrap(),
        7,
    );
    open_at(&mut state, 1, 8);
    state
        .players
        .get_mut(&first)
        .unwrap()
        .open
        .as_mut()
        .unwrap()
        .carried = item.clone();
    let menu = state.players[&first].open.as_ref().expect("personal menu");
    let input = PlayerInput::ContainerClick {
        connection: ConnectionId(1),
        window: menu.window,
        state_id: menu.state_id,
        slot: 0,
        button: 0,
        mode: ClickMode::Pickup,
    };
    run_tick(&mut state, &[input]);
    assert_eq!(
        contents(
            &state.players[&first].opaque_player_fields,
            &state.registry.items
        )
        .unwrap()[0],
        item
    );
    open_at(&mut state, 2, 8);
    assert!(
        contents(
            &state.players[&second].opaque_player_fields,
            &state.registry.items
        )
        .unwrap()
        .iter()
        .all(ItemStack::is_empty)
    );
    open_at(&mut state, 1, 9);
    assert_eq!(
        contents(
            &state.players[&first].opaque_player_fields,
            &state.registry.items
        )
        .unwrap()[0],
        item
    );
    assert!(!state.block_entities.contains_key(&(8, 65, 8)));
    assert!(!state.block_entities.contains_key(&(9, 65, 8)));
    state.world.set_block(8, 65, 8, state.registry.states.air);
    state.world.set_block(9, 65, 8, state.registry.states.air);
    assert_eq!(
        contents(
            &state.players[&first].opaque_player_fields,
            &state.registry.items
        )
        .unwrap()[0],
        item
    );
    let registry = Arc::clone(&state.registry);
    let player = state.players.get_mut(&first).unwrap();
    let death = player.die(&registry.item_rules(), false, false);
    assert!(
        death
            .dropped
            .iter()
            .all(|stack| stack.item_id != item.item_id)
    );
    player.respawn([8.0, 65.0, 8.0], 0.0, 0.0, Default::default());
    assert_eq!(
        contents(&player.opaque_player_fields, &registry.items).unwrap()[0],
        item
    );
    let record = crate::core::player_record_for(&state, first, state.tick).unwrap();
    let tag = crate::journal::player_to_tag(&record, &state.registry.items).unwrap();
    let recovered = crate::journal::player_from_tag(&tag, &state.registry.items).unwrap();
    assert_eq!(
        contents(&recovered.opaque_player_fields, &state.registry.items).unwrap()[0],
        item
    );
}
#[test]
fn ender_chest_rejects_corrupt_slots_and_preserves_unknown_fields() {
    let items = mc_data::items::solaris_required_items();
    let mut original = Vec::new();
    mc_nbt::write_named(
        &mut original,
        "",
        &Tag::Compound(vec![("CustomOwnerField".into(), Tag::Long(9))]),
    )
    .unwrap();
    let slots = vec![ItemStack::EMPTY; 27];
    let encoded = store(&original, &slots, &items).unwrap();
    assert!(
        root(&encoded)
            .unwrap()
            .contains(&("CustomOwnerField".into(), Tag::Long(9)))
    );
    assert_eq!(contents(&encoded, &items).unwrap(), slots);
    for bad in [
        Tag::Int(3),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: vec![Tag::Compound(vec![("Slot".into(), Tag::Byte(27))])],
        }),
    ] {
        let mut encoded = Vec::new();
        mc_nbt::write_named(
            &mut encoded,
            "",
            &Tag::Compound(vec![("EnderItems".into(), bad)]),
        )
        .unwrap();
        assert!(contents(&encoded, &items).is_err());
    }
}
