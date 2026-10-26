use super::*;
use crate::boundary::{ConnectionId, EntityDataValue, PlayerInput};
use crate::tick::run_tick;

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
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "stand-player".into(),
            permission_level: 0,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(slot, [6.5, 65.0, 8.5]);
    state.entities.set_rot(slot, 70.0, 0.0);
    let stone = state.registry.states.stone;
    for x in 4..12 {
        for z in 4..12 {
            state.world.set_block(x, 64, z, stone);
        }
    }
    state.world.fold_overlay();
    (state, actor)
}
fn item(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap(),
        count,
    )
}
fn place_input(face: u8) -> PlayerInput {
    PlayerInput::UseItemOn {
        connection: ConnectionId(1),
        hand: Hand::Main,
        position: BlockPos { x: 8, y: 64, z: 8 },
        face,
        cursor: [0.5, 1.0, 0.5],
        sequence: 1,
    }
}
fn place(state: &mut TickState, actor: EntityId) -> (EntityId, crate::boundary::TickOutput) {
    state.players.get_mut(&actor).unwrap().inventory.slots[0] =
        item(state, "minecraft:armor_stand", 2);
    let (output, _, _) = run_tick(state, &[place_input(1)]);
    assert_eq!(output.changes.entities_spawned.len(), 1);
    (output.changes.entities_spawned[0], output)
}
fn click(state: &mut TickState, target: EntityId, y: f64) -> crate::boundary::TickOutput {
    run_tick(
        state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target,
            hand: Hand::Main,
            location: [0.0, y, 0.0],
        }],
    )
    .0
}
fn equipment(state: &TickState, target: EntityId, slot: usize) -> Option<ItemStack> {
    state
        .entities
        .row(state.entities.slot_of(target).unwrap())
        .appearance
        .equipment[slot]
        .clone()
}
fn authored(state: &mut TickState, target: EntityId, nbt: Tag) {
    let slot = state.entities.slot_of(target).unwrap();
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        panic!()
    };
    let image = crate::command_summon::prepare(
        state,
        type_id,
        "minecraft:armor_stand",
        state.entities.pos(slot),
        &nbt,
    )
    .unwrap();
    crate::command_summon::apply(state, target, image);
}

#[test]
fn armor_stand_place_rounds_yaw_consumes_once_and_rejects_occupied_or_down_face() {
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    let slot = state.entities.slot_of(stand).unwrap();
    assert_eq!(state.entities.pos(slot), [8.5, 65.0, 8.5]);
    assert_eq!(state.entities.rot(slot)[0], -90.0);
    assert!(state.entities.row(slot).persistent);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    let (output, _, _) = run_tick(&mut state, &[place_input(1), place_input(0)]);
    assert!(output.changes.entities_spawned.is_empty());
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    state.remove_entity(slot);
    let stone = state.registry.states.stone;
    state.world.set_block(8, 66, 8, stone);
    let (output, _, _) = run_tick(&mut state, &[place_input(1)]);
    assert!(output.changes.entities_spawned.is_empty());
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
}

#[test]
fn armor_stand_item_stored_nbt_initializes_small_arms_and_rejects_malformed_pose_without_debit() {
    let (mut state, actor) = fixture();
    let mut stack = item(&state, "minecraft:armor_stand", 2);
    let mut value = Vec::new();
    mc_nbt::write_network_value(
        &mut value,
        &Tag::Compound(vec![
            ("Small".into(), Tag::Byte(1)),
            ("ShowArms".into(), Tag::Byte(1)),
        ]),
    )
    .unwrap();
    stack
        .extended_mut()
        .preserved_components
        .push(mc_data::item_stack::PreservedItemComponent {
            name: "minecraft:entity_data".into(),
            value,
            baseline: None,
        });
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack;
    let (output, _, _) = run_tick(&mut state, &[place_input(1)]);
    assert_eq!(output.changes.entities_spawned.len(), 1);
    let stand = output.changes.entities_spawned[0];
    let slot = state.entities.slot_of(stand).unwrap();
    assert_eq!(state.entities.body(slot).height, f64::from(1.975_f32) / 2.0);
    assert!(
        state
            .entities
            .row(slot)
            .appearance
            .extra
            .contains(&(15, EntityDataValue::Byte(5)))
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    let boots = item(&state, "minecraft:iron_boots", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = boots.clone();
    click(&mut state, stand, 0.15);
    assert_eq!(equipment(&state, stand, 2), Some(boots.clone()));
    click(&mut state, stand, 0.15);
    assert_eq!(state.players[&actor].inventory.slots[0], boots);
    state.remove_entity(slot);
    let mut invalid = item(&state, "minecraft:armor_stand", 1);
    let mut value = Vec::new();
    mc_nbt::write_network_value(
        &mut value,
        &Tag::Compound(vec![("Pose".into(), Tag::Int(3))]),
    )
    .unwrap();
    invalid
        .extended_mut()
        .preserved_components
        .push(mc_data::item_stack::PreservedItemComponent {
            name: "minecraft:entity_data".into(),
            value,
            baseline: None,
        });
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = invalid.clone();
    let (output, _, _) = run_tick(&mut state, &[place_input(1)]);
    assert!(output.changes.entities_spawned.is_empty());
    assert_eq!(state.players[&actor].inventory.slots[0], invalid);
}

#[test]
fn armor_stand_armor_equip_swap_and_height_selected_take_keep_full_stacks() {
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    let pieces = [
        (2, "minecraft:iron_boots", 0.2),
        (3, "minecraft:iron_leggings", 0.7),
        (4, "minecraft:iron_chestplate", 1.3),
        (5, "minecraft:iron_helmet", 1.8),
    ];
    for (slot, name, y) in pieces {
        let mut stack = item(&state, name, 1);
        stack.damage = Some(17);
        stack.custom_name = Some(format!("kept-{slot}").into());
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack.clone();
        click(&mut state, stand, y);
        assert_eq!(equipment(&state, stand, slot), Some(stack));
        assert!(state.players[&actor].inventory.slots[0].is_empty());
    }
    let gold = item(&state, "minecraft:golden_helmet", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = gold.clone();
    let output = click(&mut state, stand, 0.2);
    assert!(output.changes.entities_appearance_changed.contains(&stand));
    assert_eq!(equipment(&state, stand, 5), Some(gold));
    assert_eq!(
        state.players[&actor].inventory.slots[0]
            .custom_name
            .as_deref(),
        Some("kept-5")
    );
    for (slot, _, y) in pieces {
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
        let old = equipment(&state, stand, slot).unwrap();
        click(&mut state, stand, y);
        assert_eq!(state.players[&actor].inventory.slots[0], old);
        assert_eq!(equipment(&state, stand, slot), None);
    }
}

#[test]
fn armor_stand_hand_items_require_stored_arms_and_never_swap_a_stacked_item_into_occupied_slot() {
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    let diamonds = item(&state, "minecraft:diamond", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = diamonds.clone();
    click(&mut state, stand, 1.0);
    assert_eq!(equipment(&state, stand, 0), None);
    assert_eq!(state.players[&actor].inventory.slots[0], diamonds);
    authored(
        &mut state,
        stand,
        Tag::Compound(vec![("ShowArms".into(), Tag::Byte(1))]),
    );
    click(&mut state, stand, 1.0);
    assert_eq!(
        equipment(&state, stand, 0),
        Some(item(&state, "minecraft:diamond", 1))
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
    let gold = item(&state, "minecraft:gold_ingot", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = gold.clone();
    click(&mut state, stand, 1.0);
    assert_eq!(state.players[&actor].inventory.slots[0], gold);
    state.players.get_mut(&actor).unwrap().inventory.slots[0].count = 1;
    click(&mut state, stand, 1.0);
    assert_eq!(
        equipment(&state, stand, 0),
        Some(item(&state, "minecraft:gold_ingot", 1))
    );
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        item(&state, "minecraft:diamond", 1)
    );
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
    click(&mut state, stand, 1.0);
    assert_eq!(equipment(&state, stand, 0), None);
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        item(&state, "minecraft:gold_ingot", 1)
    );
}

#[test]
fn armor_stand_two_quick_punches_drop_stand_and_every_equipped_stack_once() {
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    let boots = item(&state, "minecraft:iron_boots", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = boots.clone();
    click(&mut state, stand, 0.2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
    let attack = PlayerInput::Attack {
        connection: ConnectionId(1),
        target: stand,
    };
    let health = state
        .entities
        .row(state.entities.slot_of(stand).unwrap())
        .health;
    let (output, _, _) = run_tick(&mut state, std::slice::from_ref(&attack));
    assert!(output.changes.entities_removed.is_empty());
    assert!(output.changes.entities_spawned.is_empty());
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(stand).unwrap())
            .health,
        health
    );
    for _ in 0..6 {
        run_tick(&mut state, &[]);
    }
    let (output, _, _) = run_tick(&mut state, std::slice::from_ref(&attack));
    assert!(
        output.changes.entities_removed.is_empty(),
        "slow second punch starts a fresh wobble"
    );
    let (output, _, _) = run_tick(&mut state, &[attack.clone(), attack]);
    assert_eq!(output.changes.entities_removed, vec![stand]);
    assert_eq!(output.changes.entities_spawned.len(), 2);
    let drops: Vec<_> = output
        .changes
        .entities_spawned
        .iter()
        .map(|id| {
            state
                .entities
                .row(state.entities.slot_of(*id).unwrap())
                .item
                .as_ref()
                .unwrap()
                .full_stack()
        })
        .collect();
    assert!(drops.contains(&boots));
    assert!(drops.contains(&item(&state, "minecraft:armor_stand", 1)));
    assert!(state.entities.slot_of(stand).is_none());
}

#[test]
fn armor_stand_recent_environmental_damage_does_not_count_as_the_first_punch() {
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    let slot = state.entities.slot_of(stand).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 1.0,
            kind: crate::survival::DamageKind::InFire,
            source: None,
        },
    ));
    state.commit_merge_effects();
    assert_eq!(state.entities.body(slot).hurt_at, Some(state.tick));
    let attack = PlayerInput::Attack {
        connection: ConnectionId(1),
        target: stand,
    };
    let (output, _, _) = run_tick(&mut state, std::slice::from_ref(&attack));
    assert!(output.changes.entities_removed.is_empty());
    assert!(state.entities.slot_of(stand).is_some());
    let (output, _, _) = run_tick(&mut state, &[attack]);
    assert_eq!(output.changes.entities_removed, vec![stand]);
}

#[test]
fn armor_stand_disabled_slots_creative_copy_and_invalid_hit_do_not_duplicate_items() {
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    authored(
        &mut state,
        stand,
        Tag::Compound(vec![("DisabledSlots".into(), Tag::Int(1 << 4))]),
    );
    let helmet = item(&state, "minecraft:iron_helmet", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = helmet.clone();
    click(&mut state, stand, 1.8);
    assert_eq!(equipment(&state, stand, 5), None);
    assert_eq!(state.players[&actor].inventory.slots[0], helmet);
    authored(
        &mut state,
        stand,
        Tag::Compound(vec![("DisabledSlots".into(), Tag::Int(0))]),
    );
    click(&mut state, stand, f64::NAN);
    assert_eq!(equipment(&state, stand, 5), None);
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    click(&mut state, stand, 1.8);
    assert_eq!(equipment(&state, stand, 5), Some(helmet.clone()));
    assert_eq!(state.players[&actor].inventory.slots[0], helmet);
}

#[test]
fn armor_stand_equipment_and_stored_pose_survive_disk_wal_native_checkpoint_restart_and_anvil_export()
 {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    let pose = Tag::Compound(vec![(
        "Head".into(),
        Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::FLOAT,
            elements: vec![Tag::Float(10.0), Tag::Float(20.0), Tag::Float(30.0)],
        }),
    )]);
    authored(
        &mut state,
        stand,
        Tag::Compound(vec![
            ("ShowArms".into(), Tag::Byte(1)),
            ("Pose".into(), pose.clone()),
        ]),
    );
    let (mut output, _, _) = run_tick(&mut state, &[]);
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_spawned
        .push(stand);
    let birth = crate::core::journal_entries_for_tick(&mut state, &output);
    let root = std::env::temp_dir().join(format!("solaris-stand-{}", uuid::Uuid::new_v4()));
    let mut writer = JournalWriter::open(&root, 1).unwrap();
    writer.append(1, output.tick, &birth).unwrap();
    let mut helmet = item(&state, "minecraft:iron_helmet", 1);
    helmet.damage = Some(13);
    helmet.custom_name = Some("retained helmet".into());
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = helmet.clone();
    let output = click(&mut state, stand, 1.8);
    let equip = crate::core::journal_entries_for_tick(&mut state, &output);
    writer.append(output.tick, output.tick, &equip).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&root.join("journal"), 0).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    let replay = replay_batches(replay, &batches);
    assert_eq!(replay.entities.len(), 1);
    assert_eq!(
        replay.entities[&stand.0].appearance.equipment[5],
        Some(helmet.clone())
    );
    let (mut restarted, _) = fixture();
    crate::core::restore_entity(&mut restarted, stand.0, &replay.entities[&stand.0]).unwrap();
    assert_eq!(equipment(&restarted, stand, 5), Some(helmet.clone()));
    let (_, chunks, _) = crate::core::checkpoint_data_for(&restarted, restarted.tick);
    let native = crate::native_checkpoint::encode_entities(
        crate::dimensions::Dimension::Overworld,
        &chunks,
        &restarted.registry.items,
        restarted.tick,
    )
    .unwrap();
    let config = crate::CoreConfig::new(root.join("checkpoint"), 42);
    crate::native_checkpoint::tests::write_manifest(&config, &Default::default()).unwrap();
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let base = store.read_checkpoint().unwrap();
    store
        .commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick: restarted.tick,
                previous_tick: base.tick,
                chunks: &[],
                entities: &native,
                players: &[],
                globals: &base.globals,
            },
            &restarted.registry.blocks,
            &restarted.registry.items,
        )
        .unwrap();
    drop(store);
    let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
    let native_replay = &recovered.dimensions[&crate::dimensions::Dimension::Overworld];
    let (mut restored, actor) = fixture();
    crate::core::restore_entity(&mut restored, stand.0, &native_replay.entities[&stand.0]).unwrap();
    assert_eq!(equipment(&restored, stand, 5), Some(helmet.clone()));
    let row = restored
        .entities
        .row(restored.entities.slot_of(stand).unwrap());
    assert!(
        row.appearance
            .extra
            .contains(&(15, EntityDataValue::Byte(4)))
    );
    assert!(
        row.appearance
            .extra
            .contains(&(16, EntityDataValue::Rotations([10.0, 20.0, 30.0])))
    );
    let (_, exported, _) = crate::core::checkpoint_data_for(&restored, restored.tick);
    let anvil = root.join("anvil-entities");
    mc_world::anvil::write_entity_regions(
        &anvil,
        &restored.registry.items,
        &exported,
        restored.tick as u32,
    )
    .unwrap();
    let exported = mc_world::anvil::read_entity_regions(&anvil, &restored.registry.items).unwrap();
    let record = exported
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.wire_id == stand.0)
        .unwrap();
    assert!(
        record
            .extra_fields
            .contains(&("ShowArms".into(), Tag::Byte(1)))
    );
    assert!(record.extra_fields.contains(&("Pose".into(), pose)));
    assert_eq!(
        crate::entity_loot::equipment_from_fields(&restored.registry.items, &record.extra_fields)
            .unwrap()[5],
        Some(helmet.clone())
    );
    restored.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
    click(&mut restored, stand, 1.8);
    assert_eq!(restored.players[&actor].inventory.slots[0], helmet);
    drop(recovered);
    std::fs::remove_dir_all(root).unwrap();
}

fn display_equipment(state: &mut TickState, stand: EntityId) -> [ItemStack; 8] {
    let names = [
        "minecraft:diamond_sword",
        "minecraft:shield",
        "minecraft:iron_boots",
        "minecraft:iron_leggings",
        "minecraft:iron_chestplate",
        "minecraft:iron_helmet",
        "minecraft:wolf_armor",
        "minecraft:saddle",
    ];
    let stacks = std::array::from_fn(|index| {
        let mut stack = item(state, names[index], 1);
        stack.custom_name = Some(format!("display slot {index}").into());
        if index != 7 {
            stack.damage = Some(17);
        }
        stack
    });
    let row = state
        .entities
        .row_mut(state.entities.slot_of(stand).unwrap());
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.equipment = std::array::from_fn(|index| Some(stacks[index].clone()));
    appearance.body_armor = Some(stacks[6].clone());
    appearance.saddle = Some(stacks[7].clone());
    row.loot.body_armor = Some(Box::new(stacks[6].clone()));
    row.loot.saddle = Some(Box::new(stacks[7].clone()));
    stacks
}

fn dropped_stacks(state: &TickState, spawned: &[EntityId]) -> Vec<ItemStack> {
    spawned
        .iter()
        .map(|id| {
            state
                .entities
                .row(state.entities.slot_of(*id).unwrap())
                .item
                .as_ref()
                .unwrap()
                .full_stack()
        })
        .collect()
}

fn lethal_damage(
    state: &mut TickState,
    stand: EntityId,
    kind: crate::survival::DamageKind,
) -> Vec<ItemStack> {
    let slot = state.entities.slot_of(stand).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 1000.0,
            kind,
            source: None,
        },
    ));
    let (deaths, _, spawned, _) = state.commit_merge_effects();
    assert_eq!(deaths, vec![stand]);
    assert!(state.entities.slot_of_any(stand).is_none());
    assert!(!state.armor_stand_hits.contains_key(&stand));
    let (_, _, repeated, _) = state.commit_merge_effects();
    assert!(repeated.is_empty(), "later merges must not duplicate gear");
    dropped_stacks(state, &spawned)
}

#[test]
fn armor_stand_environmental_death_drops_all_eight_equipment_slots_without_mob_drop_rolls_or_wear()
{
    use crate::survival::DamageKind;
    for kind in [DamageKind::OnFire, DamageKind::Explosion, DamageKind::Lava] {
        let (mut state, actor) = fixture();
        let (stand, _) = place(&mut state, actor);
        state.gamerules.insert("mob_drops".into(), "false".into());
        let stacks = display_equipment(&mut state, stand);
        let drops = lethal_damage(&mut state, stand, kind);
        assert_eq!(drops.len(), 8, "{kind:?}");
        for stack in stacks {
            assert!(
                drops.contains(&stack),
                "{kind:?}: lost or altered {stack:?}"
            );
        }
        assert!(!drops.contains(&item(&state, "minecraft:armor_stand", 1)));
    }
}

#[test]
fn armor_stand_projectile_death_drops_named_stand_and_all_nonvanishing_equipment() {
    use crate::survival::DamageKind;
    for kind in [DamageKind::Projectile, DamageKind::Fireball] {
        let (mut state, actor) = fixture();
        let (stand, _) = place(&mut state, actor);
        let mut stacks = display_equipment(&mut state, stand);
        stacks[5].enchantments.push(mc_data::ItemEnchantment {
            id: Identifier::parse("minecraft:vanishing_curse").unwrap(),
            level: 1,
        });
        let slot = state.entities.slot_of(stand).unwrap();
        let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
        appearance.equipment[5] = Some(stacks[5].clone());
        appearance.custom_name = Some("named display".into());
        let drops = lethal_damage(&mut state, stand, kind);
        assert_eq!(drops.len(), 8);
        assert!(!drops.contains(&stacks[5]));
        for index in [0, 1, 2, 3, 4, 6, 7] {
            assert!(drops.contains(&stacks[index]));
        }
        let mut named = item(&state, "minecraft:armor_stand", 1);
        named.custom_name = Some("named display".into());
        assert!(drops.contains(&named));
    }
}

#[test]
fn armor_stand_creative_first_punch_kills_without_stand_or_equipment_drops() {
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    display_equipment(&mut state, stand);
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    state.armor_stand_hits.insert(stand, state.tick);
    let (output, _, _) = run_tick(
        &mut state,
        &[PlayerInput::Attack {
            connection: ConnectionId(1),
            target: stand,
        }],
    );
    assert_eq!(output.changes.entities_removed, vec![stand]);
    assert!(output.changes.entities_spawned.is_empty());
    assert!(state.entities.slot_of_any(stand).is_none());
    assert!(!state.armor_stand_hits.contains_key(&stand));
}

#[test]
fn armor_stand_void_kill_and_disabled_entity_drops_discard_stand_and_equipment() {
    use crate::survival::DamageKind;
    for (kind, drops_enabled) in [
        (DamageKind::Void, true),
        (DamageKind::Explosion, false),
        (DamageKind::Projectile, false),
    ] {
        let (mut state, actor) = fixture();
        let (stand, _) = place(&mut state, actor);
        display_equipment(&mut state, stand);
        state
            .gamerules
            .insert("entity_drops".into(), drops_enabled.to_string());
        assert!(lethal_damage(&mut state, stand, kind).is_empty());
    }
}

#[test]
fn armor_stand_survival_punch_drops_named_stand_but_not_vanishing_equipment() {
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    let mut helmet = item(&state, "minecraft:iron_helmet", 1);
    helmet.enchantments.push(mc_data::ItemEnchantment {
        id: Identifier::parse("minecraft:vanishing_curse").unwrap(),
        level: 1,
    });
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = helmet;
    click(&mut state, stand, 1.8);
    let slot = state.entities.slot_of(stand).unwrap();
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name =
        Some("named punch display".into());
    let attack = PlayerInput::Attack {
        connection: ConnectionId(1),
        target: stand,
    };
    let (output, _, _) = run_tick(&mut state, &[attack.clone(), attack]);
    assert_eq!(output.changes.entities_removed, vec![stand]);
    let drops = dropped_stacks(&state, &output.changes.entities_spawned);
    let mut named = item(&state, "minecraft:armor_stand", 1);
    named.custom_name = Some("named punch display".into());
    assert_eq!(drops, vec![named]);
}

#[test]
fn armor_stand_named_tag_renames_normal_and_marker_stands_instead_of_equipping() {
    for marker in [false, true] {
        let (mut state, actor) = fixture();
        let (stand, _) = place(&mut state, actor);
        let mut fields = vec![("ShowArms".into(), Tag::Byte(1))];
        if marker {
            fields.push(("Marker".into(), Tag::Byte(1)));
        }
        authored(&mut state, stand, Tag::Compound(fields));
        let name = "{\"text\":\"Named display\"}";
        let mut tag = item(&state, "minecraft:name_tag", 2);
        tag.custom_name = Some(name.into());
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = tag.clone();
        click(&mut state, stand, 0.0);
        let row = state.entities.row(state.entities.slot_of(stand).unwrap());
        assert_eq!(row.appearance.custom_name.as_deref(), Some(name));
        assert!(
            !row.appearance.custom_name_visible,
            "name tags do not enable visibility"
        );
        assert!(row.persistent);
        assert!(row.appearance.equipment.iter().all(Option::is_none));
        tag.count = 1;
        assert_eq!(state.players[&actor].inventory.slots[0], tag);
    }
}

#[test]
fn weak_arrow_breaks_stand_outright() {
    // ARMOR-STAND-PROJECTILE-BREAK: any arrow breaks the stand.
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    let slot = state.entities.slot_of(stand).expect("stand");
    assert!(crate::projectiles::payloads::breaks_armor_stand(&state, stand));
    assert!(state.entities.body(slot).alive);
}

#[test]
fn splash_potion_ignores_stand_but_hits_cow() {
    // ARMOR-STAND-POTION-IMMUNITY: stands reject splash effects.
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    let cow_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let _cow = state.spawn_mob(cow_type, [6.5, 65.0, 10.5], 10.0);
    let mut intents = Vec::new();
    let stack = item(&state, "minecraft:splash_potion", 1);
    let record = crate::projectiles::ProjectileState::new(
        crate::projectiles::ProjectileKind::Potion,
        None,
        stack,
    );
    let id = state.spawn_mob(cow_type, [6.5, 66.0, 10.5], 10.0);
    crate::projectiles::payloads::apply_potion(
        &mut state,
        &mut intents,
        id,
        &record,
        stand,
        1.0,
        false,
    );
    let slot = state.entities.slot_of(stand).expect("stand");
    assert!(
        state.entities.row(slot).active_effects.entries.is_empty(),
        "stand must not gain potion effects"
    );
}

#[test]
fn armor_stand_marker_equipment_interaction_passes_without_mutating_custody() {
    let (mut state, actor) = fixture();
    let (stand, _) = place(&mut state, actor);
    authored(
        &mut state,
        stand,
        Tag::Compound(vec![
            ("Marker".into(), Tag::Byte(1)),
            ("ShowArms".into(), Tag::Byte(1)),
        ]),
    );
    let held = item(&state, "minecraft:diamond", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = held.clone();
    assert!(!interact(
        &mut state,
        actor,
        stand,
        Hand::Main,
        [0.0, 0.0, 0.0],
    ));
    assert_eq!(equipment(&state, stand, 0), None);
    assert_eq!(state.players[&actor].inventory.slots[0], held);
}
