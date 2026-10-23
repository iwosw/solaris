//! Journal kill-recovery tests: abrupt stop (no shutdown) loses at most
//! the unflushed tail, all domains at the same tick. Sibling `*_tests.rs`.

use crate::journal::{
    JournalEntry, JournalWriter, Replay, SimDebtImage, player_from_tag, player_to_tag,
    read_complete_batches_after, replay_batches,
};

#[test]
fn recovered_writer_reuses_exact_validated_interval_without_decoding_it_again() {
    let root = tempfile::tempdir().unwrap();
    let mut original = JournalWriter::open(root.path(), 1).unwrap();
    original.append(1, 2, &[]).unwrap();
    drop(original);
    let recovered =
        crate::journal::visit_complete_batches_after(&root.path().join("journal"), 0, |_| Ok(()))
            .unwrap();
    assert_eq!(recovered, 2);
    let mut writer = JournalWriter::open_recovered(root.path(), 1, 0, recovered).unwrap();
    // Make a second read observable without a production-only counter/hook.
    // Production retains immutable published files under the exclusive world lease.
    std::fs::rename(
        root.path()
            .join("journal/batch-00000000000000000001-00000000000000000002.bin"),
        root.path().join("already-validated.bin"),
    )
    .unwrap();
    // A save immediately after recovery needs no synthetic tick or append.
    writer.gate(0, recovered).unwrap();
    writer.gate(1, recovered).unwrap();
    assert!(
        writer.gate(0, recovered + 1).is_err(),
        "verified coverage cannot invent a later tick"
    );
    writer.append(3, 3, &[]).unwrap();
    writer.gate(0, 3).unwrap();
    drop(writer);

    let mut later_start = JournalWriter::open_recovered(root.path(), 1, 1, 3).unwrap();
    assert!(
        later_start.gate(0, 3).is_err(),
        "the verified interval does not cover tick one without appends either"
    );
    later_start.append(4, 4, &[]).unwrap();
    assert!(
        later_start.gate(0, 4).is_err(),
        "the verified interval does not cover tick one"
    );
    drop(later_start);
    let mut earlier_end = JournalWriter::open_recovered(root.path(), 1, 0, 2).unwrap();
    earlier_end.append(5, 5, &[]).unwrap();
    assert!(
        earlier_end.gate(0, 5).is_err(),
        "new pre-open coverage must still be verified"
    );
}

#[test]
fn unverified_writer_still_refuses_corrupt_preopen_tail() {
    let root = tempfile::tempdir().unwrap();
    let mut original = JournalWriter::open(root.path(), 1).unwrap();
    original.append(1, 2, &[]).unwrap();
    drop(original);
    std::fs::write(
        root.path()
            .join("journal/batch-00000000000000000001-00000000000000000002.bin"),
        b"corrupt published batch",
    )
    .unwrap();
    let mut writer = JournalWriter::open(root.path(), 1).unwrap();
    writer.append(3, 3, &[]).unwrap();
    assert!(writer.gate(0, 3).is_err());
}

/// Fold-over-base: tail removals delete base records (no dupe),
/// tail vitals update base health (no rollback), tail entity images keep
/// base motion.
#[test]
fn fold_applies_tail_over_checkpoint_base() {
    use crate::journal::{EntityAppearanceImage, ReplayEntity};
    use std::collections::BTreeMap;
    let mut base = Replay::default();
    base.entities.insert(
        7,
        ReplayEntity {
            uuid: "item-uuid".to_owned(),
            type_id: 99,
            pos: [10.0, 80.0, 10.0],
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            ground: true,
            health: 1.0,
            alive: true,
            age: 5,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            appearance: EntityAppearanceImage::default(),
            active_effects: Default::default(),
            villager: None,
            projectile: None,
            state: Default::default(),
            tick: 10,
        },
    );
    base.entities.insert(
        9,
        ReplayEntity {
            uuid: "mob-uuid".to_owned(),
            type_id: 100,
            pos: [20.0, 80.0, 20.0],
            vel: [1.0, 0.0, 0.0],
            yaw: 1.0,
            pitch: 0.5,
            ground: false,
            health: 20.0,
            alive: true,
            age: 0,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            appearance: EntityAppearanceImage::default(),
            active_effects: Default::default(),
            villager: None,
            projectile: None,
            state: Default::default(),
            tick: 10,
        },
    );
    let batches = vec![(
        11u64,
        12u64,
        vec![
            // Ground item picked up after the checkpoint.
            JournalEntry::RemoveEntity { id: 7, tick: 11 },
            // Mob damaged after the checkpoint.
            JournalEntry::EntityVitals {
                id: 9,
                health: 14.0,
                alive: true,
                tick: 12,
            },
        ],
    )];
    let replay = replay_batches(base, &batches);
    assert!(
        !replay.entities.contains_key(&7),
        "picked-up item stays gone"
    );
    let mob = replay.entities.get(&9).expect("mob survives");
    assert_eq!(mob.health, 14.0, "post-damage health, no rollback");
    assert_eq!(mob.pos, [20.0, 80.0, 20.0], "base motion kept");
    assert_eq!(mob.vel, [1.0, 0.0, 0.0]);
    let _ = BTreeMap::<i32, i32>::new();
}

/// Kill-style abrupt stop: published batches replay, while an unpublished
/// temporary batch is ignored; manifest + replay converge to one tick.
#[test]
fn abrupt_stop_loses_only_unflushed_tail() {
    let dir = std::env::temp_dir().join(format!("mc-core-kill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    {
        let mut writer = JournalWriter::open(&dir, 1000).expect("open");
        for tick in 1..=5u64 {
            writer
                .append(
                    tick,
                    tick,
                    &[
                        JournalEntry::Block {
                            pos: [4, 64, 4],
                            state: tick as u32,
                        },
                        JournalEntry::Player {
                            uuid: "00000000-0000-0000-0000-000000000001".to_owned(),
                            opaque_player_fields: Vec::new(),
                            statistics: Default::default(),
                            advancement_progress: Some(crate::journal::empty_advancement_progress()),
                            unlocked_recipes: Some(Default::default()),
                            dimension: Default::default(),
                            pos: [tick as f64, 80.0, 0.0],
                            yaw: 0.0,
                            pitch: 0.0,
                            health: 20.0,
                            food: 20,
                            saturation: 5.0,
                            exhaustion: 0.0,
                            air: 300,
                            experience_level: 0,
                            experience_progress: 0.0,
                            experience_total: 0,
                            game_mode: 0,
                            selected: 0,
                            inventory: vec![mc_data::ItemStack::EMPTY; 41],
                            carried: mc_data::ItemStack::EMPTY,
                            grid: Vec::new(),
                            respawn: None,
                            respawn_dimension: Default::default(),
                            respawn_forced: false,
                            respawn_yaw: 0.0,
                            respawn_pitch: 0.0,
                            active_effects: Default::default(),
                            enchantment_seed: 0x12345678,
                            enchantment_offer_seed: 12345,
                            warden_warning: Default::default(),
                            current_impulse_impact_pos: None,
                            fall_flying: false,
                            burn_ticks: 0,
                            portal: Default::default(),
                            tick,
                        },
                        JournalEntry::Entity {
                            id: 7,
                            uuid: "00000000-0000-0000-0000-000000000002".to_owned(),
                            type_id: 99,
                            pos: [8.0, 80.0, 8.0],
                            yaw: 0.0,
                            pitch: 0.0,
                            health: 10.0,
                            alive: true,
                            age: tick as u32,
                            breed_cooldown: 0,
                            pickup_delay: 0,
                            item: None,
                            appearance: crate::journal::EntityAppearanceImage::default(),
                            active_effects: Default::default(),
                            villager: None,
                            projectile: None,
                            vel: [0.0; 3],
                            state: Default::default(),
                            tick,
                        },
                    ],
                )
                .expect("append");
        }
        writer.flush().expect("flush");
        // NO shutdown: drop the writer without shutdown = kill.
    }
    // Unpublished partial batch written by the dying process.
    std::fs::write(
        dir.join("journal")
            .join("batch-00000000000000000006-00000000000000000006.bin.tmp"),
        b"SOLJ-partial",
    )
    .expect("torn");
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    assert_eq!(batches.len(), 5);
    let replay = replay_batches(Replay::default(), &batches);
    // All domains converge to the same tick.
    assert_eq!(replay.tick, 5);
    assert_eq!(replay.blocks.get(&(4, 64, 4)), Some(&5));
    let player = replay
        .players
        .get("00000000-0000-0000-0000-000000000001")
        .expect("player");
    assert_eq!(player.tick, 5);
    assert_eq!(player.inventory.len(), 41);
    assert_eq!(player.enchantment_seed, 0x12345678);
    assert_eq!(player.enchantment_offer_seed, 12345);
    let entity = replay.entities.get(&7).expect("entity");
    assert_eq!(entity.tick, 5);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Crash after a tick that changed an enchanted item entity and a player
/// inventory: recovered state equals the post-image exactly (no identity or
/// component loss).
#[test]
fn crash_recovers_enchanted_item_and_player_exactly() {
    use crate::journal::EntityAppearanceImage;
    let dir = std::env::temp_dir().join(format!("mc-core-enchant-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    // Enchanted diamond sword: components must survive the round-trip.
    let mut sword = mc_data::ItemStack::new(999, 1);
    sword.damage = Some(42);
    sword.custom_name = Some("Oathblade".to_owned().into_boxed_str());
    sword.enchantments = vec![mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:sharpness").expect("parses"),
        level: 5,
    }];
    let appearance = EntityAppearanceImage {
        item: Some(sword.clone()),
        experience_orb: None,
        spawn_data: 0,
        equipment: vec![None, None, None, None, None, Some(sword.clone())],
        baby: true,
        pose: 2,
        shared_flags: 0x01,
        health: 10.0,
        custom_name: Some("Boss".to_owned()),
        custom_name_visible: true,
        extra: vec![(16, crate::journal::EntityDataValueImage::VarInt(3))],
    };
    {
        let mut writer = JournalWriter::open(&dir, 1000).expect("open");
        writer
            .append(
                1,
                9,
                &[JournalEntry::Entity {
                    id: 11,
                    uuid: "00000000-0000-0000-0000-000000000011".to_owned(),
                    type_id: 100,
                    pos: [8.0, 80.0, 8.0],
                    yaw: 0.0,
                    pitch: 0.0,
                    health: 10.0,
                    alive: true,
                    age: 77,
                    breed_cooldown: 5,
                    pickup_delay: 0,
                    item: Some(sword.clone()),
                    appearance: appearance.clone(),
                    active_effects: Default::default(),
                    villager: None,
                    projectile: None,
                    vel: [0.0; 3],
                    state: Default::default(),
                    tick: 9,
                }],
            )
            .expect("append");
        writer.flush().expect("flush");
        // Kill: no shutdown.
    }
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    assert_eq!(batches.len(), 1);
    let replay = replay_batches(Replay::default(), &batches);
    let entity = replay.entities.get(&11).expect("entity");
    assert_eq!(entity.health, 10.0);
    assert_eq!(entity.age, 77);
    assert_eq!(entity.breed_cooldown, 5);
    assert_eq!(entity.item, Some(sword.clone()));
    assert_eq!(entity.appearance, appearance);
    assert_eq!(
        entity
            .appearance
            .item
            .as_ref()
            .expect("item")
            .enchantments
            .len(),
        1
    );
    assert_eq!(
        entity.appearance.equipment[5]
            .as_ref()
            .expect("helm")
            .damage,
        Some(42)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Step 2: the checkpoint `.dat` is vanilla-readable — shared vanilla keys
/// (`SelectedItemSlot`, `Inventory` list with `Slot`, component stacks) parse
/// through the canonical codec; Solaris custody rides alongside.
#[test]
fn checkpoint_dat_is_vanilla_readable() {
    use crate::journal::ReplayPlayer;
    let items = mc_data::items::solaris_required_items();
    let sword_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond_sword").expect("id"))
        .expect("sword");
    let record = ReplayPlayer {
        opaque_player_fields: Vec::new(),
        statistics: Default::default(),
        advancement_progress: crate::journal::empty_advancement_progress(),
        unlocked_recipes: Default::default(),
        dimension: Default::default(),
        pos: [5.0, 80.0, 5.0],
        yaw: 1.0,
        pitch: -0.5,
        health: 14.0,
        food: 17,
        saturation: 3.0,
        exhaustion: 1.0,
        air: 290,
        experience_level: 7,
        experience_progress: 0.3,
        experience_total: 100,
        game_mode: 0,
        selected: 2,
        inventory: vec![mc_data::ItemStack::EMPTY; 41],
        carried: mc_data::ItemStack::EMPTY,
        grid: Vec::new(),
        respawn: Some([3, 64, -7]),
        respawn_dimension: Default::default(),
        respawn_forced: true,
        respawn_yaw: 120.0,
        respawn_pitch: -30.0,
        active_effects: Default::default(),
        enchantment_seed: 0x12345678,
        enchantment_offer_seed: 12345,
        warden_warning: Default::default(),
        current_impulse_impact_pos: None,
        fall_flying: false,
        burn_ticks: 0,
        portal: Default::default(),
        tick: 9,
    };
    let mut record = record;
    record.inventory[0] = mc_data::ItemStack {
        item_id: sword_id,
        count: 1,
        damage: Some(37),
        enchantments: vec![mc_data::ItemEnchantment {
            id: mc_data::Identifier::parse("minecraft:sharpness").expect("e"),
            level: 3,
        }],
        custom_name: Some(" quests blade ".into()),
        ..mc_data::ItemStack::EMPTY
    };
    let root = player_to_tag(&record, &items).expect("encode");
    let back = player_from_tag(&root, &items).expect("decode");
    assert_eq!(back, record);
    assert_eq!(back.pos, record.pos);
    assert_eq!(back.selected, 2);
    assert_eq!(back.experience_level, 7);
    assert_eq!(back.experience_total, 100);
    assert_eq!(back.enchantment_seed, record.enchantment_seed);
    assert_eq!(back.enchantment_offer_seed, record.enchantment_offer_seed);
    assert_eq!(back.inventory[0], record.inventory[0]);
    assert_eq!(back.inventory[0].damage, Some(37));
    assert_eq!(
        back.inventory[0].custom_name.as_deref(),
        Some(" quests blade ")
    );
    assert_eq!(back.inventory[0].enchantments.len(), 1);
    assert_eq!(back.health, 14.0);
    assert_eq!(back.respawn, Some([3, 64, -7]));
}

/// An entity born after the checkpoint replays at its journaled spawn
/// coordinates — never the invented `[8, 80, 8]`. Pre-fix this test fails:
/// the spawn entry carried no position and replay fabricated one.
#[test]
fn newborn_entity_replays_at_journaled_spawn_pos() {
    use crate::journal::EntityAppearanceImage;
    let batches = vec![(
        11u64,
        11u64,
        vec![JournalEntry::Entity {
            id: 21,
            uuid: "00000000-0000-0000-0000-000000000021".to_owned(),
            type_id: 100,
            health: 10.0,
            alive: true,
            age: 0,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            appearance: EntityAppearanceImage::default(),
            active_effects: Default::default(),
            villager: None,
            projectile: None,
            state: Default::default(),
            vel: [0.0; 3],
            tick: 11,
            pos: [123.5, 80.0, -45.5],
            yaw: 1.5,
            pitch: -0.25,
        }],
    )];
    let replay = replay_batches(Replay::default(), &batches);
    let entity = replay.entities.get(&21).expect("newborn replays");
    assert_eq!(
        entity.pos,
        [123.5, 80.0, -45.5],
        "true spawn pos, not [8,80,8]"
    );
    assert_eq!((entity.yaw, entity.pitch), (1.5, -0.25));
}

/// Ordinary motion within a chunk remains checkpoint-owned; a cross-chunk
/// image, unlike this fixture, must move the replay's simulation clock owner.
#[test]
fn journaled_spawn_pos_does_not_teleport_checkpoint_base() {
    use crate::journal::{EntityAppearanceImage, ReplayEntity};
    let mut base = Replay::default();
    base.entities.insert(
        22,
        ReplayEntity {
            uuid: "mob-uuid".to_owned(),
            type_id: 100,
            pos: [20.0, 80.0, 20.0],
            vel: [1.0, 0.0, 0.0],
            yaw: 1.0,
            pitch: 0.5,
            ground: false,
            health: 20.0,
            alive: true,
            age: 0,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            appearance: EntityAppearanceImage::default(),
            active_effects: Default::default(),
            villager: None,
            projectile: None,
            state: Default::default(),
            tick: 10,
        },
    );
    let batches = vec![(
        11u64,
        11u64,
        vec![JournalEntry::Entity {
            id: 22,
            uuid: "mob-uuid".to_owned(),
            type_id: 100,
            health: 19.0,
            alive: true,
            age: 1,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            appearance: EntityAppearanceImage::default(),
            active_effects: Default::default(),
            villager: None,
            projectile: None,
            state: Default::default(),
            vel: [0.0; 3],
            tick: 11,
            pos: [21.0, 80.0, 21.0],
            yaw: 0.0,
            pitch: 0.0,
        }],
    )];
    let replay = replay_batches(base, &batches);
    let mob = replay.entities.get(&22).expect("mob survives");
    assert_eq!(mob.pos, [20.0, 80.0, 20.0], "base motion wins");
    assert_eq!(mob.health, 19.0, "non-motion fields still fold");
}

/// Block-entity tombstone deletes the checkpoint base: a broken chest's
/// storage never resurrects. Pre-fix there was no tombstone and the base
/// record survived replay.
#[test]
fn remove_block_entity_deletes_checkpoint_base() {
    use crate::journal::ReplayBlockEntity;
    let mut base = Replay::default();
    base.block_entities.insert(
        (4, 64, 4),
        ReplayBlockEntity {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
            tick: 10,
        },
    );
    let batches = vec![(
        11u64,
        11u64,
        vec![JournalEntry::RemoveBlockEntity {
            pos: [4, 64, 4],
            tick: 11,
        }],
    )];
    let replay = replay_batches(base, &batches);
    assert!(
        !replay.block_entities.contains_key(&(4, 64, 4)),
        "broken container stays gone"
    );
}

/// The checkpoint `.dat` is vanilla-format: `Inventory` holds only storage
/// slots 0-35 (vanilla `ddl.java:349-365`), armor/offhand ride the
/// `equipment` compound (vanilla `chl.java:622`). Pre-fix the hotbar wrote
/// as 36-44 and armor as 5-8/45, which vanilla discards/misloads.
#[test]
fn checkpoint_dat_uses_vanilla_slots_and_equipment() {
    use crate::journal::ReplayPlayer;
    use mc_data::player_nbt::{VanillaPlayer, player_from_fields};
    let items = mc_data::items::solaris_required_items();
    let boots_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond_boots").expect("id"))
        .expect("boots");
    let mut inventory = vec![mc_data::ItemStack::EMPTY; 41];
    inventory[0] = mc_data::ItemStack::new(boots_id, 1);
    inventory[9] = mc_data::ItemStack::new(boots_id, 1);
    inventory[36] = mc_data::ItemStack::new(boots_id, 1);
    inventory[40] = mc_data::ItemStack::new(boots_id, 1);
    let record = ReplayPlayer {
        opaque_player_fields: Vec::new(),
        statistics: Default::default(),
        advancement_progress: crate::journal::empty_advancement_progress(),
        unlocked_recipes: Default::default(),
        dimension: Default::default(),
        pos: [5.0, 80.0, 5.0],
        yaw: 0.0,
        pitch: 0.0,
        health: 20.0,
        food: 20,
        saturation: 5.0,
        exhaustion: 0.0,
        air: 300,
        experience_level: 0,
        experience_progress: 0.0,
        experience_total: 0,
        game_mode: 0,
        selected: 0,
        inventory,
        carried: mc_data::ItemStack::EMPTY,
        grid: Vec::new(),
        respawn: None,
        respawn_dimension: Default::default(),
        respawn_forced: false,
        respawn_yaw: 0.0,
        respawn_pitch: 0.0,
        active_effects: Default::default(),
        enchantment_seed: 0x12345678,
        enchantment_offer_seed: 12345,
        warden_warning: Default::default(),
        current_impulse_impact_pos: None,
        fall_flying: false,
        burn_ticks: 0,
        portal: Default::default(),
        tick: 9,
    };
    let root = player_to_tag(&record, &items).expect("encode");
    let mc_nbt::Tag::Compound(entries) = &root else {
        panic!("root");
    };
    let player = player_from_fields(&items, VanillaPlayer::default(), entries).expect("codec");
    assert_eq!(
        entries.iter().find(|(name, _)| name == "DataVersion"),
        Some(&("DataVersion".to_owned(), mc_nbt::Tag::Int(4790))),
        "shared 26.1.2 DataVersion at the .dat root"
    );
    let mut slots: Vec<u8> = player.inventory.iter().map(|(slot, _)| *slot).collect();
    slots.sort_unstable();
    assert_eq!(slots, vec![0, 9], "Inventory carries storage 0-35 only");
    let mut names: Vec<&str> = player
        .equipment
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["feet", "offhand"],
        "armor/offhand ride equipment"
    );
    // And the round-trip restores every v2 index.
    let back = player_from_tag(&root, &items).expect("decode");
    assert_eq!(back.inventory[0], record.inventory[0]);
    assert_eq!(back.inventory[9], record.inventory[9]);
    assert_eq!(back.inventory[36], record.inventory[36]);
    assert_eq!(back.inventory[40], record.inventory[40]);
}

#[test]
fn effects_survive_checkpoint_and_journal_recovery() {
    use crate::effects::{ActiveEffect, ActiveEffects, EffectId};
    let items = mc_data::items::solaris_required_items();
    let uuid = "00000000-0000-0000-0000-00000000effe";
    let mut speed = ActiveEffect::new(EffectId::from_name("speed").expect("speed"), 1234, 255);
    speed.ambient = true;
    speed.show_particles = false;
    speed.show_icon = true;
    speed.hidden = Some(Box::new(ActiveEffect::new(speed.id, 2400, 1)));
    let mut absorption = ActiveEffect::new(
        EffectId::from_name("absorption").expect("absorption"),
        -1,
        3,
    );
    absorption.show_icon = false;
    let effects = ActiveEffects {
        entries: vec![speed, absorption],
        absorption: 3.25,
    };
    let entry = JournalEntry::Player {
        uuid: uuid.into(),
        opaque_player_fields: Vec::new(),
        statistics: Default::default(),
        advancement_progress: Some(crate::journal::empty_advancement_progress()),
        unlocked_recipes: Some(Default::default()),
        dimension: Default::default(),
        pos: [1.0, 80.0, -2.0],
        yaw: 12.0,
        pitch: -7.0,
        health: 17.0,
        food: 20,
        saturation: 5.0,
        exhaustion: 0.0,
        air: 300,
        experience_level: 0,
        experience_progress: 0.0,
        experience_total: 0,
        game_mode: 0,
        selected: 0,
        inventory: vec![mc_data::ItemStack::EMPTY; 41],
        carried: mc_data::ItemStack::EMPTY,
        grid: Vec::new(),
        respawn: None,
        respawn_dimension: Default::default(),
        respawn_forced: false,
        respawn_yaw: 0.0,
        respawn_pitch: 0.0,
        active_effects: effects.clone(),
        enchantment_seed: 0x34567890,
        enchantment_offer_seed: 12345,
        warden_warning: Default::default(),
        current_impulse_impact_pos: None,
        fall_flying: false,
        burn_ticks: 0,
        portal: Default::default(),
        tick: 1,
    };
    let recovered = replay_batches(Replay::default(), &[(1, 1, vec![entry.clone()])]);
    let record = &recovered.players[uuid];
    let root = player_to_tag(record, &items).expect("encode");
    let checkpoint = player_from_tag(&root, &items).expect("decode");
    assert_eq!(&checkpoint, record);
    let mc_nbt::Tag::Compound(fields) = root else {
        panic!("compound")
    };
    assert!(
        fields
            .iter()
            .any(|(key, value)| key == "AbsorptionAmount" && *value == mc_nbt::Tag::Float(3.25))
    );
    let Some((_, mc_nbt::Tag::List(list))) = fields.iter().find(|(key, _)| key == "active_effects")
    else {
        panic!("vanilla active_effects")
    };
    assert_eq!(list.elements.len(), 2);
    let mut tail = entry;
    let JournalEntry::Player {
        active_effects,
        tick,
        ..
    } = &mut tail
    else {
        unreachable!()
    };
    active_effects.entries[0].duration = 1200;
    active_effects.absorption = 1.5;
    *tick = 2;
    let expected = active_effects.clone();
    let mut base = Replay::default();
    base.players.insert(uuid.into(), checkpoint);
    let recovered = replay_batches(base, &[(2, 2, vec![tail.clone()])]);
    assert_eq!(recovered.players[uuid].active_effects, expected);
    // Old journal records retain the empty-effect default.
    let mut old = serde_json::to_value(tail).expect("encode old image");
    old["Player"]
        .as_object_mut()
        .expect("player object")
        .remove("active_effects");
    let old: JournalEntry = serde_json::from_value(old).expect("old player journal");
    let old = replay_batches(Replay::default(), &[(3, 3, vec![old])]);
    assert_eq!(old.players[uuid].active_effects, ActiveEffects::default());
}

#[test]
fn fall_flying_survives_nbt_and_journal_replay() {
    use mc_nbt::Tag;
    let items = mc_data::items::solaris_required_items();
    let elytra_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:elytra").expect("id"))
        .expect("elytra");
    let uuid = "00000000-0000-0000-0000-00000000e117";
    let mut worn = mc_data::ItemStack::new(elytra_id, 1);
    worn.damage = Some(42);
    let mut inventory = vec![mc_data::ItemStack::EMPTY; 41];
    inventory[38] = worn.clone();
    let mut record = player_from_tag(&Tag::Compound(Vec::new()), &items).expect("decode");
    record.inventory = inventory;
    record.fall_flying = true;
    let Tag::Compound(fields) = player_to_tag(&record, &items).expect("encode") else {
        panic!("compound")
    };
    assert_eq!(
        fields.iter().find(|(key, _)| key == "FallFlying"),
        Some(&("FallFlying".to_owned(), Tag::Byte(1))),
        "vanilla FallFlying byte"
    );
    let back = player_from_tag(&Tag::Compound(fields), &items).expect("decode");
    assert!(back.fall_flying, "true survives NBT");
    assert_eq!(back.inventory[38], worn, "elytra wear survives NBT");
    let mut landed = back.clone();
    landed.fall_flying = false;
    let Tag::Compound(landed_fields) = player_to_tag(&landed, &items).expect("encode") else {
        panic!("compound")
    };
    assert_eq!(
        landed_fields.iter().find(|(key, _)| key == "FallFlying"),
        Some(&("FallFlying".to_owned(), Tag::Byte(0))),
        "false survives NBT"
    );
    // A stale opaque image cannot resurrect FallFlying once cleared.
    let stale = player_from_tag(&Tag::Compound(landed_fields), &items).expect("decode");
    assert!(!stale.fall_flying);
    assert_eq!(stale.inventory[38], worn, "elytra wear survives landing");
    let entry = |record: &crate::journal::ReplayPlayer, tick: u64| JournalEntry::Player {
        uuid: uuid.into(),
        opaque_player_fields: record.opaque_player_fields.clone(),
        statistics: Box::new(record.statistics.clone()),
        advancement_progress: Some(crate::journal::empty_advancement_progress()),
        unlocked_recipes: Some(Default::default()),
        dimension: record.dimension,
        pos: record.pos,
        yaw: record.yaw,
        pitch: record.pitch,
        health: record.health,
        food: record.food,
        saturation: record.saturation,
        exhaustion: record.exhaustion,
        air: record.air,
        experience_level: record.experience_level,
        experience_progress: record.experience_progress,
        experience_total: record.experience_total,
        game_mode: record.game_mode,
        selected: record.selected,
        inventory: record.inventory.clone(),
        carried: record.carried.clone(),
        grid: record.grid.clone(),
        respawn: record.respawn,
        respawn_dimension: record.respawn_dimension,
        respawn_forced: record.respawn_forced,
        respawn_yaw: record.respawn_yaw,
        respawn_pitch: record.respawn_pitch,
        active_effects: record.active_effects.clone(),
        enchantment_seed: record.enchantment_seed,
        enchantment_offer_seed: record.enchantment_offer_seed,
        warden_warning: record.warden_warning,
        current_impulse_impact_pos: record.current_impulse_impact_pos,
        fall_flying: record.fall_flying,
        burn_ticks: record.burn_ticks,
        portal: record.portal,
        tick,
    };
    let flying = replay_batches(Replay::default(), &[(1, 1, vec![entry(&back, 1)])]);
    assert!(flying.players[uuid].fall_flying, "true survives WAL replay");
    assert_eq!(
        flying.players[uuid].inventory[38], worn,
        "elytra wear survives WAL replay"
    );
    let grounded = replay_batches(flying, &[(2, 2, vec![entry(&landed, 2)])]);
    assert!(
        !grounded.players[uuid].fall_flying,
        "false survives WAL replay"
    );
    assert_eq!(
        grounded.players[uuid].inventory[38], worn,
        "elytra wear survives landing replay"
    );
    // Old journal records without the key default to grounded.
    let mut old = serde_json::to_value(entry(&back, 3)).expect("encode old image");
    old["Player"]
        .as_object_mut()
        .expect("player object")
        .remove("fall_flying");
    let old: JournalEntry = serde_json::from_value(old).expect("old player journal");
    let old = replay_batches(Replay::default(), &[(3, 3, vec![old])]);
    assert!(
        !old.players[uuid].fall_flying,
        "old WAL defaults to grounded"
    );
    assert_eq!(
        old.players[uuid].inventory[38], worn,
        "elytra wear survives old WAL"
    );
}

#[test]
fn mob_effects_survive_entity_region_and_journal_recovery() {
    use crate::effects::{ActiveEffect, ActiveEffects, EffectId};
    let dir = std::env::temp_dir().join(format!(
        "mc-core-mob-effects-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let items = mc_data::items::solaris_required_items();
    let effects = ActiveEffects {
        entries: vec![ActiveEffect::new(
            EffectId::from_name("regeneration").expect("effect"),
            -1,
            2,
        )],
        absorption: 2.75,
    };
    let entity = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:cow".into(),
        wire_id: 7,
        active_effects: crate::journal::active_effects_to_nbt(&effects),
        absorption_amount: effects.absorption,
        ..Default::default()
    };
    let chunks = std::collections::BTreeMap::from([(
        (0, 0),
        mc_world::anvil::EntityChunk {
            x: 0,
            z: 0,
            entities: vec![entity.clone()],
        },
    )]);
    mc_world::anvil::write_entity_regions(&dir.join("entities"), &items, &chunks, 1)
        .expect("save entity checkpoint");
    let chunks = mc_world::anvil::read_entity_regions(&dir.join("entities"), &items)
        .expect("recover entity checkpoint");
    let saved = &chunks[&(0, 0)].entities[0];
    let recovered_effects =
        crate::journal::active_effects_from_nbt(&saved.active_effects, saved.absorption_amount);
    assert_eq!(recovered_effects, effects);
    let mut base = Replay::default();
    base.entities.insert(
        7,
        crate::journal::ReplayEntity {
            uuid: saved.uuid.to_string(),
            type_id: 100,
            pos: saved.pos,
            vel: [0.0; 3],
            yaw: saved.yaw,
            pitch: saved.pitch,
            ground: saved.on_ground,
            health: saved.health,
            alive: true,
            age: 0,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            appearance: Default::default(),
            active_effects: recovered_effects,
            villager: None,
            projectile: None,
            state: Default::default(),
            tick: 1,
        },
    );
    let mut tail_effects = effects;
    tail_effects.absorption = 0.5;
    tail_effects.entries[0].show_particles = false;
    let tail = JournalEntry::Entity {
        id: 7,
        uuid: entity.uuid.to_string(),
        type_id: 100,
        pos: [10.0, 80.0, 10.0],
        yaw: 0.0,
        pitch: 0.0,
        health: 19.0,
        alive: true,
        age: 0,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: Default::default(),
        active_effects: tail_effects.clone(),
        villager: None,
        projectile: None,
        vel: [0.0; 3],
        state: Default::default(),
        tick: 2,
    };
    {
        let mut writer = JournalWriter::open(&dir, 1).expect("writer");
        writer
            .append(2, 2, std::slice::from_ref(&tail))
            .expect("append");
        writer.flush().expect("flush");
    }
    let replay = replay_batches(
        base,
        &read_complete_batches_after(&dir.join("journal"), 1).unwrap(),
    );
    assert_eq!(replay.entities[&7].active_effects, tail_effects);
    assert_eq!(replay.entities[&7].pos, saved.pos);
    let mut old = serde_json::to_value(tail).expect("encode");
    old["Entity"]
        .as_object_mut()
        .expect("entity object")
        .remove("active_effects");
    let old: JournalEntry = serde_json::from_value(old).expect("legacy entity");
    let replay = replay_batches(Replay::default(), &[(2, 2, vec![old])]);
    assert_eq!(replay.entities[&7].active_effects, ActiveEffects::default());
    std::fs::remove_dir_all(dir).expect("remove test world");
}

fn villager_recovery_fixture(
    items: &mc_data::items::ItemRegistry,
) -> crate::villager::VillagerState {
    use crate::villager::{Gossip, VillagerTradeOffer};
    let emerald = items
        .id_of(&mc_data::Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    let sword = items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond_sword").unwrap())
        .unwrap();
    let mut result = mc_data::ItemStack::new(sword, 1);
    result.damage = Some(19);
    result.custom_name = Some("Recovered blade".into());
    result.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:sharpness").unwrap(),
        level: 3,
    });
    let mut state = crate::villager::VillagerState {
        age: -100,
        xp: 90,
        food_level: 12,
        last_restock: 80,
        last_restock_day: 3,
        restocks_today: 1,
        last_work: 90,
        last_slept: Some(40),
        golem_detected_until: Some(500),
        job_site: Some(crate::boundary::BlockPos { x: 9, y: 64, z: -4 }),
        home: Some(crate::boundary::BlockPos { x: 4, y: 64, z: -9 }),
        meeting_point: Some(crate::boundary::BlockPos { x: -2, y: 66, z: 7 }),
        offers: vec![VillagerTradeOffer {
            cost_a: mc_data::ItemStack::new(emerald, 15),
            cost_b: None,
            result: result.clone(),
            uses: 2,
            max_uses: 12,
            reward_exp: true,
            xp: 10,
            special_price: -2,
            demand: 4,
            price_multiplier: 0.2,
        }],
        gossips: vec![Gossip {
            target: uuid::Uuid::from_u128(91),
            kind: "minor_positive".into(),
            value: 20,
        }],
        ..Default::default()
    };
    state.inventory[7] = Some(result);
    state
}

fn villager_entry(state: crate::villager::VillagerState, tick: u64) -> JournalEntry {
    JournalEntry::Entity {
        id: 7,
        uuid: uuid::Uuid::from_u128(7).to_string(),
        type_id: 119,
        health: 20.0,
        alive: true,
        age: 0,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: Default::default(),
        active_effects: Default::default(),
        villager: Some(Box::new(state)),
        state: Default::default(),
        projectile: None,
        tick,
        // Intra-chunk motion stays checkpoint-only; crossings have their own gate.
        pos: [5.0, 90.0, -10.0],
        vel: [0.0; 3],
        yaw: 1.0,
        pitch: 0.0,
    }
}

#[test]
fn villager_checkpoint_and_wal_recover_absolute_conversion_custody_and_memories() {
    use crate::villager::persistence::{villager_from_fields, villager_to_fields};
    let items = mc_data::items::solaris_required_items();
    let dir = std::env::temp_dir().join(format!("mc-core-villager-{}", uuid::Uuid::new_v4()));
    let initial = villager_recovery_fixture(&items);
    let native = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:villager".into(),
        uuid: uuid::Uuid::from_u128(7),
        wire_id: 7,
        pos: [4.0, 64.0, -9.0],
        motion: [0.5, 0.0, 0.0],
        extra_fields: villager_to_fields(&items, &initial, 100).unwrap(),
        ..Default::default()
    };
    let chunks = std::collections::BTreeMap::from([(
        (0, -1),
        mc_world::anvil::EntityChunk {
            x: 0,
            z: -1,
            entities: vec![native],
        },
    )]);
    mc_world::anvil::write_entity_regions(&dir.join("entities"), &items, &chunks, 100).unwrap();
    let loaded = mc_world::anvil::read_entity_regions(&dir.join("entities"), &items).unwrap();
    let native = &loaded[&(0, -1)].entities[0];
    let checkpoint = villager_from_fields(&items, &native.extra_fields, 100).unwrap();
    assert_eq!(checkpoint, initial);
    let mut base = Replay {
        tick: 100,
        ..Replay::default()
    };
    base.sim_debt.insert(
        [0, -1],
        SimDebtImage {
            chunk: [0, -1],
            last_sim: 100,
            remaining: 0,
            tick: 100,
            active: true,
        },
    );
    base.entities.insert(
        7,
        crate::journal::ReplayEntity {
            uuid: native.uuid.to_string(),
            type_id: 119,
            pos: native.pos,
            vel: [10.0, 0.0, 0.0],
            yaw: native.yaw,
            pitch: native.pitch,
            ground: true,
            health: native.health,
            alive: true,
            age: 0,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            appearance: Default::default(),
            active_effects: Default::default(),
            villager: Some(checkpoint),
            state: Default::default(),
            projectile: None,
            tick: 100,
        },
    );
    let mut latest = initial;
    latest.age = -99;
    latest.xp = 115;
    latest.offers[0].uses = 0;
    latest.offers[0].demand = 6;
    latest.inventory[7].as_mut().unwrap().damage = Some(35);
    latest.gossips[0].value = 17;
    latest.last_restock = 101;
    latest.restocks_today = 2;
    latest.last_slept = Some(101);
    latest.conversion_time = Some(321);
    latest.conversion_player = Some(uuid::Uuid::from_u128(55));
    {
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer
            .append(101, 101, &[villager_entry(latest.clone(), 101)])
            .unwrap();
        writer
            .append(
                102,
                102,
                &[JournalEntry::EntityVitals {
                    id: 7,
                    health: 18.0,
                    alive: true,
                    tick: 102,
                }],
            )
            .unwrap();
        writer.flush().unwrap();
    }
    let replay = replay_batches(
        base,
        &read_complete_batches_after(&dir.join("journal"), 100).unwrap(),
    );
    latest.conversion_time = Some(320);
    let recovered = &replay.entities[&7];
    assert_eq!(recovered.villager.as_ref(), Some(&latest));
    assert_eq!(recovered.health, 18.0);
    assert_eq!(
        recovered.pos, native.pos,
        "villager WAL must not advance checkpoint motion"
    );
    assert_eq!(recovered.vel, [10.0, 0.0, 0.0]);
    assert_eq!(replay.tick, 102);
    let fields = villager_to_fields(&items, recovered.villager.as_ref().unwrap(), 102).unwrap();
    assert_eq!(villager_from_fields(&items, &fields, 102).unwrap(), latest);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn legacy_journal_and_manifest_decode_without_villager_or_generated_claim_fields() {
    let items = mc_data::items::solaris_required_items();
    let villager = villager_recovery_fixture(&items);
    let expected_villager = serde_json::to_value(&villager).unwrap();
    let mut json = serde_json::to_value(villager_entry(villager, 1)).unwrap();
    assert_eq!(json["Entity"]["villager"], expected_villager);
    assert_eq!(
        json["Entity"]["state"],
        serde_json::to_value(crate::journal::EntityStateImage::default()).unwrap()
    );
    json["Entity"].as_object_mut().unwrap().remove("villager");
    let entry: JournalEntry = serde_json::from_value(json).unwrap();
    assert!(serde_json::to_value(&entry).unwrap()["Entity"]["villager"].is_null());
    let replay = replay_batches(Replay::default(), &[(1, 1, vec![entry])]);
    assert!(replay.entities[&7].villager.is_none());
    assert!(serde_json::from_str::<crate::journal::CheckpointManifest>(r#"{"tick":9}"#).is_err());
}

#[test]
fn malformed_villager_image_cannot_replace_a_valid_journal_batch() {
    let items = mc_data::items::solaris_required_items();
    let dir =
        std::env::temp_dir().join(format!("mc-core-villager-invalid-{}", uuid::Uuid::new_v4()));
    let initial = villager_recovery_fixture(&items);
    let valid = villager_entry(initial.clone(), 1);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, 1, std::slice::from_ref(&valid)).unwrap();
    let before =
        std::fs::read(dir.join("journal/batch-00000000000000000001-00000000000000000001.bin"))
            .unwrap();
    let mut invalid = initial;
    invalid.data.level = 0;
    assert!(writer.append(2, 2, &[villager_entry(invalid, 2)]).is_err());
    let after =
        std::fs::read(dir.join("journal/batch-00000000000000000001-00000000000000000001.bin"))
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        read_complete_batches_after(&dir.join("journal"), 0)
            .unwrap()
            .len(),
        1
    );
    let mut json = serde_json::to_value(valid).unwrap();
    json["Entity"]["villager"]["data"]["level"] = serde_json::json!(0);
    assert!(serde_json::from_value::<JournalEntry>(json).is_err());
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn generated_villager_claim_survives_entity_tombstone_and_manifest_recovery() {
    let items = mc_data::items::solaris_required_items();
    let claim = "village/plains/0,0/resident/4,64,9".to_owned();
    let replay = replay_batches(
        Replay::default(),
        &[(
            1,
            2,
            vec![
                JournalEntry::GeneratedEntityClaim {
                    claim: claim.clone(),
                },
                villager_entry(villager_recovery_fixture(&items), 1),
                JournalEntry::RemoveEntity { id: 7, tick: 2 },
            ],
        )],
    );
    assert!(replay.entities.is_empty());
    assert!(replay.generated_entity_claims.contains(&claim));
    let manifest: crate::journal::CheckpointManifest = serde_json::from_value(serde_json::json!({
        "tick": 2, "domain_versions": {}, "generated_entity_claims": replay.generated_entity_claims,
    }))
    .unwrap();
    let restored: crate::journal::CheckpointManifest =
        serde_json::from_slice(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(restored.generated_entity_claims.contains(&claim));
}

fn decode_failure_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    for tick in 1..=3 {
        writer
            .append(tick, tick, &[JournalEntry::RemoveEntity { id: 77, tick }])
            .unwrap();
    }
    writer.flush().unwrap();
    dir
}

fn batch_path(dir: &std::path::Path, tick: u64) -> std::path::PathBuf {
    dir.join("journal")
        .join(format!("batch-{tick:020}-{tick:020}.bin"))
}

fn journal_recovery_files(
    dir: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(journal_recovery_files(&path));
        } else {
            files.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    files
}

#[test]
fn journal_decode_corrupt_final_batch_fails_open_without_writes() {
    for crc_damage in [false, true] {
        let dir = decode_failure_fixture();
        let path = batch_path(dir.path(), 3);
        let mut bytes = std::fs::read(&path).unwrap();
        if crc_damage {
            bytes[32] ^= 1;
        } else {
            bytes.pop();
        }
        std::fs::write(&path, bytes).unwrap();
        let before = journal_recovery_files(dir.path());
        let error = read_complete_batches_after(&dir.path().join("journal"), 0)
            .unwrap_err()
            .to_string();
        assert!(error.contains(path.to_str().unwrap()), "{error}");
        assert!(error.contains("record 0 offset 0"), "{error}");
        assert_eq!(journal_recovery_files(dir.path()), before);
    }
}

#[test]
fn journal_missing_middle_batch_refuses_runtime_recovery_without_rewrites() {
    let dir = decode_failure_fixture();
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    drop(store);
    std::fs::remove_file(batch_path(dir.path(), 2)).unwrap();
    let before = journal_recovery_files(&dir.path().join("journal"));
    let error = crate::Core::open(config.clone()).unwrap_err().to_string();
    assert!(error.contains("journal hole: ticks 2..=2"), "{error}");
    assert!(
        error.contains(batch_path(dir.path(), 3).to_str().unwrap()),
        "{error}"
    );
    assert_eq!(journal_recovery_files(&dir.path().join("journal")), before);
    let recovered = crate::native_checkpoint::tests::fixture_store(&config)
        .unwrap()
        .read_checkpoint()
        .unwrap();
    assert_eq!(recovered.tick, checkpoint.tick);
    assert_eq!(recovered.globals.len(), checkpoint.globals.len());
    for (key, expected) in &checkpoint.globals {
        let actual = &recovered.globals[key];
        assert_eq!(actual.version, expected.version, "{key}");
        assert_eq!(actual.captured_tick, expected.captured_tick, "{key}");
        assert_eq!(actual.payload, expected.payload, "{key}");
    }
    assert!(checkpoint.players.is_empty() && recovered.players.is_empty());
    assert!(checkpoint.entities.is_empty() && recovered.entities.is_empty());
}

#[test]
fn journal_decode_unpublished_tmp_is_ignored_and_open_succeeds() {
    let dir = decode_failure_fixture();
    let path = dir
        .path()
        .join("journal/batch-00000000000000000999-00000000000000000999.bin.tmp");
    std::fs::write(&path, b"unpublished partial bytes").unwrap();
    let batches = read_complete_batches_after(&dir.path().join("journal"), 0).unwrap();
    assert_eq!(batches.len(), 3);
    assert_eq!(replay_batches(Replay::default(), &batches).tick, 3);
    assert_eq!(std::fs::read(path).unwrap(), b"unpublished partial bytes");
}

#[test]
fn streaming_reader_visits_in_order_and_stops_before_reading_later_damage() {
    let dir = decode_failure_fixture();
    let journal = dir.path().join("journal");
    let mut ticks = Vec::new();
    let last = crate::journal::visit_complete_batches_after(&journal, 0, |(lo, hi, _)| {
        ticks.push((lo, hi));
        Ok(())
    })
    .unwrap();
    assert_eq!(last, 3);
    assert_eq!(ticks, [(1, 1), (2, 2), (3, 3)]);
    let later = journal.join("batch-00000000000000000003-00000000000000000003.bin");
    std::fs::write(&later, b"damaged later batch").unwrap();
    let mut visited = 0;
    let error = crate::journal::visit_complete_batches_after(&journal, 0, |_| {
        visited += 1;
        Err(crate::CoreError::Storage("consumer stopped".into()))
    })
    .unwrap_err();
    assert_eq!(visited, 1);
    assert!(error.to_string().contains("consumer stopped"));
    let error = crate::journal::visit_complete_batches_after(&journal, 0, |_| Ok(())).unwrap_err();
    assert!(error.to_string().contains(later.to_str().unwrap()));
}

#[test]
fn journal_decode_crc_valid_invalid_records_fail_open_without_writes() {
    for invalid in [r#"{"Entity":}"#, r#"{"Entity":{"id":"not-an-integer"}}"#] {
        // Test both a middle batch and the final batch: valid framing never
        // makes malformed JSON or a semantically invalid image a torn tail.
        for tick in [2, 3] {
            let dir = decode_failure_fixture();
            let path = batch_path(dir.path(), tick);
            let mut bytes = std::fs::read(&path).unwrap();
            let prefix = format!("{{\"RemoveEntity\":{{\"id\":77,\"tick\":{tick}}}}}\n");
            let payload = format!("{prefix}{invalid}\n");
            bytes.truncate(32);
            bytes[24..28].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes[28..32].copy_from_slice(&crc32fast::hash(payload.as_bytes()).to_le_bytes());
            bytes.extend_from_slice(payload.as_bytes());
            std::fs::write(&path, bytes).unwrap();
            let before = journal_recovery_files(dir.path());
            let error = read_complete_batches_after(&dir.path().join("journal"), 0)
                .unwrap_err()
                .to_string();
            assert!(error.contains(path.to_str().unwrap()), "{error}");
            assert!(
                error.contains(&format!("record 1 offset {}", 32 + prefix.len())),
                "{error}"
            );
            assert!(error.contains("line 1 column"), "{error}");
            if invalid.contains("not-an-integer") {
                assert!(error.contains("kind Entity"), "{error}");
                assert!(error.contains("invalid type"), "{error}");
            }
            assert_eq!(journal_recovery_files(dir.path()), before);
        }
    }
}

#[test]
fn journal_decode_mid_file_crc_damage_fails_open_without_writes() {
    let dir = decode_failure_fixture();
    let path = batch_path(dir.path(), 2);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[32] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    let before = journal_recovery_files(dir.path());
    let error = read_complete_batches_after(&dir.path().join("journal"), 0)
        .unwrap_err()
        .to_string();
    assert!(error.contains(path.to_str().unwrap()), "{error}");
    assert!(error.contains("record 0 offset 0: CRC mismatch"), "{error}");
    assert_eq!(journal_recovery_files(dir.path()), before);
}

#[test]
fn journal_decode_corrupt_player_custody_item_fails_open_without_writes() {
    use mc_nbt::{ListTag, Tag, tag_type};
    let invalid = Tag::Compound(vec![
        (
            "id".into(),
            Tag::String("minecraft:missing_custody_item".into()),
        ),
        ("count".into(), Tag::Int(1)),
    ]);
    for (field, value, slot) in [
        ("solaris:Carried", invalid.clone(), 0),
        (
            "solaris:CraftingGrid",
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![Tag::Compound(Vec::new()), invalid],
            }),
            1,
        ),
    ] {
        let items = mc_data::items::solaris_required_items();
        let error = player_from_tag(&Tag::Compound(vec![(field.into(), value)]), &items)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("{field} slot {slot}")), "{error}");
        assert!(
            error.contains("item NBT: unknown item minecraft:missing_custody_item"),
            "{error}"
        );
    }
}

#[test]
fn journal_decode_valid_empty_player_custody_preserves_grid_slots() {
    use mc_nbt::{ListTag, Tag, tag_type};
    let root = Tag::Compound(vec![
        ("solaris:Carried".into(), Tag::Compound(Vec::new())),
        (
            "solaris:CraftingGrid".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![Tag::Compound(Vec::new()), Tag::Compound(Vec::new())],
            }),
        ),
    ]);
    let items = mc_data::items::solaris_required_items();
    let record = player_from_tag(&root, &items).unwrap();
    assert!(record.carried.is_empty());
    assert_eq!(record.grid.len(), 2);
    assert!(record.grid.iter().all(mc_data::ItemStack::is_empty));
}

#[test]
fn checkpoint_sparse_crafting_grid_preserves_all_cells_and_item_custody() {
    let items = mc_data::items::solaris_required_items();
    let mut record = player_from_tag(&mc_nbt::Tag::Compound(Vec::new()), &items).unwrap();
    let stone = items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    record.grid = vec![
        mc_data::ItemStack::EMPTY,
        mc_data::ItemStack::new(stone, 3),
        mc_data::ItemStack::EMPTY,
        mc_data::ItemStack::new(stone, 1),
    ];
    record.dimension = crate::dimensions::Dimension::Nether;
    record.respawn_dimension = crate::dimensions::Dimension::End;
    record.respawn_forced = true;
    record.respawn_yaw = 45.0;
    record.respawn_pitch = -12.0;
    record.burn_ticks = u32::MAX;
    record.tick = u64::MAX;
    record.carried = mc_data::ItemStack::new(stone, 2);
    let exact = player_from_tag(&player_to_tag(&record, &items).unwrap(), &items).unwrap();
    assert_eq!(exact, record);
    let recovered = player_from_tag(&player_to_tag(&record, &items).unwrap(), &items).unwrap();
    assert_eq!(recovered.grid, record.grid);
    assert_eq!(recovered.carried, record.carried);
    assert_eq!(recovered.inventory, record.inventory);
    record.grid.fill(mc_data::ItemStack::EMPTY);
    let empty = player_from_tag(&player_to_tag(&record, &items).unwrap(), &items).unwrap();
    assert_eq!(empty.grid, record.grid);
}

#[test]
fn player_codec_rejects_noncompound_and_invalid_native_timers() {
    let items = mc_data::items::solaris_required_items();
    assert!(player_from_tag(&mc_nbt::Tag::Int(1), &items).is_err());
    for value in [
        mc_nbt::Tag::Long(-1),
        mc_nbt::Tag::Long(i64::from(u32::MAX) + 1),
        mc_nbt::Tag::Int(1),
    ] {
        let root = mc_nbt::Tag::Compound(vec![("solaris:BurnTicks".into(), value)]);
        assert!(player_from_tag(&root, &items).is_err());
    }
}

#[test]
fn entity_state_image_retains_opaque_nbt_through_journal_serialization() {
    let fields = vec![(
        "converter:opaque".to_owned(),
        mc_nbt::Tag::Compound(vec![("value".into(), mc_nbt::Tag::Long(123))]),
    )];
    let mut opaque_fields = Vec::new();
    mc_nbt::write_named(
        &mut opaque_fields,
        "",
        &mc_nbt::Tag::Compound(fields.clone()),
    )
    .unwrap();
    let image = crate::journal::EntityStateImage {
        opaque_entity_fields: opaque_fields,
        ..Default::default()
    };
    let restored: crate::journal::EntityStateImage =
        serde_json::from_slice(&serde_json::to_vec(&image).unwrap()).unwrap();
    assert_eq!(restored, image);
    let (_, root) =
        mc_nbt::read_named(&mut std::io::Cursor::new(&restored.opaque_entity_fields)).unwrap();
    assert_eq!(root, mc_nbt::Tag::Compound(fields));
}

#[test]
fn player_opaque_fields_survive_typed_edits_without_resurrecting_removed_keys() {
    use mc_nbt::Tag;
    let items = mc_data::items::solaris_required_items();
    let stone = items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let mut player = player_from_tag(&Tag::Compound(Vec::new()), &items).unwrap();
    player.health = 17.0;
    player.carried = mc_data::ItemStack::new(stone, 3);
    player.inventory[36] = mc_data::ItemStack::new(stone, 1);
    player.respawn = Some([1, 64, 2]);
    player.current_impulse_impact_pos = Some([1.0, 2.0, 3.0]);
    player.fall_flying = true;
    let Tag::Compound(mut fields) = player_to_tag(&player, &items).unwrap() else {
        panic!("compound")
    };
    let unknown = (
        "converter:opaque".to_owned(),
        Tag::Compound(vec![(
            "list".into(),
            Tag::LongArray(vec![i64::MIN, 123, i64::MAX]),
        )]),
    );
    fields.push(unknown.clone());
    let mut retained = player_from_tag(&Tag::Compound(fields.clone()), &items).unwrap();
    assert!(!retained.opaque_player_fields.is_empty());
    retained.health = 9.0;
    retained.carried = mc_data::ItemStack::EMPTY;
    retained.inventory[36] = mc_data::ItemStack::EMPTY;
    retained.respawn = None;
    retained.current_impulse_impact_pos = None;
    retained.fall_flying = false;
    // Even an opaque image supplied by a caller cannot override or resurrect
    // fields whose typed values have changed or been removed.
    retained.opaque_player_fields.clear();
    mc_nbt::write_named(
        &mut retained.opaque_player_fields,
        "",
        &Tag::Compound(fields),
    )
    .unwrap();
    let root = player_to_tag(&retained, &items).unwrap();
    let Tag::Compound(output) = &root else {
        panic!("compound")
    };
    assert!(output.contains(&unknown));
    assert_eq!(
        output.iter().find(|(key, _)| key == "Health").unwrap().1,
        Tag::Float(9.0)
    );
    for key in [
        "solaris:Carried",
        "equipment",
        "respawn",
        "current_explosion_impact_pos",
    ] {
        assert!(!output.iter().any(|(name, _)| name == key), "{key}");
    }
    assert_eq!(
        output
            .iter()
            .filter(|(key, _)| key == "FallFlying")
            .collect::<Vec<_>>(),
        vec![&("FallFlying".to_owned(), Tag::Byte(0))],
        "the authoritative false value replaces the stale opaque true flag"
    );
    let decoded = player_from_tag(&root, &items).unwrap();
    assert_eq!(decoded.health, 9.0);
    assert!(decoded.carried.is_empty());
    assert!(decoded.inventory[36].is_empty());
    assert_eq!(decoded.respawn, None);
    assert_eq!(decoded.current_impulse_impact_pos, None);
    assert!(!decoded.fall_flying);
    let (_, unknown_root) =
        mc_nbt::read_named(&mut std::io::Cursor::new(&decoded.opaque_player_fields)).unwrap();
    assert_eq!(unknown_root, Tag::Compound(vec![unknown]));
}

#[test]
fn player_opaque_fields_and_advancements_follow_join_checkpoint_and_wal_replay() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use std::sync::Arc;
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(
        mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks)),
    );
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = crate::tick::TickState::new(Arc::clone(&registry), world, 0, 512);
    let uuid = uuid::Uuid::new_v4();
    let unknown = (
        "converter:opaque".to_owned(),
        mc_nbt::Tag::LongArray(vec![1, 2, 3]),
    );
    let mut record = player_from_tag(
        &mc_nbt::Tag::Compound(vec![unknown.clone()]),
        &registry.items,
    )
    .unwrap();
    let progress = serde_json::json!({
        "DataVersion": 4790,
        "minecraft:story/root": {
            "criteria": {"tick": "2026-10-14 11:12:13 +0000"},
            "done": true,
            "unknown": [null, {"mod:note": "retained"}]
        },
        "mod:future": {"nested": [1, 2, 3], "large": "x".repeat(70_000)}
    });
    record.advancement_progress = Arc::new(progress.clone());
    Arc::make_mut(&mut record.unlocked_recipes).insert("minecraft:oak_planks".into());
    let root = player_to_tag(&record, &registry.items).unwrap();
    let mut encoded = Vec::new();
    mc_nbt::write_named(&mut encoded, "", &root).unwrap();
    let checkpoint_dir = tempfile::tempdir().unwrap();
    let checkpoint_path = checkpoint_dir.path().join("player.dat");
    std::fs::write(&checkpoint_path, encoded).unwrap();
    let checkpoint_bytes = std::fs::read(&checkpoint_path).unwrap();
    let (_, root) = mc_nbt::read_named(&mut checkpoint_bytes.as_slice()).unwrap();
    let record = player_from_tag(&root, &registry.items).unwrap();
    assert_eq!(*record.advancement_progress, progress);
    assert!(record.unlocked_recipes.contains("minecraft:oak_planks"));
    let mut checkpoint = Replay::default();
    checkpoint.players.insert(uuid.to_string(), record.clone());
    let copy_player_replay = |image: &Replay| Replay {
        players: image.players.clone(),
        tick: image.tick,
        ..Default::default()
    };
    let expected = record.opaque_player_fields.clone();
    state.offline_players.insert(uuid, record);
    let (mut output, _, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(7),
            uuid,
            name: "opaque-player".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(7)).unwrap();
    assert_eq!(state.players[&entity].opaque_player_fields, expected);
    assert_eq!(*state.players[&entity].advancement_progress, progress);
    assert!(
        state.players[&entity]
            .unlocked_recipes
            .contains("minecraft:oak_planks")
    );
    let initial = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(initial.iter().any(|entry| matches!(
        entry,
        JournalEntry::Player {
            advancement_progress: Some(_),
            unlocked_recipes: Some(_),
            ..
        }
    )));
    let full = initial
        .iter()
        .find(|entry| matches!(entry, JournalEntry::Player { .. }))
        .unwrap();
    let full_json = serde_json::to_value(full).unwrap();
    assert_eq!(full_json["Player"]["advancement_progress"], progress);
    assert_eq!(
        full_json["Player"]["unlocked_recipes"],
        serde_json::json!(["minecraft:oak_planks"])
    );
    let decoded: JournalEntry = serde_json::from_value(full_json).unwrap();
    assert!(matches!(
        decoded,
        JournalEntry::Player {
            advancement_progress: Some(_),
            unlocked_recipes: Some(_),
            ..
        }
    ));
    let mut null_json = serde_json::to_value(full).unwrap();
    null_json["Player"]["advancement_progress"] = serde_json::Value::Null;
    let null_entry: JournalEntry = serde_json::from_value(null_json).unwrap();
    assert!(matches!(&null_entry, JournalEntry::Player {
        advancement_progress: Some(value), ..
    } if value.is_null()));
    checkpoint = replay_batches(checkpoint, &[(output.tick, output.tick, initial)]);
    let snapshot = crate::core::player_record_for(&state, entity, output.tick).unwrap();
    assert!(Arc::ptr_eq(
        &snapshot.advancement_progress,
        &state.players[&entity].advancement_progress
    ));
    assert!(Arc::ptr_eq(
        &snapshot.unlocked_recipes,
        &state.players[&entity].unlocked_recipes
    ));
    let mut movement_batches = Vec::new();
    for offset in 1..=3 {
        output.tick += 1;
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.set_pos(slot, [offset as f64, 80.0, -2.0]);
        let captured = crate::core::player_record_for(&state, entity, output.tick).unwrap();
        assert!(Arc::ptr_eq(
            &snapshot.advancement_progress,
            &captured.advancement_progress
        ));
        assert!(Arc::ptr_eq(
            &snapshot.unlocked_recipes,
            &captured.unlocked_recipes
        ));
        let movement = crate::core::journal_entries_for_tick(&mut state, &output);
        let entry = movement
            .iter()
            .find(|entry| matches!(entry, JournalEntry::Player { .. }))
            .unwrap();
        assert!(matches!(
            entry,
            JournalEntry::Player {
                advancement_progress: None,
                unlocked_recipes: None,
                ..
            }
        ));
        let bytes = serde_json::to_vec(entry).unwrap();
        assert!(bytes.len() < 32 * 1024, "movement WAL must stay bounded");
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json["Player"].get("advancement_progress").is_none());
        assert!(json["Player"].get("unlocked_recipes").is_none());
        movement_batches.push((output.tick, output.tick, movement));
    }
    let movement_dir = tempfile::tempdir().unwrap();
    let mut movement_writer = JournalWriter::open(movement_dir.path(), 1).unwrap();
    for (lo, hi, entries) in &movement_batches {
        movement_writer.append(*lo, *hi, entries).unwrap();
    }
    movement_writer.flush().unwrap();
    drop(movement_writer);
    let movement_batches =
        read_complete_batches_after(&movement_dir.path().join("journal"), checkpoint.tick).unwrap();
    let moved = replay_batches(copy_player_replay(&checkpoint), &movement_batches);
    assert_eq!(
        *moved.players[&uuid.to_string()].advancement_progress,
        progress
    );
    assert_eq!(
        moved.players[&uuid.to_string()].unlocked_recipes,
        snapshot.unlocked_recipes
    );
    assert_eq!(moved.players[&uuid.to_string()].pos, [3.0, 80.0, -2.0]);
    let repeated = replay_batches(copy_player_replay(&moved), &movement_batches);
    assert_eq!(repeated.players, moved.players);
    assert!(Arc::ptr_eq(
        &repeated.players[&uuid.to_string()].advancement_progress,
        &moved.players[&uuid.to_string()].advancement_progress
    ));
    let player = state.players.get_mut(&entity).unwrap();
    Arc::make_mut(&mut player.advancement_progress)["minecraft:story/root"]["done"] = false.into();
    output.tick += 1;
    let advancement_only = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(advancement_only.iter().any(|entry| matches!(
        entry,
        JournalEntry::Player {
            advancement_progress: Some(_),
            unlocked_recipes: None,
            ..
        }
    )));
    let advanced = replay_batches(
        moved,
        &[(output.tick, output.tick, advancement_only.clone())],
    );
    assert!(
        !advanced.players[&uuid.to_string()].advancement_progress["minecraft:story/root"]["done"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(
        *snapshot.advancement_progress, progress,
        "COW leaves old snapshots intact"
    );
    let advanced_again = replay_batches(
        copy_player_replay(&advanced),
        &[(output.tick, output.tick, advancement_only)],
    );
    assert_eq!(advanced_again.players, advanced.players);
    Arc::make_mut(&mut state.players.get_mut(&entity).unwrap().unlocked_recipes)
        .insert("minecraft:stick".into());
    output.tick += 1;
    let recipe_only = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(recipe_only.iter().any(|entry| matches!(
        entry,
        JournalEntry::Player {
            advancement_progress: None,
            unlocked_recipes: Some(_),
            ..
        }
    )));
    let recipes = replay_batches(advanced, &[(output.tick, output.tick, recipe_only.clone())]);
    assert!(
        recipes.players[&uuid.to_string()]
            .unlocked_recipes
            .contains("minecraft:stick")
    );
    assert!(!snapshot.unlocked_recipes.contains("minecraft:stick"));
    let recipes_again = replay_batches(
        copy_player_replay(&recipes),
        &[(output.tick, output.tick, recipe_only)],
    );
    assert_eq!(recipes_again.players, recipes.players);
    checkpoint = recipes;
    output.tick += 1;
    let updated = state.players[&entity].advancement_progress.clone();
    state.players.get_mut(&entity).unwrap().vitals.health = 9.0;
    let captured = crate::core::player_record_for(&state, entity, output.tick).unwrap();
    assert_eq!(captured.opaque_player_fields, expected);
    assert_eq!(captured.advancement_progress, updated);
    assert_eq!(captured.unlocked_recipes.len(), 2);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let dir = tempfile::tempdir().unwrap();
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches =
        read_complete_batches_after(&dir.path().join("journal"), checkpoint.tick).unwrap();
    let recovered = replay_batches(checkpoint, &batches);
    let player = &recovered.players[&uuid.to_string()];
    assert_eq!(player.opaque_player_fields, expected);
    assert_eq!(player.health, 9.0);
    assert_eq!(player.advancement_progress, updated);
    assert_eq!(player.unlocked_recipes, captured.unlocked_recipes);
    let entry = entries
        .iter()
        .find(|entry| matches!(entry, JournalEntry::Player { .. }))
        .unwrap();
    let mut old = serde_json::to_value(entry).unwrap();
    let fields = old["Player"].as_object_mut().unwrap();
    fields.remove("advancement_progress");
    fields.remove("unlocked_recipes");
    let old: JournalEntry = serde_json::from_value(old).unwrap();
    let old = replay_batches(Replay::default(), &[(output.tick, output.tick, vec![old])]);
    assert_eq!(
        *old.players[&uuid.to_string()].advancement_progress,
        serde_json::json!({})
    );
    assert!(old.players[&uuid.to_string()].unlocked_recipes.is_empty());
    let mc_nbt::Tag::Compound(fields) = player_to_tag(player, &registry.items).unwrap() else {
        panic!("compound")
    };
    assert!(fields.contains(&unknown));
}
