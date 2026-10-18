use super::*;
use crate::bees::tests::{item, mob, state};
use crate::boundary::EntityId;
use crate::entities::ColdRow;
use mc_data::Identifier;

const POS: [f64; 3] = [4.5, 201.0, 4.5];

fn row(state: &TickState, id: EntityId) -> &ColdRow {
    state.entities.row(state.entities.slot_of_any(id).unwrap())
}

fn collector(state: &mut TickState, name: &str, pos: [f64; 3]) -> EntityId {
    let id = mob(state, name, pos);
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row_mut(slot);
    Arc::make_mut(&mut row.appearance).equipment = Default::default();
    row.loot.can_pick_up_loot = true;
    row.loot.drop_chances = [0.085; 6];
    id
}

fn ground(state: &mut TickState, stack: ItemStack, pos: [f64; 3]) -> EntityId {
    let id = crate::tick::spawn_item_entity(state, pos, stack);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.body_mut(slot).pickup_delay = 0;
    id
}

fn enchant(stack: &mut ItemStack, name: &str) {
    stack.enchantments.push(mc_data::ItemEnchantment {
        id: Identifier::parse(name).unwrap(),
        level: 1,
    });
}

#[test]
fn supported_humanoids_collect_armor_and_keep_other_collectors_out() {
    let mut state = state(None);
    let mut equipped = Vec::new();
    for (i, name) in [
        "minecraft:zombie",
        "minecraft:zombie_villager",
        "minecraft:husk",
        "minecraft:drowned",
        "minecraft:zombified_piglin",
        "minecraft:skeleton",
        "minecraft:wither_skeleton",
        "minecraft:stray",
        "minecraft:bogged",
        "minecraft:parched",
        "minecraft:piglin",
    ]
    .into_iter()
    .enumerate()
    {
        let pos = [
            2.0 + (i % 3) as f64 * 4.0,
            201.0,
            2.0 + (i / 3) as f64 * 4.0,
        ];
        let id = collector(&mut state, name, pos);
        let helmet = item(&state, "minecraft:iron_helmet", 1);
        let dropped = ground(&mut state, helmet.clone(), pos);
        equipped.push((id, dropped, helmet));
    }
    let mut changes = ChangeSet::default();
    run(&mut state, &mut changes);
    for (id, dropped, helmet) in equipped {
        assert_eq!(row(&state, id).appearance.equipment[5], Some(helmet));
        assert_eq!(row(&state, id).loot.drop_chances[5], 2.0);
        assert!(row(&state, id).persistent);
        assert!(state.appearance_changed.contains(&id));
        assert!(changes.entities_removed.contains(&dropped));
        assert!(state.entities.slot_of(dropped).is_none());
    }
    for name in [
        "minecraft:cow",
        "minecraft:piglin_brute",
        "minecraft:allay",
        "minecraft:copper_golem",
        "minecraft:dolphin",
    ] {
        let id = collector(&mut state, name, POS);
        let sword = item(&state, "minecraft:diamond_sword", 1);
        let dropped = ground(&mut state, sword, POS);
        // The supported mobs are no longer in this collector's reach.
        for slot in state.entities.slots().collect::<Vec<_>>() {
            if state.entities.body(slot).id != id
                && supported(match state.entities.body(slot).kind {
                    EntityKind::Mob { type_id } => state.registry.blueprint(type_id).unwrap().name,
                    _ => "",
                })
            {
                state.entities.set_pos(slot, [100.0, 201.0, 100.0]);
            }
        }
        run(&mut state, &mut changes);
        assert!(
            row(&state, id)
                .appearance
                .equipment
                .iter()
                .all(Option::is_none)
        );
        assert!(state.entities.slot_of(dropped).is_some(), "{name}");
    }
}

#[test]
fn comparisons_follow_armor_weapon_and_piglin_preferences() {
    let state = state(None);
    let stack = |name| item(&state, name, 1);
    let cases = [
        (
            "minecraft:zombie",
            "minecraft:diamond_chestplate",
            "minecraft:iron_chestplate",
            true,
        ),
        (
            "minecraft:zombie",
            "minecraft:leather_chestplate",
            "minecraft:iron_chestplate",
            false,
        ),
        (
            "minecraft:zombie",
            "minecraft:netherite_helmet",
            "minecraft:diamond_helmet",
            true,
        ),
        (
            "minecraft:zombie",
            "minecraft:diamond_sword",
            "minecraft:iron_sword",
            true,
        ),
        (
            "minecraft:zombie",
            "minecraft:wooden_sword",
            "minecraft:diamond_axe",
            true,
        ),
        (
            "minecraft:zombie",
            "minecraft:diamond_axe",
            "minecraft:wooden_sword",
            false,
        ),
        (
            "minecraft:skeleton",
            "minecraft:bow",
            "minecraft:stone_sword",
            false,
        ),
        (
            "minecraft:zombie",
            "minecraft:iron_pickaxe",
            "minecraft:stone",
            true,
        ),
        (
            "minecraft:zombie",
            "minecraft:iron_pickaxe",
            "minecraft:stick",
            false,
        ),
        (
            "minecraft:piglin",
            "minecraft:golden_helmet",
            "minecraft:diamond_helmet",
            true,
        ),
        (
            "minecraft:piglin",
            "minecraft:diamond_helmet",
            "minecraft:golden_helmet",
            false,
        ),
        (
            "minecraft:piglin",
            "minecraft:golden_sword",
            "minecraft:diamond_sword",
            true,
        ),
        (
            "minecraft:piglin",
            "minecraft:diamond_sword",
            "minecraft:golden_sword",
            false,
        ),
        (
            "minecraft:drowned",
            "minecraft:trident",
            "minecraft:diamond_sword",
            true,
        ),
        (
            "minecraft:drowned",
            "minecraft:diamond_sword",
            "minecraft:trident",
            false,
        ),
    ];
    for (mob, new, old, expected) in cases {
        assert_eq!(
            better(&state.registry, mob, &stack(new), Some(&stack(old))),
            expected,
            "{mob}: {new} over {old}"
        );
    }
    let mut old = stack("minecraft:iron_helmet");
    enchant(&mut old, "minecraft:binding_curse");
    assert!(!better(
        &state.registry,
        "minecraft:zombie",
        &stack("minecraft:diamond_helmet"),
        Some(&old)
    ));
    let mut old = stack("minecraft:bow");
    old.damage = Some(30);
    let mut new = old.clone();
    assert!(!better(
        &state.registry,
        "minecraft:skeleton",
        &new,
        Some(&old)
    ));
    new.damage = Some(10);
    assert!(better(
        &state.registry,
        "minecraft:skeleton",
        &new,
        Some(&old)
    ));
    new.damage = old.damage;
    enchant(&mut new, "minecraft:power");
    assert!(better(
        &state.registry,
        "minecraft:skeleton",
        &new,
        Some(&old)
    ));
    assert_eq!(
        slot(&state.registry.item_rules(), &stack("minecraft:stone")),
        None
    );
}

#[test]
fn no_pickup_flag_no_ai_dead_delay_distance_and_griefing_are_respected() {
    for reason in [
        "flag", "no_ai", "dead", "delay", "distance", "vertical", "griefing", "worse",
    ] {
        let mut state = state(None);
        let id = collector(&mut state, "minecraft:zombie", POS);
        let slot = state.entities.slot_of(id).unwrap();
        let sword = item(&state, "minecraft:iron_sword", 1);
        let dropped = ground(&mut state, sword.clone(), POS);
        match reason {
            "flag" => state.entities.row_mut(slot).loot.can_pick_up_loot = false,
            "no_ai" => state.entities.row_mut(slot).controls.no_ai = true,
            "dead" => state.entities.body_mut(slot).alive = false,
            "delay" => {
                let slot = state.entities.slot_of(dropped).unwrap();
                state.entities.body_mut(slot).pickup_delay = 10;
            }
            "distance" => state.entities.set_pos(slot, [10.5, 201.0, 4.5]),
            "vertical" => state.entities.set_pos(slot, [4.5, 205.0, 4.5]),
            "griefing" => {
                state
                    .gamerules
                    .insert("mob_griefing".into(), "false".into());
            }
            "worse" => {
                let better = item(&state, "minecraft:diamond_sword", 1);
                Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] =
                    Some(better);
            }
            _ => unreachable!(),
        }
        let before = row(&state, id).appearance.equipment.clone();
        state.appearance_changed.clear();
        let mut changes = ChangeSet::default();
        run(&mut state, &mut changes);
        assert_eq!(
            row(&state, dropped).item.as_ref().unwrap().full_stack(),
            sword,
            "{reason}"
        );
        assert_eq!(row(&state, id).appearance.equipment, before, "{reason}");
        assert!(changes.entities_removed.is_empty(), "{reason}");
        assert!(state.appearance_changed.is_empty(), "{reason}");
    }
}

#[test]
fn swap_preserves_components_and_guarantees_environmental_death_drop() {
    let mut state = state(None);
    let old = item(&state, "minecraft:iron_sword", 1);
    let mut acquired = item(&state, "minecraft:netherite_sword", 1);
    acquired.damage = Some(37);
    acquired.custom_name = Some("Recovered blade".into());
    enchant(&mut acquired, "minecraft:unbreaking");
    // Put item rows before the collector to exercise swap-remove moving it.
    let dropped = ground(&mut state, acquired.clone(), POS);
    let boots = item(&state, "minecraft:iron_boots", 1);
    let footwear = ground(&mut state, boots.clone(), POS);
    let id = collector(&mut state, "minecraft:zombie", POS);
    let slot = state.entities.slot_of(id).unwrap();
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] = Some(old.clone());
    state.entities.row_mut(slot).loot.drop_chances[0] = 2.0;
    let mut changes = ChangeSet::default();
    run(&mut state, &mut changes);
    assert!(state.entities.slot_of(dropped).is_none());
    assert!(state.entities.slot_of(footwear).is_none());
    assert_eq!(
        row(&state, id).appearance.equipment[0],
        Some(acquired.clone())
    );
    assert_eq!(row(&state, id).appearance.equipment[2], Some(boots));
    assert_eq!(changes.entities_spawned.len(), 1);
    assert_eq!(
        row(&state, changes.entities_spawned[0])
            .item
            .as_ref()
            .unwrap()
            .full_stack(),
        old
    );
    let slot = state.entities.slot_of(id).unwrap();
    // commit_death owns only an accepted alive -> dead transition.
    state.entities.row_mut(slot).health = 0.0;
    state.entities.body_mut(slot).alive = false;
    let mut drops = Vec::new();
    crate::entity_loot::commit_death(
        &mut state,
        slot,
        mc_data::loot::entity_26_1_2::EntityDeathCause::new(
            Default::default(),
            mc_data::loot::entity_26_1_2::EntityLootAttack::None,
            None,
        ),
        &mut drops,
    );
    assert!(
        drops
            .iter()
            .any(|id| row(&state, *id).item.as_ref().unwrap().full_stack() == acquired)
    );
    assert!((0..1000).all(|seed| swap_drop(2.0, seed)));
    let ordinary = (0..10_000).filter(|seed| swap_drop(0.085, *seed)).count();
    assert!(
        (1500..2200).contains(&ordinary),
        "vanilla swap probability: {ordinary}"
    );
}

#[test]
fn one_ground_item_has_one_owner_and_partial_stacks_publish_post_images() {
    let mut state = state(None);
    let first = collector(&mut state, "minecraft:zombie", POS);
    let second = collector(&mut state, "minecraft:skeleton", POS);
    let sword = item(&state, "minecraft:diamond_sword", 1);
    let dropped = ground(&mut state, sword, POS);
    let mut changes = ChangeSet::default();
    run(&mut state, &mut changes);
    assert_eq!(
        [first, second]
            .into_iter()
            .filter(|id| row(&state, *id).appearance.equipment[0].is_some())
            .count(),
        1
    );
    assert_eq!(changes.entities_removed, vec![dropped]);
    let helmet = item(&state, "minecraft:iron_helmet", 3);
    let dropped = ground(&mut state, helmet, POS);
    state.appearance_changed.clear();
    run(&mut state, &mut changes);
    assert_eq!(row(&state, dropped).item.as_ref().unwrap().count, 1);
    assert_eq!(
        row(&state, dropped).appearance.item.as_ref().unwrap().count,
        1
    );
    assert!(state.appearance_changed.contains(&dropped));
    assert!(state.appearance_changed.contains(&first));
    assert!(state.appearance_changed.contains(&second));
}

#[test]
fn can_pick_up_loot_defaults_false_and_native_boolean_is_authoritative() {
    use crate::entity_loot::{EntityLootState, from_fields, to_fields};
    use mc_nbt::Tag;
    assert!(!EntityLootState::default().can_pick_up_loot);
    assert!(!from_fields(&[]).unwrap().can_pick_up_loot);
    for enabled in [false, true] {
        let expected = EntityLootState {
            can_pick_up_loot: enabled,
            ..Default::default()
        };
        let mut fields = to_fields(&expected);
        assert!(fields.contains(&("CanPickUpLoot".into(), Tag::Byte(i8::from(enabled)))));
        assert_eq!(from_fields(&fields).unwrap(), expected);
        fields.retain(|(key, _)| key != "solaris:entity_loot");
        assert_eq!(from_fields(&fields).unwrap().can_pick_up_loot, enabled);
    }
    let mut fields = to_fields(&EntityLootState::default());
    fields.retain(|(key, _)| key != "CanPickUpLoot");
    fields.push(("CanPickUpLoot".into(), Tag::Byte(1)));
    assert!(from_fields(&fields).unwrap().can_pick_up_loot);
    assert!(from_fields(&[("CanPickUpLoot".into(), Tag::Int(1))]).is_err());
}

#[test]
fn native_checkpoint_plus_disk_wal_replays_equipment_and_item_custody_once() {
    use crate::dimensions::Dimension;
    use crate::journal::{
        CheckpointManifest, JournalWriter, read_complete_batches_after, replay_batches,
    };
    use crate::native_checkpoint::{encode_entities, record, refresh_metadata};
    use mc_world::native::NativeCheckpoint;

    for count in [1, 3] {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
        let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
        let mut state = state(None);
        state.tick = 1;
        state.day_time = 18000;
        state.gamerules.insert("spawn_mobs".into(), "false".into());
        // The real tick evicts resident chunks without players or tickets.
        state.tickets.insert(
            "equipment-pickup-test".into(),
            (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
        );
        let id = collector(&mut state, "minecraft:zombie", POS);
        let old = item(&state, "minecraft:iron_sword", 1);
        let slot = state.entities.slot_of(id).unwrap();
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] =
            Some(old.clone());
        state.entities.row_mut(slot).loot.drop_chances[0] = 2.0;
        let mut expected = item(&state, "minecraft:diamond_sword", count);
        expected.damage = Some(23);
        expected.custom_name = Some("Durable loot".into());
        let dropped = ground(&mut state, expected.clone(), POS);
        let (_, captured, _) = crate::core::checkpoint_data_for(&state, 1);
        let entities =
            encode_entities(Dimension::Overworld, &captured, &state.registry.items, 1).unwrap();
        let mut image = store.read_checkpoint().unwrap();
        let mut manifest: CheckpointManifest =
            serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
        manifest.tick = 1;
        for dimension in manifest.dimensions.values_mut() {
            dimension.tick = 1;
        }
        image.globals.insert(
            "checkpoint".into(),
            record(1, serde_json::to_vec(&manifest).unwrap()),
        );
        refresh_metadata(&mut image.globals, &manifest).unwrap();
        for record in image.globals.values_mut() {
            record.captured_tick = 1;
        }
        store
            .commit_checkpoint(
                NativeCheckpoint {
                    tick: 1,
                    previous_tick: image.tick,
                    chunks: &[],
                    entities: &entities,
                    players: &[],
                    globals: &image.globals,
                },
                &state.registry.blocks,
                &state.registry.items,
            )
            .unwrap();
        let before = crate::native_checkpoint::recover_with_reader(
            &config,
            Arc::clone(&state.registry),
            store.checkpoint_reader().unwrap(),
        )
        .unwrap();
        assert!(
            before.dimensions[&Dimension::Overworld].entities[&id.0]
                .state
                .loot
                .can_pick_up_loot
        );
        state.appearance_changed.clear();
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        assert!(output.changes.entities_appearance_changed.contains(&id));
        assert_eq!(output.changes.entities_spawned.len(), 1);
        let released = output.changes.entities_spawned[0];
        if count == 1 {
            assert!(output.changes.entities_removed.contains(&dropped));
        } else {
            assert!(
                output
                    .changes
                    .entities_appearance_changed
                    .contains(&dropped)
            );
        }
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
        writer.append(output.tick, output.tick, &entries).unwrap();
        writer.flush().unwrap();
        drop(writer);
        drop(before);
        drop(store);
        let mut recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
        let replay = recovered.dimensions.remove(&Dimension::Overworld).unwrap();
        expected.count = 1;
        assert_eq!(
            replay.entities[&id.0].appearance.equipment[0],
            Some(expected)
        );
        assert_eq!(replay.entities[&id.0].state.loot.drop_chances[0], 2.0);
        assert!(replay.entities[&id.0].state.loot.can_pick_up_loot);
        assert!(replay.entities[&id.0].state.persistent);
        assert_eq!(replay.entities[&released.0].item, Some(old));
        if count == 1 {
            assert!(!replay.entities.contains_key(&dropped.0));
        } else {
            assert_eq!(
                replay.entities[&dropped.0].item.as_ref().unwrap().count,
                count - 1
            );
        }
        let once = serde_json::to_value(&replay.entities).unwrap();
        let batches = read_complete_batches_after(&dir.path().join("journal"), 1).unwrap();
        let twice = replay_batches(replay, &batches);
        assert_eq!(serde_json::to_value(&twice.entities).unwrap(), once);
        drop(recovered);
        let mut restarted =
            crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
        assert!(row(&restarted, id).loot.can_pick_up_loot);
        assert_eq!(row(&restarted, id).loot.drop_chances[0], 2.0);
        assert_eq!(
            row(&restarted, id).appearance.equipment[0],
            row(&state, id).appearance.equipment[0],
        );
        let mut changes = ChangeSet::default();
        run(&mut restarted, &mut changes);
        assert!(changes.entities_spawned.is_empty());
        assert!(changes.entities_removed.is_empty());
    }
}
