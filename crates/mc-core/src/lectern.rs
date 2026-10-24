//! Native lectern book custody and page selection (26.1.2 LecternMenu).
use crate::bees::{block, centre, distance, property};
use crate::boundary::{BlockPos, ChangeSet, EntityId, Hand, PlayerChange};
use crate::inventory::MenuKind;
use crate::tick::{BlockEntityState, TickState};
use mc_data::ItemStack;
use mc_nbt::Tag;
use std::collections::BTreeMap;

type Cell = (i32, i32, i32);
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Lectern {
    page: i32,
    pages: i32,
}
pub(crate) type Lecterns = BTreeMap<Cell, Lectern>;
fn key(pos: BlockPos) -> Cell {
    (pos.x, pos.y, pos.z)
}
fn page_count(book: &ItemStack) -> i32 {
    let Some(component) = book
        .extended()
        .preserved_components
        .iter()
        .find(|component| {
            matches!(
                component.name.as_str(),
                "minecraft:writable_book_content" | "minecraft:written_book_content"
            )
        })
    else {
        return 0;
    };
    let Ok(Tag::Compound(fields)) = mc_nbt::read_network_value(&mut component.value.as_slice())
    else {
        return 0;
    };
    fields
        .iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("pages", Tag::List(pages)) => Some(pages.elements.len() as i32),
            _ => None,
        })
        .unwrap_or(0)
}
fn accepts(state: &TickState, book: &ItemStack) -> bool {
    !book.is_empty()
        && state
            .registry
            .items
            .name_of(book.item_id)
            .is_some_and(|name| matches!(name.path(), "writable_book" | "written_book"))
}
pub(crate) fn hydrate_chunk(
    lecterns: &mut Lecterns,
    inventories: &mut crate::redstone::custody::BlockEntities,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) {
    for (pos, bytes) in &chunk.block_entities {
        if lecterns.contains_key(&key(*pos))
            || chunk
                .get_block(
                    pos.x.rem_euclid(16) as u8,
                    pos.y,
                    pos.z.rem_euclid(16) as u8,
                )
                .and_then(|id| registry.blocks.by_id(id))
                .is_none_or(|block| block.block.id.path() != "lectern")
        {
            continue;
        }
        let Ok(Tag::Compound(fields)) = mc_nbt::read_network(&mut bytes.as_slice()) else {
            continue;
        };
        let book = fields
            .iter()
            .find_map(|(name, tag)| match (name.as_str(), tag) {
                ("Book", Tag::Compound(fields)) => {
                    mc_data::item_nbt::stack_from_fields(&registry.items, fields)
                        .ok()
                        .flatten()
                }
                _ => None,
            })
            .unwrap_or(ItemStack::EMPTY);
        let pages = page_count(&book);
        let page = fields
            .iter()
            .find_map(|(name, tag)| match (name.as_str(), tag) {
                ("Page", Tag::Int(page)) => Some(*page),
                _ => None,
            })
            .unwrap_or(0)
            .clamp(0, (pages - 1).max(0));
        inventories.hydrate(key(*pos), || BlockEntityState {
            slots: vec![book],
            furnace: None,
        });
        lecterns.insert(key(*pos), Lectern { page, pages });
    }
}
fn persist(state: &mut TickState, pos: BlockPos) {
    let mut fields = vec![
        ("id".into(), Tag::String("minecraft:lectern".into())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ];
    if let Some(book) = state
        .block_entities
        .get(&key(pos))
        .and_then(|container| container.slots.first())
        .filter(|book| !book.is_empty())
    {
        fields.push((
            "Book".into(),
            Tag::Compound(
                mc_data::item_nbt::stack_to_fields(&state.registry.items, book)
                    .expect("registered book"),
            ),
        ));
        fields.push(("Page".into(), Tag::Int(state.lecterns[&key(pos)].page)));
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("lectern NBT");
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
    state.block_entities.mark_dirty(key(pos));
}
fn set_property(state: &mut TickState, pos: BlockPos, name: &str, value: &str) {
    if let Some(next) = property(state, pos, name, value)
        && state.world.block_at(pos.x, pos.y, pos.z) != Some(next)
    {
        state.world.set_block(pos.x, pos.y, pos.z, next);
        state.merge_block_writes.push((pos, next));
    }
}
pub(crate) fn use_on(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    pos: BlockPos,
    changes: &mut ChangeSet,
) -> bool {
    if block(state, pos).is_none_or(|block| block.block.id.path() != "lectern") {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    let connection = player.connection;
    let spectator = player.spectator();
    let held = player.inventory.slots[crate::tick::held_slot_index(player, hand)].clone();
    let occupied = state
        .block_entities
        .get(&key(pos))
        .and_then(|container| container.slots.first())
        .is_some_and(|book| !book.is_empty());
    if occupied {
        let rules = state.registry.item_rules();
        let book = &state.block_entities[&key(pos)].slots;
        let effects = state.players.get_mut(&actor).unwrap().open_menu(
            &rules,
            MenuKind::Lectern,
            "Lectern".into(),
            book,
        );
        state.open_containers.insert(actor, key(pos));
        crate::tick::apply_player_effects(state, connection, actor, effects, changes);
        if let Some(menu) = &state.players[&actor].open {
            state.push_player(
                connection,
                PlayerChange::MenuProperty {
                    window: menu.window,
                    property: 0,
                    value: state
                        .lecterns
                        .get(&key(pos))
                        .map_or(0, |lectern| lectern.page as i16),
                },
            );
        }
    } else if !spectator
        && accepts(state, &held)
        && !crate::tick::held_debit_below_floor(state, actor, hand, 1)
    {
        let mut book = held;
        book.count = 1;
        state.lecterns.insert(
            key(pos),
            Lectern {
                page: 0,
                pages: page_count(&book),
            },
        );
        state.block_entities.insert(
            key(pos),
            BlockEntityState {
                slots: vec![book],
                furnace: None,
            },
        );
        let effects = state.players.get_mut(&actor).unwrap().consume_held(hand, 1);
        crate::tick::apply_player_effects(state, connection, actor, effects, changes);
        set_property(state, pos, "has_book", "true");
        persist(state, pos);
    }
    true
}
pub(crate) fn button(
    state: &mut TickState,
    actor: EntityId,
    window: u8,
    button: i32,
    changes: &mut ChangeSet,
) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let Some(menu) = player
        .open
        .as_ref()
        .filter(|menu| menu.kind == MenuKind::Lectern)
    else {
        return false;
    };
    if menu.window != window {
        return true;
    }
    let Some(&(x, y, z)) = state.open_containers.get(&actor) else {
        return true;
    };
    let pos = BlockPos { x, y, z };
    if state
        .entities
        .slot_of(actor)
        .is_none_or(|slot| distance(state.entities.pos(slot), centre(pos)) > 64.0)
        || block(state, pos).is_none_or(|block| block.block.id.path() != "lectern")
    {
        return true;
    }
    let connection = player.connection;
    if button == 3 {
        if player.spectator()
            || player.game_mode == crate::commands::GameMode::Adventure
            || player.vitals.is_dead()
        {
            return true;
        }
        let Some(container) = state.block_entities.get_mut(&key(pos)) else {
            return true;
        };
        let mut book = std::mem::take(&mut container.slots[0]);
        let player = state.players.get_mut(&actor).unwrap();
        player
            .inventory
            .add(&state.registry.item_rules(), &mut book);
        let contents = player.inventory_contents();
        state.push_player(connection, contents);
        if !book.is_empty() {
            changes
                .entities_spawned
                .push(crate::tick::spawn_item_entity(state, centre(pos), book));
        }
        state.lecterns.insert(key(pos), Lectern::default());
        set_property(state, pos, "has_book", "false");
        set_property(state, pos, "powered", "false");
    } else {
        let Some(lectern) = state.lecterns.get_mut(&key(pos)) else {
            return true;
        };
        let page = match button {
            1 => lectern.page - 1,
            2 => lectern.page + 1,
            100.. => button - 100,
            _ => return true,
        }
        .clamp(0, (lectern.pages - 1).max(0));
        if page == lectern.page {
            return true;
        }
        lectern.page = page;
        set_property(state, pos, "powered", "true");
        crate::redstone::schedule_cell(state, pos, 2, 4);
    }
    persist(state, pos);
    let viewers: Vec<_> = state
        .open_containers
        .iter()
        .filter_map(|(id, cell)| (*cell == key(pos)).then_some(*id))
        .collect();
    for viewer in viewers {
        let Some(player) = state.players.get_mut(&viewer) else {
            continue;
        };
        let Some(menu) = player
            .open
            .as_mut()
            .filter(|menu| menu.kind == MenuKind::Lectern)
        else {
            continue;
        };
        menu.state_id = (menu.state_id + 1) & 0x7fff;
        let window = menu.window;
        let connection = player.connection;
        let sync = player.sync_container_view(&state.block_entities[&key(pos)].slots);
        for change in sync {
            state.push_player(connection, change);
        }
        state.push_player(
            connection,
            PlayerChange::MenuProperty {
                window,
                property: 0,
                value: state.lecterns[&key(pos)].page as i16,
            },
        );
    }
    true
}
pub(crate) fn comparator(state: &TickState, pos: BlockPos) -> Option<u8> {
    if block(state, pos).is_none_or(|block| block.block.id.path() != "lectern") {
        return None;
    }
    if state
        .block_entities
        .get(&key(pos))
        .and_then(|container| container.slots.first())
        .is_none_or(ItemStack::is_empty)
    {
        return Some(0);
    }
    let lectern = state.lecterns.get(&key(pos)).copied().unwrap_or_default();
    Some(if lectern.pages <= 1 {
        15
    } else {
        (14 * lectern.page / (lectern.pages - 1) + 1) as u8
    })
}
