use super::*;
use crate::boundary::PlayerInput;
use crate::tick::run_tick;
use std::sync::Arc;
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
fn fixture() -> (TickState, EntityId) {
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
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "Author".into(),
            permission_level: 0,
        }],
    );
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    let mut book = mc_data::ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:writable_book").unwrap())
            .unwrap(),
        1,
    );
    book.custom_name = Some("kept name".into());
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = book;
    (state, actor)
}
#[test]
fn book_edit_sign_and_player_persistence_preserve_author_and_pages() {
    let (mut state, actor) = fixture();
    let pages = vec!["literal @a text".into(), "second page".into()];
    run_tick(
        &mut state,
        &[PlayerInput::EditBook {
            connection: ConnectionId(1),
            slot: 0,
            pages: pages.clone(),
            title: None,
        }],
    );
    assert_eq!(
        state.players[&actor].inventory.slots[0]
            .extended()
            .preserved_components[0]
            .name,
        "minecraft:writable_book_content"
    );
    run_tick(
        &mut state,
        &[PlayerInput::EditBook {
            connection: ConnectionId(1),
            slot: 0,
            pages: pages.clone(),
            title: Some("Title".into()),
        }],
    );
    let book = &state.players[&actor].inventory.slots[0];
    assert_eq!(
        state.registry.items.name_of(book.item_id).unwrap().as_str(),
        "minecraft:written_book"
    );
    assert_eq!(book.custom_name.as_deref(), Some("kept name"));
    let component = &book.extended().preserved_components[0];
    assert_eq!(component.name, "minecraft:written_book_content");
    let Tag::Compound(fields) =
        mc_nbt::read_network_value(&mut component.value.as_slice()).unwrap()
    else {
        panic!("book compound")
    };
    assert_eq!(
        mc_data::item_nbt::field(&fields, "author"),
        Some(&Tag::String("Author".into()))
    );
    assert_eq!(
        mc_data::item_nbt::field(&fields, "generation"),
        Some(&Tag::Int(0))
    );
    let record = crate::core::player_record_for(&state, actor, state.tick).unwrap();
    let tag = crate::journal::player_to_tag(&record, &state.registry.items).unwrap();
    let recovered = crate::journal::player_from_tag(&tag, &state.registry.items).unwrap();
    assert_eq!(&recovered.inventory[0], book);
    let before = book.clone();
    edit(
        &mut state,
        ConnectionId(1),
        0,
        &["rewrite signed book".into()],
        None,
    );
    assert_eq!(state.players[&actor].inventory.slots[0], before);
}
#[test]
fn book_edit_refuses_invalid_slot_page_and_spectator_without_mutation() {
    let (mut state, actor) = fixture();
    let before = state.players[&actor].inventory.slots[0].clone();
    for slot in [-1, 9, 39, 41] {
        edit(&mut state, ConnectionId(1), slot, &["text".into()], None);
    }
    edit(&mut state, ConnectionId(1), 0, &["x".repeat(1025)], None);
    edit(
        &mut state,
        ConnectionId(1),
        0,
        &vec![String::new(); 101],
        None,
    );
    edit(&mut state, ConnectionId(1), 0, &[], Some(&"x".repeat(33)));
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Spectator;
    edit(&mut state, ConnectionId(1), 0, &["text".into()], None);
    assert_eq!(state.players[&actor].inventory.slots[0], before);
}

#[test]
fn lectern_insert_read_turn_take_preserves_exact_book() {
    let (mut state, actor) = fixture();
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let position = crate::boundary::BlockPos { x: 8, y: 65, z: 9 };
    let lectern = state.registry.blocks.block(&Identifier::parse("minecraft:lectern").unwrap()).unwrap().default.0;
    state.world.set_block(position.x, position.y, position.z, lectern);
    state.world.fold_overlay();
    let player_slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(player_slot, [8.5, 65.0, 8.5]);
    edit(&mut state, ConnectionId(1), 0, &["first".into(), "second".into()], None);
    let book = state.players[&actor].inventory.slots[0].clone();
    let use_on = PlayerInput::UseItemOn {
        connection: ConnectionId(1), hand: Hand::Main, position,
        face: 1, cursor: [0.5; 3], sequence: 1,
    };
    run_tick(&mut state, &[use_on.clone()]);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    run_tick(&mut state, &[use_on]);
    let menu = state.players[&actor].open.as_ref().expect("lectern opens");
    assert_eq!(menu.kind.menu_type(), Some(17));
    let window = menu.window;
    run_tick(&mut state, &[PlayerInput::ContainerButton { connection: ConnectionId(1), window, button: 2 }]);
    run_tick(&mut state, &[PlayerInput::ContainerButton { connection: ConnectionId(1), window, button: 3 }]);
    assert_eq!(state.players[&actor].inventory.slots.iter().filter(|stack| **stack == book).count(), 1);
}
