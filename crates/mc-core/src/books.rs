//! Book edits are authoritative player inventory replacements in the native tick.
use crate::boundary::{ConnectionId, EntityId, Hand, PlayerChange};
use crate::tick::TickState;
use mc_data::Identifier;
use mc_nbt::{ListTag, Tag, tag_type};

pub(crate) fn use_item(state: &mut TickState, actor: EntityId, hand: Hand) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let held = &player.inventory.slots[crate::tick::held_slot_index(player, hand)];
    let Some(name) = state.registry.items.name_of(held.item_id) else {
        return false;
    };
    if held.is_empty()
        || !matches!(
            name.as_str(),
            "minecraft:writable_book" | "minecraft:written_book"
        )
    {
        return false;
    }
    if name.as_str() == "minecraft:written_book" && !player.vitals.is_dead() {
        state.push_player(player.connection, PlayerChange::BookOpened { hand });
    }
    true
}

pub(crate) fn edit(
    state: &mut TickState,
    connection: ConnectionId,
    slot: i32,
    pages: &[String],
    title: Option<&str>,
) {
    if !((0..=8).contains(&slot) || slot == 40)
        || pages.len() > 100
        || pages.iter().any(|page| page.encode_utf16().count() > 1024)
        || title.is_some_and(|title| title.encode_utf16().count() > 32)
    {
        return;
    }
    let Some(actor) = state.connections.entity_of(connection) else {
        return;
    };
    let Some(player) = state.players.get(&actor) else {
        return;
    };
    if player.spectator() || player.vitals.is_dead() {
        return;
    }
    let slot = slot as usize;
    let held = &player.inventory.slots[slot];
    if held.count != 1
        || state
            .registry
            .items
            .name_of(held.item_id)
            .is_none_or(|id| id.as_str() != "minecraft:writable_book")
    {
        return;
    }
    let mut replacement = held.clone();
    let mut fields = vec![(
        "pages".into(),
        Tag::List(ListTag {
            element_type: tag_type::STRING,
            elements: pages.iter().cloned().map(Tag::String).collect(),
        }),
    )];
    let component = if let Some(title) = title {
        replacement.item_id = state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:written_book").unwrap())
            .expect("written book item");
        fields.extend([
            ("title".into(), Tag::String(title.into())),
            (
                "author".into(),
                Tag::String(
                    state
                        .connections
                        .name_of(actor)
                        .expect("connected author")
                        .into(),
                ),
            ),
            ("generation".into(), Tag::Int(0)),
            // Authored pages are literal text, not commands/selectors to resolve.
            ("resolved".into(), Tag::Byte(1)),
        ]);
        "minecraft:written_book_content"
    } else {
        "minecraft:writable_book_content"
    };
    let mut value = Vec::new();
    if mc_nbt::write_network_value(&mut value, &Tag::Compound(fields)).is_err() {
        return;
    }
    replacement
        .extended_mut()
        .preserved_components
        .retain(|old| {
            !matches!(
                old.name.trim_start_matches('!'),
                "minecraft:writable_book_content" | "minecraft:written_book_content"
            )
        });
    replacement.extended_mut().preserved_components.push(
        mc_data::item_stack::PreservedItemComponent {
            name: component.into(),
            value,
            baseline: None,
        },
    );
    if crate::plugin_inventories::below_player_replacement_floors(
        state,
        actor,
        &[(slot, &replacement)],
    ) {
        let effects = crate::tick::inventory_transition_refused(state, actor);
        for change in effects.changes {
            state.push_player(connection, change);
        }
        return;
    }
    let player = state.players.get_mut(&actor).expect("book owner");
    player.inventory.slots[slot] = replacement;
    let change = player.inventory_contents();
    state.push_player(connection, change);
}

#[cfg(test)]
#[path = "books_tests.rs"]
mod tests;
