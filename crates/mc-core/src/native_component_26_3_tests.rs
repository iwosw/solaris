//! Give + creative input custody -> redb checkpoint -> reopened inventory.
use super::*;
use crate::dimensions::Dimension;

#[test]
fn new_26_3_components_survive_give_creative_checkpoint_and_native_restart() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let mut state = recover_dimension_state(&config, Dimension::Overworld).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        state.other_dimensions.insert(
            dimension,
            Box::new(recover_dimension_state(&config, dimension).unwrap()),
        );
    }
    let uuid = uuid::Uuid::from_u128(263);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(263),
            uuid,
            name: "Components263".into(),
            permission_level: 4,
        }],
    );
    let actor = *state.players.keys().next().unwrap();
    for (name, value) in [
        ("attack_animation", "{type:'stab',duration:9}"),
        ("interact_animation", "{}"),
        ("block_transformer", "'minecraft:axe'"),
        ("villager_food", "{nutrition:12}"),
        ("sulfur_cube_content", "'minecraft:stone'"),
        ("compostable", "{layers:1}"),
        ("cooking_fuel", "{burn_time:200,speed_multiplier:1.0f}"),
        ("brewing_fuel", "{uses:20,speed_multiplier:1.0f}"),
        (
            "mob_visibility",
            "{targeting_entity_types:'#minecraft:skeletons',visibility:0.5f}",
        ),
        ("provides_pottery_pattern", "'minecraft:angler'"),
        ("sign_text_front", "{messages:['A','B','C','D']}"),
        (
            "sign_text_back",
            "{messages:['a','b','c','d'],color:'red',has_glowing_text:true}",
        ),
        ("waxed", "{}"),
        ("cushion/color", "'red'"),
    ] {
        let item =
            crate::command_items::ItemArgument::parse(&format!("stone[minecraft:{name}={value}]"))
                .unwrap();
        let stack = item.stack(&state.registry.items).unwrap();
        crate::commands::give(
            &state.registry.item_rules(),
            &mut state.players.get_mut(&actor).unwrap().inventory,
            &stack,
            1,
        );
    }
    let given = state.players[&actor].inventory.slots.clone();
    assert_eq!(given.iter().filter(|stack| !stack.is_empty()).count(), 14);
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    for (index, mut stack) in given.into_iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        stack.count = 2;
        // Native hotbar indices 0..8 use vanilla menu slots 36..44.
        let slot = if index < 9 { index + 36 } else { index };
        crate::tick::run_tick(
            &mut state,
            &[PlayerInput::CreativeSlot {
                connection: ConnectionId(263),
                slot: slot as i16,
                stack: stack.clone(),
            }],
        );
        assert_eq!(
            state.players[&actor].inventory.slots[index], stack,
            "creative input must actually replace component-bearing slot {index}"
        );
    }
    let expected = state.players[&actor].inventory.slots.clone();
    assert_eq!(
        expected.iter().filter(|stack| !stack.is_empty()).count(),
        14
    );
    let tick = state.tick;
    let (work_tx, work_rx) = journal_work_channel();
    let (done_tx, _) = std::sync::mpsc::channel();
    assert!(start_checkpoint(&mut state, &work_tx, &done_tx, tick));
    let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
        panic!("checkpoint capture")
    };
    let mut manifest = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
    let mut writer =
        JournalWriter::open_recovered(&config.world_dir, 1, manifest.tick, manifest.tick).unwrap();
    writer.append(1, tick, &[]).unwrap();
    write_checkpoint(
        &config.world_dir,
        &job,
        &state.plugins,
        &mut writer,
        &mut manifest,
    )
    .unwrap();
    drop(writer);
    drop(job);
    drop(state);
    let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
    let saved = recovered
        .checkpoint
        .players
        .iter()
        .find(|player| player.uuid == *uuid.as_bytes())
        .unwrap();
    let tag = crate::native_checkpoint::decode_tag(&saved.record, "player").unwrap();
    let player = crate::journal::player_from_tag(&tag, &recovered.registry.items).unwrap();
    assert_eq!(player.inventory, expected);
}
