use super::*;
use mc_data::ItemStack;

fn stack(state: &TickState, name: &str) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap(),
        1,
    )
}
fn fixture(name: &str) -> (TickState, EntityId, [f64; 3]) {
    let (mut state, _, mut pos) = flat_player_fixture();
    pos[0] += 4.0;
    let value = stack(&state, name);
    let item = crate::tick::spawn_item_entity(&mut state, pos, value);
    (state, item, pos)
}

#[test]
fn item_hazards_lava_fire_and_cactus_destroy_ordinary_drops_without_replacements() {
    for hazard in ["lava", "fire", "soul_fire", "cactus"] {
        let (mut state, item, pos) = fixture("minecraft:stone");
        place(&mut state, pos, hazard, &[]);
        let output = run_tick(&mut state, &[]).0;
        assert!(state.entities.slot_of(item).is_none(), "{hazard}");
        assert!(output.changes.entities_removed.contains(&item), "{hazard}");
        assert!(
            output.changes.entities_spawned.is_empty(),
            "must not drop a replacement"
        );
    }
}

#[test]
fn item_hazards_report_authored_netherite_defaults_resist_fire_but_not_cactus() {
    for hazard in ["lava", "fire", "soul_fire"] {
        let (mut state, item, pos) = fixture("minecraft:netherite_ingot");
        place(&mut state, pos, hazard, &[]);
        run_tick(&mut state, &[]);
        assert!(state.entities.slot_of(item).is_some(), "{hazard}");
    }
    let (mut state, item, pos) = fixture("minecraft:netherite_ingot");
    place(&mut state, pos, "cactus", &[]);
    run_tick(&mut state, &[]);
    assert!(state.entities.slot_of(item).is_none());
}

#[test]
fn item_hazards_removed_damage_resistant_burns_netherite_but_plain_default_survives() {
    for removed in [false, true] {
        let (mut state, _, mut pos) = flat_player_fixture();
        pos[0] += 4.0;
        let value = if removed {
            let fields = vec![
                (
                    "id".into(),
                    mc_nbt::Tag::String("minecraft:netherite_ingot".into()),
                ),
                ("count".into(), mc_nbt::Tag::Int(1)),
                (
                    "components".into(),
                    mc_nbt::Tag::Compound(vec![(
                        "!minecraft:damage_resistant".into(),
                        mc_nbt::Tag::Compound(vec![]),
                    )]),
                ),
            ];
            let value = mc_data::item_nbt::stack_from_fields(&state.registry.items, &fields)
                .unwrap()
                .unwrap();
            // Native world/player NBT preserves removal independently of defaults.
            let durable =
                mc_data::item_nbt::stack_to_fields(&state.registry.items, &value).unwrap();
            let value = mc_data::item_nbt::stack_from_fields(&state.registry.items, &durable)
                .unwrap()
                .unwrap();
            assert!(
                value
                    .extended()
                    .preserved_components
                    .iter()
                    .any(|component| component.name == "!minecraft:damage_resistant")
            );
            value
        } else {
            stack(&state, "minecraft:netherite_ingot")
        };
        let item = crate::tick::spawn_item_entity(&mut state, pos, value);
        place(&mut state, pos, "lava", &[]);
        let output = run_tick(&mut state, &[]).0;
        assert_eq!(state.entities.slot_of(item).is_none(), removed);
        assert_eq!(output.changes.entities_removed.contains(&item), removed);
    }
}

#[test]
fn item_hazards_destroyed_drop_tombstone_replays_idempotently_without_resurrection() {
    let (mut state, item, pos) = fixture("minecraft:stone");
    let mut first = run_tick(&mut state, &[]).0;
    std::sync::Arc::get_mut(&mut first.changes)
        .unwrap()
        .entities_spawned
        .push(item);
    let image = crate::core::journal_entries_for_tick(&mut state, &first);
    let initial = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &[(first.tick, first.tick, image)],
    );
    assert!(initial.entities.contains_key(&item.0));
    place(&mut state, pos, "lava", &[]);
    let output = run_tick(&mut state, &[]).0;
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|entry| matches!(entry, crate::journal::JournalEntry::RemoveEntity { id, .. } if *id == item.0)));
    let tail = [(output.tick, output.tick, entries)];
    let replay = crate::journal::replay_batches(initial, &tail);
    assert!(!replay.entities.contains_key(&item.0));
    let replay = crate::journal::replay_batches(replay, &tail);
    assert!(!replay.entities.contains_key(&item.0));
}
