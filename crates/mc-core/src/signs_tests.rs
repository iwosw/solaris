use super::{Y, give, held, plant, setup};
use crate::boundary::{BlockPos, ConnectionId, Hand, PlayerChange, PlayerInput};
use crate::tick::{TickState, run_tick};
use mc_nbt::Tag;

const POS: BlockPos = BlockPos {
    x: 1,
    y: Y + 1,
    z: 0,
};

fn image(state: &TickState) -> Tag {
    let bytes = &state
        .world
        .chunk(crate::boundary::ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities[&POS];
    mc_nbt::read_network(&mut bytes.as_slice()).unwrap()
}

fn field<'a>(nbt: &'a Tag, key: &str) -> &'a Tag {
    let Tag::Compound(fields) = nbt else {
        panic!("not a compound")
    };
    &fields.iter().find(|(name, _)| name == key).unwrap().1
}

fn use_item(
    state: &mut TickState,
    player: crate::boundary::EntityId,
    item: &str,
) -> Vec<PlayerChange> {
    give(state, player, item, 3);
    let (_, drains, _) = run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: POS,
            face: 2,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    drains
        .into_iter()
        .flat_map(|(_, changes)| changes)
        .collect()
}

/// Vanilla applicators skip blank faces (SignApplicator.canApplyToSign), so
/// decoration fixtures start with one line written on both faces.
fn inscribe(state: &mut TickState) {
    let Tag::Compound(mut fields) = crate::signs::text_nbt(&["line".into()]) else {
        unreachable!()
    };
    fields[1].1 = fields[0].1.clone();
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).unwrap();
    state.world.set_opaque_block_entity(POS, Some(bytes));
}

fn edit(state: &mut TickState, front: bool, value: &str) {
    run_tick(
        state,
        &[PlayerInput::SignUpdate {
            connection: ConnectionId(1),
            position: POS,
            front,
            lines: vec![value.into(), String::new(), String::new(), String::new()],
        }],
    );
}

#[test]
fn public_sign_payload_updates_chunk_wire_revision_without_a_block_edit() {
    let (mut state, player) = setup();
    plant(
        &mut state,
        POS.x,
        POS.y,
        POS.z,
        "oak_sign",
        &[("rotation", "8"), ("waterlogged", "false")],
    );
    inscribe(&mut state);
    run_tick(&mut state, &[]);
    let block = state.world.block_at(POS.x, POS.y, POS.z);
    let revision = state.world.chunk_revision(0, 0);
    let changes = use_item(&mut state, player, "red_dye");
    assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), block);
    assert!(state.world.chunk_revision(0, 0) > revision);
    assert!(changes.iter().any(|change| matches!(change,
        PlayerChange::BlockEntityData { position, .. } if *position == POS)));
}

#[test]
fn dye_glow_ink_and_wax_update_only_facing_side_and_refuse_waxed_edits() {
    let (mut state, player) = setup();
    // Rotation 8 faces north, toward the joined player at z=0.
    plant(
        &mut state,
        POS.x,
        POS.y,
        POS.z,
        "oak_sign",
        &[("rotation", "8"), ("waterlogged", "false")],
    );
    inscribe(&mut state);
    let changes = use_item(&mut state, player, "red_dye");
    assert_eq!(held(&state, player).count, 2);
    assert!(changes.iter().any(|change| matches!(change,
        PlayerChange::BlockEntityData { position, nbt, .. }
        if *position == POS && field(field(nbt, "front_text"), "color") == &Tag::String("red".into())
    )));
    assert_eq!(
        field(field(&image(&state), "front_text"), "color"),
        &Tag::String("red".into())
    );
    assert_eq!(
        field(field(&image(&state), "back_text"), "color"),
        &Tag::String("black".into())
    );
    use_item(&mut state, player, "glow_ink_sac");
    assert_eq!(held(&state, player).count, 2);
    assert_eq!(
        field(field(&image(&state), "front_text"), "has_glowing_text"),
        &Tag::Byte(1)
    );
    use_item(&mut state, player, "glow_ink_sac");
    assert_eq!(
        held(&state, player).count,
        3,
        "unchanged glow consumes nothing"
    );
    assert!(
        state.sign_editors[&ConnectionId(1)].front,
        "no-op main-hand applicator falls through to editing"
    );
    use_item(&mut state, player, "ink_sac");
    assert_eq!(held(&state, player).count, 2);
    assert_eq!(
        field(field(&image(&state), "front_text"), "has_glowing_text"),
        &Tag::Byte(0)
    );
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::EMPTY;
    super::use_on(&mut state, POS.x, POS.y, POS.z, 3);
    assert!(state.sign_editors[&ConnectionId(1)].front);
    edit(&mut state, false, "wrong side");
    assert!(state.sign_editors.contains_key(&ConnectionId(1)));
    edit(&mut state, true, "front");
    assert_eq!(
        field(field(&image(&state), "front_text"), "color"),
        &Tag::String("red".into())
    );
    // Move around the sign without changing the packet face.
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [1.5, f64::from(Y + 1), 1.5]);
    use_item(&mut state, player, "blue_dye");
    assert_eq!(
        field(field(&image(&state), "back_text"), "color"),
        &Tag::String("blue".into())
    );
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::EMPTY;
    super::use_on(&mut state, POS.x, POS.y, POS.z, 2);
    assert!(!state.sign_editors[&ConnectionId(1)].front);
    edit(&mut state, false, "back");
    let before = image(&state);
    assert_ne!(
        field(field(&before, "front_text"), "messages"),
        field(field(&before, "back_text"), "messages")
    );
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::EMPTY;
    super::use_on(&mut state, POS.x, POS.y, POS.z, 2);
    use_item(&mut state, player, "honeycomb");
    assert_eq!(held(&state, player).count, 2);
    assert!(state.sign_editors.is_empty());
    assert_eq!(field(&image(&state), "is_waxed"), &Tag::Byte(1));
    let waxed = image(&state);
    for item in ["green_dye", "glow_ink_sac", "ink_sac", "honeycomb"] {
        use_item(&mut state, player, item);
        assert_eq!(held(&state, player).count, 3);
        assert_eq!(image(&state), waxed);
    }
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::EMPTY;
    super::use_on(&mut state, POS.x, POS.y, POS.z, 2);
    assert!(state.sign_editors.is_empty());
    edit(&mut state, false, "wax bypass");
    assert_eq!(image(&state), waxed);
    state.sign_editors.insert(
        ConnectionId(1),
        crate::signs::Editor {
            position: POS,
            block: state.world.block_at(POS.x, POS.y, POS.z).unwrap(),
            opened_tick: state.tick,
            front: false,
        },
    );
    edit(&mut state, false, "late authorized submission");
    assert_eq!(image(&state), waxed, "submission rechecks wax");
}

#[test]
fn hanging_and_wall_sign_decorations_support_offhand_and_creative() {
    for name in ["oak_wall_sign", "oak_hanging_sign", "oak_wall_hanging_sign"] {
        let (mut state, player) = setup();
        let props = if name == "oak_hanging_sign" {
            vec![
                ("rotation", "8"),
                ("attached", "true"),
                ("waterlogged", "false"),
            ]
        } else {
            vec![("facing", "north"), ("waterlogged", "false")]
        };
        plant(&mut state, POS.x, POS.y, POS.z, name, &props);
        inscribe(&mut state);
        give(&mut state, player, "orange_dye", 2);
        let dye = held(&state, player);
        state.players.get_mut(&player).unwrap().inventory.slots[crate::inventory::OFF_HAND] = dye;
        assert!(crate::signs::use_on(&mut state, player, Hand::Off, POS));
        assert_eq!(
            state.players[&player].inventory.slots[crate::inventory::OFF_HAND].count,
            1
        );
        assert_eq!(
            field(field(&image(&state), "front_text"), "color"),
            &Tag::String("orange".into())
        );
        if name == "oak_wall_sign" {
            let slot = state.entities.slot_of(player).unwrap();
            state.entities.set_pos(slot, [1.5, f64::from(Y + 1), 0.75]);
            give(&mut state, player, "cyan_dye", 2);
            assert!(crate::signs::use_on(&mut state, player, Hand::Main, POS));
            assert_eq!(
                field(field(&image(&state), "front_text"), "color"),
                &Tag::String("cyan".into()),
                "wall sign side uses the sign's hitbox center",
            );
        }
        state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
        give(&mut state, player, "glow_ink_sac", 2);
        assert!(crate::signs::use_on(&mut state, player, Hand::Main, POS));
        assert_eq!(held(&state, player).count, 2);
        assert_eq!(
            field(field(&image(&state), "front_text"), "has_glowing_text"),
            &Tag::Byte(1)
        );
    }
}

#[test]
fn decoration_journal_and_chunk_nbt_keep_both_sides_and_wax() {
    let (mut state, player) = setup();
    plant(
        &mut state,
        POS.x,
        POS.y,
        POS.z,
        "oak_sign",
        &[("rotation", "8"), ("waterlogged", "false")],
    );
    use_item(&mut state, player, "red_dye");
    use_item(&mut state, player, "glow_ink_sac");
    give(&mut state, player, "honeycomb", 2);
    let (output, _, _) = run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: POS,
            face: 2,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    let expected = image(&state);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let replay = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &[(output.tick, output.tick, entries)],
    );
    let bytes = replay.opaque_block_entities[&(POS.x, POS.y, POS.z)]
        .as_ref()
        .unwrap();
    assert_eq!(
        mc_nbt::read_network(&mut bytes.as_slice()).unwrap(),
        expected
    );
    let chunk = state
        .world
        .chunk(crate::boundary::ChunkPos { x: 0, z: 0 })
        .unwrap();
    let nbt = mc_world::anvil::chunk_to_nbt(chunk, &state.registry.blocks).unwrap();
    let restored =
        mc_world::anvil::chunk_from_nbt(&nbt, &state.registry.blocks, mc_world::OVERWORLD_GEOMETRY)
            .unwrap();
    let bytes = &restored.block_entities[&POS];
    assert_eq!(
        mc_nbt::read_network(&mut bytes.as_slice()).unwrap(),
        expected
    );
}

#[test]
fn decorated_sign_and_pending_leaf_ticks_survive_native_checkpoint_restart_and_anvil() {
    let directory = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(directory.path().to_owned(), 42);
    let native_world = |state: &TickState| {
        crate::world::ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&config).unwrap(),
            0,
            std::sync::Arc::new(super::EmptyWorld {
                air: state.registry.states.air,
            }),
            &state.registry,
            mc_world::BlockStateId(state.registry.states.air),
            [0, Y + 1, 0],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    let (mut state, player) = setup();
    state.world = native_world(&state);
    state.world.ensure_blocking(&state.desired_chunks());
    plant(
        &mut state,
        POS.x,
        POS.y,
        POS.z,
        "oak_sign",
        &[("rotation", "8"), ("waterlogged", "false")],
    );
    for item in ["purple_dye", "glow_ink_sac", "honeycomb"] {
        give(&mut state, player, item, 2);
        assert!(crate::signs::use_on(&mut state, player, Hand::Main, POS));
    }
    let expected = image(&state);
    let leaf = BlockPos { x: 3, ..POS };
    plant(
        &mut state,
        leaf.x,
        leaf.y,
        leaf.z,
        "oak_leaves",
        &[
            ("distance", "7"),
            ("persistent", "true"),
            ("waterlogged", "false"),
        ],
    );
    let leaf_id = state.world.block_at(leaf.x, leaf.y, leaf.z).unwrap();
    crate::leaves::schedule(&mut state, leaf, leaf_id);
    let absent = state.block_ticks.checkpoint(&mut state.world, state.tick);
    let mut capture = state.world.capture_dirty(&Default::default());
    state
        .world
        .capture_scheduled_block_ticks(&mut capture, absent);
    capture.write().unwrap();
    drop(capture);
    drop(state);
    let (mut restarted, player) = setup();
    restarted.world = native_world(&restarted);
    restarted.world.ensure_blocking(&restarted.desired_chunks());
    assert_eq!(image(&restarted), expected);
    give(&mut restarted, player, "red_dye", 2);
    assert!(crate::signs::use_on(
        &mut restarted,
        player,
        Hand::Main,
        POS
    ));
    assert_eq!(
        held(&restarted, player).count,
        2,
        "reloaded wax refuses dye"
    );
    assert_eq!(image(&restarted), expected);
    assert_eq!(
        restarted.world.block_at(leaf.x, leaf.y, leaf.z),
        Some(leaf_id)
    );
    let chunk = restarted
        .world
        .chunk(crate::boundary::ChunkPos { x: 0, z: 0 })
        .unwrap();
    assert!(
        chunk
            .scheduled_block_ticks()
            .iter()
            .any(|tick| tick.pos == leaf)
    );
    let nbt = mc_world::anvil::chunk_to_nbt(chunk, &restarted.registry.blocks).unwrap();
    let exported = mc_world::anvil::chunk_from_nbt(
        &nbt,
        &restarted.registry.blocks,
        mc_world::OVERWORLD_GEOMETRY,
    )
    .unwrap();
    assert_eq!(
        mc_nbt::read_network(&mut exported.block_entities[&POS].as_slice()).unwrap(),
        expected,
    );
    assert!(
        exported
            .scheduled_block_ticks()
            .iter()
            .any(|tick| tick.pos == leaf)
    );
}

#[test]
fn secondary_use_places_held_block_instead_of_opening_sign_editor() {
    let (mut state, player) = setup();
    plant(
        &mut state,
        POS.x,
        POS.y,
        POS.z,
        "oak_sign",
        &[("rotation", "8"), ("waterlogged", "false")],
    );
    give(&mut state, player, "stone", 2);
    run_tick(
        &mut state,
        &[
            PlayerInput::Sneak {
                connection: ConnectionId(1),
                sneaking: true,
            },
            PlayerInput::UseItemOn {
                connection: ConnectionId(1),
                hand: Hand::Main,
                position: POS,
                face: 3,
                cursor: [0.5; 3],
                sequence: 1,
            },
        ],
    );
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z + 1),
        Some(state.registry.states.stone),
    );
    assert!(state.sign_editors.is_empty());
    assert_eq!(held(&state, player).count, 1);
    run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Off,
            position: POS,
            face: 3,
            cursor: [0.5; 3],
            sequence: 2,
        }],
    );
    assert!(
        state.sign_editors.is_empty(),
        "empty offhand cannot bypass secondary-use"
    );
}

#[test]
fn another_players_editor_refuses_decoration_but_allows_ordinary_item_placement() {
    let (mut state, player) = setup();
    plant(
        &mut state,
        POS.x,
        POS.y,
        POS.z,
        "oak_sign",
        &[("rotation", "8"), ("waterlogged", "false")],
    );
    inscribe(&mut state);
    use_item(&mut state, player, "red_dye");
    let expected = image(&state);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: uuid::Uuid::new_v4(),
            name: "editor".into(),
            permission_level: 0,
        }],
    );
    let editor = state.connections.entity_of(ConnectionId(2)).unwrap();
    assert!(crate::signs::use_on(&mut state, editor, Hand::Main, POS));
    assert!(state.sign_editors.contains_key(&ConnectionId(2)));
    use_item(&mut state, player, "blue_dye");
    assert_eq!(held(&state, player).count, 3);
    assert_eq!(image(&state), expected);
    assert!(!state.sign_editors.contains_key(&ConnectionId(1)));
    give(&mut state, player, "stone", 2);
    super::use_on(&mut state, POS.x, POS.y, POS.z, 3);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z + 1),
        Some(state.registry.states.stone),
    );
    assert_eq!(held(&state, player).count, 1);
    assert!(state.sign_editors.contains_key(&ConnectionId(2)));
}

#[test]
fn sign_decorations_admit_only_actual_free_source_debits() {
    let (mut state, actor) = setup();
    plant(
        &mut state,
        POS.x,
        POS.y,
        POS.z,
        "oak_sign",
        &[("rotation", "8"), ("waterlogged", "false")],
    );
    inscribe(&mut state);
    use_item(&mut state, actor, "red_dye");
    for resource in ["blue_dye", "glow_ink_sac", "ink_sac", "honeycomb"] {
        give(&mut state, actor, resource, 3);
        crate::crafter::tests::hold(
            &mut state,
            mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
            resource,
            3,
        );
        let before = image(&state);
        let opaque_count = state.opaque_changes.len();
        assert!(crate::signs::use_on(&mut state, actor, Hand::Main, POS));
        assert_eq!(image(&state), before);
        assert_eq!(held(&state, actor).count, 3);
        assert_eq!(state.opaque_changes.len(), opaque_count);
        give(&mut state, actor, resource, 4);
        assert!(crate::signs::use_on(&mut state, actor, Hand::Main, POS));
        assert_eq!(held(&state, actor).count, 3);
        assert_ne!(image(&state), before);
        crate::plugin_inventories::invalidate_player(&mut state, actor);
    }
}
