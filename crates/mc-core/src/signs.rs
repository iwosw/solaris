//! Owned, side-aware sign editing and decoration in vanilla opaque NBT.

use mc_nbt::{ListTag, Tag};
use mc_physics::mth;
use mc_world::BlockState;

use crate::boundary::{BlockPos, ConnectionId, EntityId, Hand, PlayerChange};
use crate::tick::TickState;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Editor {
    pub position: BlockPos,
    pub block: u32,
    pub opened_tick: u64,
    pub front: bool,
}

/// The placement that owns this editor is allowed its initial block write.
/// Any later write revokes it, including break/re-place of the same state.
pub(crate) fn invalidate(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    let tick = state.tick;
    state.sign_editors.retain(|_, editor| {
        editor.opened_tick == tick || !writes.iter().any(|(pos, _)| *pos == editor.position)
    });
    // Flow/support/falling edits do not enter container-break staging.
    // Tombstone removed opaque signs in their block tick, before replay can
    // reapply an older text post-image over the replacement block.
    for (pos, new) in writes {
        let removed = state
            .world
            .chunk(crate::boundary::ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .is_some_and(|chunk| {
                chunk.block_entities.contains_key(pos)
                    && chunk
                        .get_block(
                            pos.x.rem_euclid(16) as u8,
                            pos.y,
                            pos.z.rem_euclid(16) as u8,
                        )
                        .and_then(|old| state.registry.blocks.by_id(old))
                        .zip(state.registry.blocks.by_id(mc_world::BlockStateId(*new)))
                        .is_some_and(|(old, new)| old.block.id != new.block.id)
            });
        if removed {
            state.world.set_opaque_block_entity(*pos, None);
            if !state
                .opaque_changes
                .iter()
                .any(|(cell, nbt)| *cell == *pos && nbt.is_none())
            {
                state.opaque_changes.push((*pos, None));
            }
        }
    }
}

pub(crate) fn editable(state: &BlockState) -> bool {
    let path = state.block.id.path();
    path.ends_with("_sign")
}

pub(crate) fn update(
    state: &mut TickState,
    connection: ConnectionId,
    position: BlockPos,
    front: bool,
    lines: &[String],
) {
    let Some(editor) = state.sign_editors.get(&connection).copied() else {
        return;
    };
    if editor.position != position || front != editor.front || lines.len() != 4 {
        return;
    }
    let Some(entity) = state.connections.entity_of(connection) else {
        return;
    };
    if !crate::tick::within_block_reach(state, entity, position)
        || state
            .players
            .get(&entity)
            .is_none_or(|player| player.spectator())
        || state.world.block_at(position.x, position.y, position.z) != Some(editor.block)
    {
        state.sign_editors.remove(&connection);
        return;
    }
    state.sign_editors.remove(&connection);
    let Some(mut nbt) = image(state, position) else {
        return;
    };
    if waxed(&nbt) {
        return;
    }
    let messages = messages(lines);
    let text = side_mut(&mut nbt, front);
    set(text, "messages", messages);
    text.retain(|(key, _)| key != "filtered_messages");
    publish(state, position, editor.block, nbt);
}

fn publish(state: &mut TickState, position: BlockPos, block: u32, nbt: Tag) {
    use mc_data::block_entity_kinds::BlockEntityKind;
    let kind = if state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(block))
        .is_some_and(|block| block.block.id.path().ends_with("_hanging_sign"))
    {
        BlockEntityKind::HangingSign
    } else {
        BlockEntityKind::Sign
    };
    let Tag::Compound(mut fields) = nbt.clone() else {
        unreachable!()
    };
    for (key, value) in [
        ("x", Tag::Int(position.x)),
        ("y", Tag::Int(position.y)),
        ("z", Tag::Int(position.z)),
        ("id", Tag::String(kind.vanilla_id().into())),
    ] {
        set(&mut fields, key, value);
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("sign NBT encodes");
    state
        .world
        .set_opaque_block_entity(position, Some(bytes.clone()));
    state.opaque_changes.push((position, Some(bytes)));
    state.world.mark_block_entity_visible(position);
    let viewers: Vec<_> = state
        .players
        .values()
        .map(|player| player.connection)
        .collect();
    for viewer in viewers {
        state.push_player(
            viewer,
            PlayerChange::BlockEntityData {
                position,
                block_entity_type: kind.protocol_id(),
                nbt: nbt.clone(),
            },
        );
    }
}

fn read_image(state: &TickState, pos: BlockPos) -> Option<Tag> {
    let bytes = state
        .world
        .chunk(crate::boundary::ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos));
    let Some(bytes) = bytes else {
        return Some(text_nbt(&[]));
    };
    let mut input = bytes.as_slice();
    match mc_nbt::read_network(&mut input) {
        Ok(tag @ Tag::Compound(_)) if input.is_empty() => Some(tag),
        _ => None,
    }
}

fn image(state: &mut TickState, pos: BlockPos) -> Option<Tag> {
    let image = read_image(state, pos);
    if image.is_none() {
        crate::block_drop_context::reject(
            state,
            pos,
            crate::CoreError::Storage("sign: invalid block entity NBT".into()),
        );
    }
    image
}

fn waxed(nbt: &Tag) -> bool {
    matches!(nbt, Tag::Compound(fields) if fields.iter().any(|(key, value)|
        key == "is_waxed" && matches!(value, Tag::Byte(value) if *value != 0)))
}

fn set(fields: &mut Vec<(String, Tag)>, key: &str, value: Tag) {
    if let Some((_, old)) = fields.iter_mut().find(|(name, _)| name == key) {
        *old = value;
    } else {
        fields.push((key.into(), value));
    }
}

fn side_mut(nbt: &mut Tag, front: bool) -> &mut Vec<(String, Tag)> {
    let Tag::Compound(fields) = nbt else {
        unreachable!("sign image is a compound")
    };
    let key = if front { "front_text" } else { "back_text" };
    if !fields.iter().any(|(name, _)| name == key) {
        fields.push((key.into(), text(&[])));
    }
    let (_, value) = fields.iter_mut().find(|(name, _)| name == key).unwrap();
    if !matches!(value, Tag::Compound(_)) {
        *value = text(&[]);
    }
    let Tag::Compound(fields) = value else {
        unreachable!()
    };
    fields
}

/// Vanilla chooses a side from the player's position relative to sign rotation,
/// not the clicked packet face (SignBlockEntity.isFacingFrontText, local javap).
fn front(block: &BlockState, pos: BlockPos, player: [f64; 3]) -> bool {
    let prop = |name: &str| {
        block
            .properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let normal = match prop("facing") {
        Some("north") => [0.0, -1.0],
        Some("south") => [0.0, 1.0],
        Some("east") => [1.0, 0.0],
        Some("west") => [-1.0, 0.0],
        _ => {
            let angle = prop("rotation")
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0)
                * std::f64::consts::TAU
                / 16.0;
            [-f64::from(mth::sin(angle)), f64::from(mth::cos(angle))]
        }
    };
    // WallSignBlock's local javap shape is two units thick against the wall;
    // its hitbox center is 7/16 behind the block center along the facing.
    let wall_offset = if block.block.id.path().ends_with("_wall_sign") {
        7.0 / 16.0
    } else {
        0.0
    };
    (player[0] - f64::from(pos.x) - 0.5) * normal[0]
        + (player[2] - f64::from(pos.z) - 0.5) * normal[1]
        + wall_offset
        >= 0.0
}

/// Dispatch for a held block item, which is not a sign decoration. Keep the
/// native sneaking, waxing, editor ownership and off-hand fallthroughs intact.
pub(crate) fn intercepts_placement(
    state: &TickState,
    actor: EntityId,
    hand: Hand,
    pos: BlockPos,
    sneaking: Option<bool>,
) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.spectator() {
        return false;
    }
    let Some(block) = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
    else {
        return false;
    };
    if !editable(block) {
        return false;
    }
    let Some(slot) = state.entities.slot_of(actor) else {
        return false;
    };
    if sneaking.unwrap_or_else(|| {
        state.entities.row(slot).appearance.shared_flags & crate::tick::SNEAKING_FLAG != 0
    }) && (!player.inventory.slots[crate::tick::held_slot_index(player, Hand::Main)].is_empty()
        || !player.inventory.slots[crate::inventory::OFF_HAND].is_empty())
    {
        return false;
    }
    let Some(nbt) = read_image(state, pos) else {
        return true;
    };
    if waxed(&nbt) {
        return true;
    }
    !state
        .sign_editors
        .iter()
        .any(|(owner, editor)| *owner != player.connection && editor.position == pos)
        && hand == Hand::Main
}

/// Whether the sign face NBT carries a nonempty rendered message.
fn face_has_message(nbt: &Tag, front: bool) -> bool {
    let Tag::Compound(fields) = nbt else {
        return false;
    };
    let side = if front { "front_text" } else { "back_text" };
    let Some(Tag::Compound(side_fields)) =
        fields.iter().find(|(key, _)| key == side).map(|(_, value)| value)
    else {
        return false;
    };
    side_fields
        .iter()
        .find(|(key, _)| key == "messages" || key == "message")
        .is_some_and(|(_, value)| match value {
            Tag::List(list) => list.elements.iter().any(|line| match line {
                Tag::String(text) => {
                    let text = text.trim();
                    !text.trim().is_empty()
                }
                _ => false,
            }),
            Tag::String(text) => {
                let text = text.trim();
                !text.trim().is_empty()
            }
            _ => false,
        })
}

pub(crate) fn use_on(state: &mut TickState, actor: EntityId, hand: Hand, pos: BlockPos) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.spectator() {
        return false;
    }
    // Vanilla mayBuild: Adventure players cannot edit or decorate signs.
    if player.game_mode == crate::commands::GameMode::Adventure {
        return false;
    }
    let Some(id) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
        return false;
    };
    if !editable(block) {
        return false;
    }
    let Some(slot) = state.entities.slot_of(actor) else {
        return false;
    };
    let front = front(block, pos, state.entities.pos(slot));
    let connection = player.connection;
    let held = &player.inventory.slots[crate::tick::held_slot_index(player, hand)];
    if state.entities.row(slot).appearance.shared_flags & crate::tick::SNEAKING_FLAG != 0
        && (!player.inventory.slots[crate::tick::held_slot_index(player, Hand::Main)].is_empty()
            || !player.inventory.slots[crate::inventory::OFF_HAND].is_empty())
    {
        return false;
    }
    let item = state
        .registry
        .items
        .name_of(held.item_id)
        .map(|id| id.path())
        .unwrap_or("");
    let decoration = if held.is_empty() {
        None
    } else if item == "honeycomb" {
        Some(("is_waxed", Tag::Byte(1)))
    } else if item == "glow_ink_sac" {
        Some(("has_glowing_text", Tag::Byte(1)))
    } else if item == "ink_sac" {
        Some(("has_glowing_text", Tag::Byte(0)))
    } else {
        item.strip_suffix("_dye")
            .map(|color| ("color", Tag::String(color.into())))
    };
    let Some(mut nbt) = image(state, pos) else {
        return true;
    };
    if waxed(&nbt) {
        return true;
    }
    if state
        .sign_editors
        .iter()
        .any(|(owner, editor)| *owner != connection && editor.position == pos)
    {
        return false;
    }
    if let Some((key, value)) = decoration {
        // Vanilla SignApplicator.canApplyToSign: dye/glow apply only when
        // the targeted face has a nonempty message; blank faces consume
        // nothing. Waxing has no message requirement.
        if key != "is_waxed" && !face_has_message(&nbt, front) {
            return true;
        }
        let fields = if key == "is_waxed" {
            let Tag::Compound(fields) = &mut nbt else {
                unreachable!()
            };
            fields
        } else {
            side_mut(&mut nbt, front)
        };
        if !fields
            .iter()
            .any(|(name, old)| name == key && *old == value)
        {
            if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
                return true;
            }
            let before = {
                let player = &state.players[&actor];
                player.inventory.slots[crate::tick::held_slot_index(player, hand)].clone()
            };
            set(fields, key, value);
            if key == "is_waxed" {
                state
                    .sign_editors
                    .retain(|_, editor| editor.position != pos);
            }
            publish(state, pos, id, nbt);
            let effects = state.players.get_mut(&actor).unwrap().consume_held(hand, 1);
            for change in effects.changes {
                state.push_player(connection, change);
            }
            crate::advancements::accepted_block(
                state,
                actor,
                "minecraft:item_used_on_block",
                pos,
                Some(id),
                Some(before),
            );
            return true;
        }
    }
    // Vanilla only falls back to empty-hand editing for a main-hand use.
    if hand == Hand::Off {
        return false;
    }
    state.sign_editors.insert(
        connection,
        Editor {
            position: pos,
            block: id,
            opened_tick: state.tick,
            front,
        },
    );
    state.push_player(
        connection,
        PlayerChange::SignEditorOpened {
            position: pos,
            front,
            block: id,
        },
    );
    true
}

fn messages(lines: &[String]) -> Tag {
    Tag::List(ListTag {
        element_type: mc_nbt::tag_type::STRING,
        elements: (0..4)
            .map(|i| Tag::String(lines.get(i).cloned().unwrap_or_default()))
            .collect(),
    })
}

fn text(lines: &[String]) -> Tag {
    Tag::Compound(vec![
        ("messages".into(), messages(lines)),
        ("color".into(), Tag::String("black".into())),
        ("has_glowing_text".into(), Tag::Byte(0)),
    ])
}

pub(crate) fn text_nbt(lines: &[String]) -> Tag {
    Tag::Compound(vec![
        ("front_text".into(), text(lines)),
        ("back_text".into(), text(&[])),
        ("is_waxed".into(), Tag::Byte(0)),
    ])
}
