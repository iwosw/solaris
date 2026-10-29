use crate::boundary::{ConnectionId, EntityDataValue, EntityId, PlayerInput};
use crate::entities::EntityKind;
use crate::entity_loot::*;
use crate::intent::DamageSource;
use crate::registry::CoreRegistry;
use crate::survival::{DamageKind, Difficulty};
use crate::tick::{DamagePiece, TickState};
use mc_data::loot::entity_26_1_2::{
    EntityDeathCause, EntityLootAttack, EntityLootEntity, EntityLootPlayerAttribution,
};
use mc_data::loot::{LootContextItem, LootEnchantments};
use mc_data::{Identifier, ItemStack};
use std::sync::Arc;

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}
fn state() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(
        mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks)),
    );
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 0, 64)
}
fn mob(state: &mut TickState, name: &str, health: f32) -> EntityId {
    let kind = state.registry.entity_types.id_of(&id(name)).unwrap();
    state.spawn_mob(kind, [0.5, 80.0, 0.5], health)
}
fn source(attacker: EntityId) -> DamageSource {
    DamageSource::Melee {
        attacker,
        position: [1.5, 80.0, 0.5],
        yaw: 0.0,
        bonus_knockback: 0.0,
        sweep: false,
        fire_ticks: 0,
        bane_ticks: 0,
    }
}
fn cause(player: bool, looting: u32) -> EntityDeathCause {
    let mut attacker = EntityLootEntity::new(id("minecraft:player"));
    attacker.active_enchantments = LootEnchantments::try_from_levels(
        (looting > 0).then(|| (id("minecraft:looting"), looting)),
    )
    .unwrap();
    attacker.mainhand = Some(
        LootContextItem::new(id("minecraft:diamond_sword"))
            .with_enchantments(attacker.active_enchantments.clone()),
    );
    if player {
        EntityDeathCause::new(
            Default::default(),
            EntityLootAttack::Direct(attacker.clone()),
            Some(EntityLootPlayerAttribution::try_new(attacker).unwrap()),
        )
    } else {
        EntityDeathCause::new(Default::default(), EntityLootAttack::None, None)
    }
}
fn drops(
    registry: &CoreRegistry,
    name: &str,
    burning: bool,
    baby: bool,
    size: Option<i32>,
    seed: u64,
    attribution: EntityDeathCause,
) -> Vec<ItemStack> {
    let mut entity = EntityLootEntity::new(id(name));
    entity.flags.is_on_fire = burning;
    entity.flags.is_baby = baby;
    entity.slime_size = size;
    crate::mob_loot::mob_drops(
        registry,
        entity,
        &id(&format!(
            "minecraft:entities/{}",
            name.trim_start_matches("minecraft:")
        )),
        [0.0; 3],
        seed,
        None,
        attribution,
    )
}
#[test]
fn contextual_burning_player_credit_looting_and_slime_size_use_vanilla_tables() {
    let registry = CoreRegistry::load(None).unwrap();
    let cooked = drops(
        &registry,
        "minecraft:cow",
        true,
        false,
        None,
        5,
        cause(false, 0),
    );
    assert!(cooked.iter().any(
        |stack| registry.items.name_of(stack.item_id).unwrap().as_str() == "minecraft:cooked_beef"
    ));
    assert!(
        cooked.iter().all(
            |stack| registry.items.name_of(stack.item_id).unwrap().as_str() != "minecraft:beef"
        )
    );
    assert!(
        drops(
            &registry,
            "minecraft:blaze",
            false,
            false,
            None,
            3,
            cause(false, 0)
        )
        .is_empty()
    );
    let count = |looting| {
        (0..32)
            .map(|seed| {
                drops(
                    &registry,
                    "minecraft:blaze",
                    false,
                    false,
                    None,
                    seed,
                    cause(true, looting),
                )
                .iter()
                .map(|stack| stack.count)
                .sum::<i32>()
            })
            .sum::<i32>()
    };
    assert!(count(0) > 0);
    assert!(count(3) > count(0));
    assert!(
        drops(
            &registry,
            "minecraft:slime",
            false,
            false,
            Some(4),
            7,
            cause(true, 0)
        )
        .is_empty()
    );
    assert!((0..32).any(|seed| {
        !drops(
            &registry,
            "minecraft:slime",
            false,
            false,
            Some(1),
            seed,
            cause(true, 0),
        )
        .is_empty()
    }));
}
#[test]
fn equipment_and_default_native_attributes_mitigate_damage_without_double_armor() {
    let mut state = state();
    let zombie = mob(&mut state, "minecraft:zombie", 20.0);
    let slot = state.entities.slot_of(zombie).unwrap();
    let armor = effective_attribute(
        &state.registry.item_rules(),
        state.entities.row(slot),
        "armor",
        0.0,
    );
    assert_eq!(armor, 2.0);
    let bare = reduce_damage_with_breach(
        &state.registry.item_rules(),
        state.entities.row(slot),
        10.0,
        DamageKind::Melee,
        0,
    );
    let chest = state
        .registry
        .items
        .id_of(&id("minecraft:diamond_chestplate"))
        .unwrap();
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[4] =
        Some(ItemStack::new(chest, 1));
    let armored = reduce_damage_with_breach(
        &state.registry.item_rules(),
        state.entities.row(slot),
        10.0,
        DamageKind::Melee,
        0,
    );
    assert!(armored < bare);
    let rules = state.registry.item_rules();
    let row = state.entities.row(slot);
    let expected = mc_data::armor::armor_reduced_damage(
        10.0,
        mc_data::armor::ArmorStats {
            armor: effective_attribute(&rules, row, "armor", 0.0) as f32,
            toughness: effective_attribute(&rules, row, "armor_toughness", 0.0) as f32,
        },
    );
    assert_eq!(armored, expected);
    assert_eq!(
        reduce_damage_with_breach(&rules, row, 10.0, DamageKind::Void, 0),
        10.0
    );
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 10.0,
            kind: DamageKind::Melee,
            source: None,
        },
    ));
    state.commit_merge_effects();
    assert_eq!(state.entities.row(slot).health, 20.0 - armored);
    assert_eq!(
        state.entities.row(slot).appearance.equipment[4]
            .as_ref()
            .unwrap()
            .damage,
        Some(2)
    );
}
#[test]
fn retained_attribute_operations_and_equipment_slot_matching_drive_native_values() {
    let mut state = state();
    let cow = mob(&mut state, "minecraft:cow", 10.0);
    let slot = state.entities.slot_of(cow).unwrap();
    let row = state.entities.row_mut(slot);
    row.loot.attributes = vec![EntityAttribute {
        id: "minecraft:movement_speed".into(),
        base: 0.2,
        modifiers: vec![
            EntityAttributeModifier {
                id: "test:add".into(),
                amount: 0.1,
                operation: "add_value".into(),
            },
            EntityAttributeModifier {
                id: "test:base".into(),
                amount: 0.5,
                operation: "add_multiplied_base".into(),
            },
            EntityAttributeModifier {
                id: "test:total".into(),
                amount: 1.0,
                operation: "add_multiplied_total".into(),
            },
        ],
    }];
    restore(&mut state, slot);
    assert!((state.entities.row(slot).speed - 9.0).abs() < 0.0001);
    let sword = state
        .registry
        .items
        .id_of(&id("minecraft:diamond_sword"))
        .unwrap();
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[1] =
        Some(ItemStack::new(sword, 1));
    assert_eq!(
        effective_attribute(
            &state.registry.item_rules(),
            state.entities.row(slot),
            "attack_damage",
            0.0
        ),
        0.0
    );
    let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
    appearance.equipment[0] = appearance.equipment[1].take();
    assert!(
        effective_attribute(
            &state.registry.item_rules(),
            state.entities.row(slot),
            "attack_damage",
            0.0
        ) > 0.0
    );
}

fn clear_melee_lane(state: &mut TickState) {
    state.world.ensure_blocking(
        &[(mc_world::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    for x in 0..=1 {
        for y in 80..=83 {
            state.world.set_block(x, y, 0, state.registry.states.air);
        }
    }
    state.world.fold_overlay();
}
#[test]
fn accepted_cave_spider_melee_poisons_by_difficulty_but_blocked_hits_do_not() {
    for (difficulty, expected) in [
        (Difficulty::Peaceful, None),
        (Difficulty::Easy, None),
        (Difficulty::Normal, Some(140)),
        (Difficulty::Hard, Some(300)),
    ] {
        let mut state = state();
        state.difficulty = difficulty;
        clear_melee_lane(&mut state);
        let spider = mob(&mut state, "minecraft:cave_spider", 12.0);
        let cow = mob(&mut state, "minecraft:cow", 10.0);
        let slot = state.entities.slot_of(cow).unwrap();
        state.entities.row_mut(slot).protected_until = 10;
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 2.0,
                kind: DamageKind::Melee,
                source: Some(source(spider)),
            },
        ));
        state.commit_merge_effects();
        assert!(
            state
                .entities
                .row(slot)
                .active_effects
                .get(crate::effects::EffectId::Poison)
                .is_none()
        );
        state.tick = 10;
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 2.0,
                kind: DamageKind::Melee,
                source: Some(source(spider)),
            },
        ));
        state.commit_merge_effects();
        assert_eq!(
            state
                .entities
                .row(slot)
                .active_effects
                .get(crate::effects::EffectId::Poison)
                .map(|effect| effect.duration),
            expected
        );
    }
}
#[test]
fn cave_spider_effect_is_reliable_for_player_and_survives_native_effect_serialization() {
    let mut state = state();
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "target".into(),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    state.difficulty = Difficulty::Normal;
    let spider = mob(&mut state, "minecraft:cave_spider", 12.0);
    clear_melee_lane(&mut state);
    state.entities.set_pos(slot, [0.5, 80.0, 0.5]);
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 2.0,
            kind: DamageKind::Melee,
            source: Some(source(spider)),
        },
    ));
    state.commit_merge_effects();
    assert_eq!(
        state.players[&player]
            .active_effects
            .get(crate::effects::EffectId::Poison)
            .unwrap()
            .duration,
        140
    );
    let effects = state.players[&player].active_effects.clone();
    let decoded = crate::journal::active_effects_from_nbt(
        &crate::journal::active_effects_to_nbt(&effects),
        effects.absorption,
    );
    assert_eq!(effects, decoded);
    assert!(state.outbox[&ConnectionId(1)].iter().any(|change| matches!(change,
        crate::boundary::PlayerChange::MobEffect { effect, .. } if effect.id == crate::effects::EffectId::Poison)));
}
fn entry(state: &TickState, entity: EntityId) -> crate::journal::JournalEntry {
    let slot = state.entities.slot_of_any(entity).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let type_id = match body.kind {
        EntityKind::Mob { type_id }
        | EntityKind::Item { type_id }
        | EntityKind::ExperienceOrb { type_id } => type_id,
        _ => panic!("non-player fixture"),
    };
    crate::journal::JournalEntry::Entity {
        id: entity.0,
        uuid: row.uuid.to_string(),
        type_id,
        health: row.health,
        alive: body.alive,
        age: row.age,
        breed_cooldown: row.breed_cooldown,
        pickup_delay: body.pickup_delay,
        item: row.item.as_ref().map(|item| item.full_stack()),
        appearance: crate::journal::EntityAppearanceImage::from(row.appearance.as_ref()),
        active_effects: row.active_effects.clone(),
        villager: row.villager.clone(),
        state: std::sync::Arc::new(crate::journal::EntityStateImage::from_row(body, row)),
        projectile: row.projectile.clone(),
        tick: state.tick,
        pos: state.entities.pos(slot),
        vel: state.entities.vel(slot),
        yaw: state.entities.rot(slot)[0],
        pitch: state.entities.rot(slot)[1],
    }
}
#[test]
fn splitting_is_once_only_and_journal_restart_preserves_half_size_and_equipment() {
    for name in ["minecraft:slime", "minecraft:magma_cube"] {
        let mut original = state();
        let parent = mob(&mut original, name, 16.0);
        let slot = original.entities.slot_of(parent).unwrap();
        set_size(&mut original, slot, 4, true);
        original.entities.row_mut(slot).persistent = true;
        Arc::make_mut(&mut original.entities.row_mut(slot).appearance).custom_name =
            Some("split family".into());
        original.tick = 20;
        original.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 100.0,
                kind: DamageKind::Void,
                source: None,
            },
        ));
        let (deaths, _, children, _) = original.commit_merge_effects();
        assert_eq!(deaths, vec![parent]);
        assert!((2..=4).contains(&children.len()));
        for child in &children {
            let row = original
                .entities
                .row(original.entities.slot_of(*child).unwrap());
            assert_eq!(row.loot.slime_size, Some(2));
            assert_eq!(row.health, 4.0);
            assert!(row.persistent);
            assert_eq!(row.appearance.custom_name.as_deref(), Some("split family"));
        }
        let mut repeated = Vec::new();
        commit_death(&mut original, slot, cause(false, 0), &mut repeated);
        assert!(repeated.is_empty());
        let sword = original
            .registry
            .items
            .id_of(&id("minecraft:diamond_sword"))
            .unwrap();
        let first = original.entities.slot_of(children[0]).unwrap();
        Arc::make_mut(&mut original.entities.row_mut(first).appearance).equipment[0] =
            Some(ItemStack::new(sword, 1));
        let mut entries: Vec<_> = children
            .iter()
            .map(|child| entry(&original, *child))
            .collect();
        entries.push(crate::journal::JournalEntry::RemoveEntity {
            id: parent.0,
            tick: original.tick,
        });
        let dir = std::env::temp_dir().join(format!("loot-restart-{}", uuid::Uuid::new_v4()));
        let mut writer = crate::journal::JournalWriter::open(&dir, 1).unwrap();
        // The fixture jumps directly to the death tick; cover its idle prefix.
        writer.append(1, 20, &entries).unwrap();
        writer.gate(0, original.tick).unwrap();
        drop(writer);
        let batches = crate::journal::read_complete_batches_after(&dir.join("journal"), 0).unwrap();
        let replay = crate::journal::replay_batches(crate::journal::Replay::default(), &batches);
        assert!(!replay.entities.contains_key(&parent.0));
        let mut restarted = state();
        for (entity, image) in &replay.entities {
            crate::core::restore_entity(&mut restarted, *entity, image).unwrap();
        }
        assert_eq!(restarted.entities.len(), children.len());
        for child in &children {
            let row = restarted
                .entities
                .row(restarted.entities.slot_of(*child).unwrap());
            assert_eq!(row.loot.slime_size, Some(2));
            assert_eq!(row.health, 4.0);
            assert!(row.persistent);
            assert_eq!(row.appearance.custom_name.as_deref(), Some("split family"));
        }
        assert_eq!(
            restarted
                .entities
                .row(restarted.entities.slot_of(children[0]).unwrap())
                .appearance
                .equipment[0],
            Some(ItemStack::new(sword, 1))
        );
        assert!(restarted.commit_merge_effects().2.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn native_nbt_keeps_size_attributes_equipment_credit_and_death_marker() {
    let mut state = state();
    let slime = mob(&mut state, "minecraft:slime", 16.0);
    let slot = state.entities.slot_of(slime).unwrap();
    set_size(&mut state, slot, 4, true);
    let sword = state
        .registry
        .items
        .id_of(&id("minecraft:diamond_sword"))
        .unwrap();
    let row = state.entities.row_mut(slot);
    row.loot.player_credit = Some(Box::new(PlayerLootCredit {
        id: 19,
        tick: 40,
        held: ItemStack::new(sword, 1),
    }));
    row.loot.death_handled = true;
    row.loot.drop_chances[0] = 2.0;
    row.loot.body_armor = Some(Box::new(ItemStack::new(sword, 1)));
    row.loot.saddle = Some(Box::new(ItemStack::new(sword, 1)));
    Arc::make_mut(&mut row.appearance).equipment[0] = Some(ItemStack::new(sword, 1));
    let expected = row.loot.clone();
    let equipment = row.appearance.equipment.clone();
    let fields = to_fields(&expected);
    assert!(
        fields
            .iter()
            .any(|(key, tag)| key == "Size" && *tag == mc_nbt::Tag::Int(3))
    );
    assert_eq!(from_fields(&fields).unwrap(), expected);
    assert_eq!(
        equipment_from_fields(
            &state.registry.items,
            &equipment_to_fields(&state.registry.items, &equipment, &expected).unwrap()
        )
        .unwrap(),
        equipment
    );
    let mut native = EntityLootState::default();
    extra_equipment_from_fields(
        &state.registry.items,
        &equipment_to_fields(&state.registry.items, &equipment, &expected).unwrap(),
        &mut native,
    )
    .unwrap();
    assert_eq!(native.body_armor, expected.body_armor);
    assert_eq!(native.saddle, expected.saddle);
    let fields: Vec<_> = fields
        .into_iter()
        .filter(|(key, _)| key != "solaris:entity_loot")
        .collect();
    let vanilla = from_fields(&fields).unwrap();
    assert_eq!(vanilla.slime_size, Some(4));
    assert_eq!(vanilla.attributes, expected.attributes);
    assert_eq!(vanilla.drop_chances, expected.drop_chances);
}
#[test]
fn child_death_suppresses_table_loot_but_guaranteed_equipment_still_drops_once() {
    let mut state = state();
    let child = mob(&mut state, "minecraft:cow", 10.0);
    let slot = state.entities.slot_of(child).unwrap();
    let saddle = state.registry.items.id_of(&id("minecraft:saddle")).unwrap();
    let row = state.entities.row_mut(slot);
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.baby = true;
    appearance.equipment[0] = Some(ItemStack::new(saddle, 1));
    row.loot.drop_chances[0] = 2.0;
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (_, _, drops, _) = state.commit_merge_effects();
    assert_eq!(drops.len(), 1);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(drops[0]).unwrap())
            .item
            .as_ref()
            .unwrap()
            .item_id,
        saddle
    );
    assert!(state.commit_merge_effects().2.is_empty());
}

#[test]
fn hostile_gap_baby_zombies_drop_ordinary_loot() {
    let mut state = state();
    let flesh = state
        .registry
        .items
        .id_of(&id("minecraft:rotten_flesh"))
        .unwrap();
    let mut count = 0;
    for _ in 0..32 {
        let child = mob(&mut state, "minecraft:zombie", 20.0);
        let slot = state.entities.slot_of(child).unwrap();
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance).baby = true;
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 100.0,
                kind: DamageKind::Void,
                source: None,
            },
        ));
        let (_, _, spawned, _) = state.commit_merge_effects();
        count += spawned
            .iter()
            .filter(|id| {
                state
                    .entities
                    .row(state.entities.slot_of(**id).unwrap())
                    .item
                    .as_ref()
                    .is_some_and(|stack| stack.item_id == flesh)
            })
            .count();
    }
    assert!(
        count > 0,
        "baby hostile loot must not use the baby-animal suppression"
    );
}

#[test]
fn hostile_gap_witches_resist_magic_but_not_physical_damage() {
    let mut state = state();
    for (kind, expected) in [
        (DamageKind::Magic, 1.5),
        (DamageKind::Poison, 1.5),
        (DamageKind::Melee, 10.0),
    ] {
        let witch = mob(&mut state, "minecraft:witch", 26.0);
        let slot = state.entities.slot_of(witch).unwrap();
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 10.0,
                kind,
                source: None,
            },
        ));
        state.commit_merge_effects();
        assert!(
            (state.entities.row(slot).health - (26.0 - expected)).abs() < 0.001,
            "{kind:?}"
        );
    }
}

#[test]
fn hostile_gap_natural_equipment_adds_xp_but_picked_up_items_do_not() {
    for chance in [0.085, 2.0] {
        let mut state = state();
        let zombie = mob(&mut state, "minecraft:zombie", 20.0);
        let slot = state.entities.slot_of(zombie).unwrap();
        let sword = state
            .registry
            .items
            .id_of(&id("minecraft:iron_sword"))
            .unwrap();
        let tick = state.tick;
        let row = state.entities.row_mut(slot);
        Arc::make_mut(&mut row.appearance).equipment = Default::default();
        Arc::make_mut(&mut row.appearance).equipment[0] = Some(ItemStack::new(sword, 1));
        row.loot.drop_chances[0] = chance;
        row.last_player_hit = Some((EntityId(999), tick));
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 100.0,
                kind: DamageKind::Void,
                source: None,
            },
        ));
        state.commit_merge_effects();
        let xp: i32 = state
            .entities
            .slots()
            .filter_map(|slot| {
                state
                    .entities
                    .row(slot)
                    .appearance
                    .experience_orb
                    .map(|(value, count)| value * count)
            })
            .sum();
        if chance > 1.0 {
            assert_eq!(xp, 5);
        } else {
            assert!((6..=8).contains(&xp), "natural equipment bonus: {xp}");
        }
    }
}

#[test]
fn environmental_finish_keeps_native_player_credit_until_exactly_one_hundred_ticks() {
    let mut state = state();
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "killer".into(),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    let blaze = mob(&mut state, "minecraft:blaze", 20.0);
    let slot = state.entities.slot_of(blaze).unwrap();
    state.tick = 20;
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 1.0,
            kind: DamageKind::Melee,
            source: Some(source(player)),
        },
    ));
    state.commit_merge_effects();
    let image = state.entities.row(slot).loot.clone();
    assert_eq!(image.player_credit.as_ref().unwrap().id, player.0);
    state.tick = 119;
    let cause = death_cause(&state, slot, None, DamageKind::OnFire);
    assert!(cause.attributed_player().is_some());
    assert!(cause.source_attacker().is_none());
    assert!(cause.tags().contains(&id("minecraft:is_fire")));
    state.entities.row_mut(slot).loot = from_fields(&to_fields(&image)).unwrap();
    restore(&mut state, slot);
    assert_eq!(state.entities.row(slot).last_player_hit, Some((player, 20)));
    assert!(
        death_cause(&state, slot, None, DamageKind::OnFire)
            .attributed_player()
            .is_some()
    );
    state.tick = 120;
    assert!(
        death_cause(&state, slot, None, DamageKind::OnFire)
            .attributed_player()
            .is_none()
    );
}

#[test]
fn body_armor_and_saddle_drop_without_player_and_retained_guard_prevents_duplicates() {
    let mut state = state();
    let horse = mob(&mut state, "minecraft:horse", 15.0);
    let slot = state.entities.slot_of(horse).unwrap();
    let body = state
        .registry
        .items
        .id_of(&id("minecraft:iron_horse_armor"))
        .unwrap();
    let saddle = state.registry.items.id_of(&id("minecraft:saddle")).unwrap();
    state.entities.row_mut(slot).loot.body_armor = Some(Box::new(ItemStack::new(body, 1)));
    state.entities.row_mut(slot).loot.saddle = Some(Box::new(ItemStack::new(saddle, 1)));
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (_, _, dropped, _) = state.commit_merge_effects();
    let items: Vec<_> = dropped
        .iter()
        .filter_map(|entity| state.entities.slot_of(*entity))
        .filter_map(|slot| {
            state
                .entities
                .row(slot)
                .item
                .as_ref()
                .map(|item| item.item_id)
        })
        .collect();
    assert!(items.contains(&body));
    assert!(items.contains(&saddle));
    let saved = from_fields(&to_fields(&state.entities.row(slot).loot)).unwrap();
    state.entities.row_mut(slot).loot = saved;
    let mut repeated = Vec::new();
    commit_death(&mut state, slot, cause(false, 0), &mut repeated);
    assert!(repeated.is_empty());
}

#[test]
fn disabled_mob_drops_still_split_slimes_but_emit_no_items() {
    let mut state = state();
    state.gamerules.insert("mob_drops".into(), "false".into());
    let slime = mob(&mut state, "minecraft:slime", 16.0);
    let slot = state.entities.slot_of(slime).unwrap();
    set_size(&mut state, slot, 4, true);
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (_, _, children, _) = state.commit_merge_effects();
    assert!((2..=4).contains(&children.len()));
    assert!(children.iter().all(|child| matches!(
        state
            .entities
            .body(state.entities.slot_of(*child).unwrap())
            .kind,
        EntityKind::Mob { .. }
    )));
}

#[test]
fn vanishing_prevents_guaranteed_equipment_drop_without_suppressing_other_slots() {
    let mut state = state();
    let cow = mob(&mut state, "minecraft:cow", 10.0);
    let slot = state.entities.slot_of(cow).unwrap();
    let sword = state
        .registry
        .items
        .id_of(&id("minecraft:diamond_sword"))
        .unwrap();
    let saddle = state.registry.items.id_of(&id("minecraft:saddle")).unwrap();
    let mut cursed = ItemStack::new(sword, 1);
    cursed.enchantments.push(mc_data::ItemEnchantment {
        id: id("minecraft:vanishing_curse"),
        level: 1,
    });
    let row = state.entities.row_mut(slot);
    Arc::make_mut(&mut row.appearance).equipment[0] = Some(cursed);
    row.loot.drop_chances[0] = 2.0;
    row.loot.saddle = Some(Box::new(ItemStack::new(saddle, 1)));
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (_, _, drops, _) = state.commit_merge_effects();
    let items: Vec<_> = drops
        .iter()
        .filter_map(|entity| state.entities.slot_of(*entity))
        .filter_map(|slot| {
            state
                .entities
                .row(slot)
                .item
                .as_ref()
                .map(|item| item.item_id)
        })
        .collect();
    assert!(!items.contains(&sword));
    assert!(items.contains(&saddle));
}

#[test]
fn projectile_death_context_keeps_actual_direct_type_and_skeleton_disc_roll() {
    let mut state = state();
    let skeleton = mob(&mut state, "minecraft:skeleton", 20.0);
    let creeper = mob(&mut state, "minecraft:creeper", 20.0);
    let slot = state.entities.slot_of(creeper).unwrap();
    let arrow_type = state
        .registry
        .entity_types
        .id_of(&id("minecraft:arrow"))
        .unwrap();
    let arrow_source = DamageSource::Projectile {
        projectile: EntityId(987),
        direct_type_id: arrow_type,
        owner: skeleton,
        velocity: [1.0, 0.0],
        punch: 0,
        flame: false,
    };
    let context = death_cause(&state, slot, Some(arrow_source), DamageKind::Projectile);
    assert_eq!(
        context.direct_attacker().unwrap().entity_type,
        id("minecraft:arrow")
    );
    assert_eq!(
        context.source_attacker().unwrap().entity_type,
        id("minecraft:skeleton")
    );
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: DamageKind::Projectile,
            source: Some(arrow_source),
        },
    ));
    let (_, _, drops, _) = state.commit_merge_effects();
    assert!(
        drops
            .iter()
            .filter_map(|entity| state.entities.slot_of(*entity))
            .filter_map(|slot| state.entities.row(slot).item.as_ref())
            .any(|item| state
                .registry
                .items
                .name_of(item.item_id)
                .unwrap()
                .as_str()
                .starts_with("minecraft:music_disc_"))
    );
    let fireball_type = state
        .registry
        .entity_types
        .id_of(&id("minecraft:fireball"))
        .unwrap();
    let target = mob(&mut state, "minecraft:cow", 10.0);
    let target = state.entities.slot_of(target).unwrap();
    let context = death_cause(
        &state,
        target,
        Some(DamageSource::Projectile {
            direct_type_id: fireball_type,
            projectile: EntityId(988),
            owner: skeleton,
            velocity: [1.0, 0.0],
            punch: 0,
            flame: false,
        }),
        DamageKind::Fireball,
    );
    assert_eq!(
        context.direct_attacker().unwrap().entity_type,
        id("minecraft:fireball")
    );
    assert!(context.tags().contains(&id("minecraft:is_fire")));
    assert!(context.tags().contains(&id("minecraft:is_projectile")));
}

#[test]
fn retained_animal_equipment_updates_appearance_only_when_it_changes() {
    let mut state = state();
    let horse = mob(&mut state, "minecraft:horse", 15.0);
    let slot = state.entities.slot_of(horse).unwrap();
    let body = state
        .registry
        .items
        .id_of(&id("minecraft:diamond_horse_armor"))
        .unwrap();
    let saddle = state.registry.items.id_of(&id("minecraft:saddle")).unwrap();
    let before = std::sync::Arc::clone(&state.entities.row(slot).appearance);
    state.entities.row_mut(slot).loot.body_armor = Some(Box::new(mc_data::ItemStack::new(body, 1)));
    state.entities.row_mut(slot).loot.saddle = Some(Box::new(mc_data::ItemStack::new(saddle, 1)));
    restore(&mut state, slot);
    let row = state.entities.row(slot);
    assert_eq!(
        row.appearance.body_armor.as_ref(),
        row.loot.body_armor.as_deref()
    );
    assert_eq!(row.appearance.saddle.as_ref(), row.loot.saddle.as_deref());
    assert!(!std::sync::Arc::ptr_eq(&before, &row.appearance));
    let equipped = std::sync::Arc::clone(&row.appearance);
    restore(&mut state, slot);
    assert!(std::sync::Arc::ptr_eq(
        &equipped,
        &state.entities.row(slot).appearance
    ));
    state.entities.row_mut(slot).loot.body_armor = None;
    state.entities.row_mut(slot).loot.saddle = None;
    restore(&mut state, slot);
    let appearance = &state.entities.row(slot).appearance;
    assert!(appearance.body_armor.is_none() && appearance.saddle.is_none());
    assert!(!std::sync::Arc::ptr_eq(&equipped, appearance));
}

#[test]
fn slime_sizes_scale_health_attack_hitboxes_and_spawn_only_vanilla_sizes() {
    for name in ["minecraft:slime", "minecraft:magma_cube"] {
        let mut state = state();
        let entity = mob(&mut state, name, 1.0);
        let slot = state.entities.slot_of(entity).unwrap();
        for size in [1, 2, 4] {
            set_size(&mut state, slot, size, true);
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            assert_eq!(row.health, f32::from(size).powi(2));
            assert_eq!(
                row.attack_damage,
                if name.ends_with("magma_cube") {
                    f32::from(size) + 2.0
                } else if size == 1 {
                    0.0
                } else {
                    f32::from(size)
                }
            );
            let bp = state
                .registry
                .blueprint(match body.kind {
                    EntityKind::Mob { type_id } => type_id,
                    _ => unreachable!(),
                })
                .unwrap();
            assert_eq!(body.half_width, bp.half_width * f64::from(size));
            assert_eq!(body.height, bp.height * f64::from(size));
            assert!(
                row.appearance
                    .extra
                    .contains(&(18, EntityDataValue::VarInt(i32::from(size))))
            );
            assert!(
                !row.appearance
                    .extra
                    .iter()
                    .any(|(index, value)| *index == 16
                        && matches!(value, EntityDataValue::VarInt(_))),
                "slime size must not overwrite AgeableMob's baby Boolean at index 16"
            );
            assert!(
                to_fields(&row.loot)
                    .contains(&("Size".into(), mc_nbt::Tag::Int(i32::from(size) - 1)))
            );
            assert!(state.appearance_changed.contains(&entity));
        }
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..96 {
            let entity = mob(&mut state, name, 1.0);
            let size = state
                .entities
                .row(state.entities.slot_of(entity).unwrap())
                .loot
                .slime_size
                .unwrap();
            assert!([1, 2, 4].contains(&size));
            seen.insert(size);
        }
        assert_eq!(seen, [1, 2, 4].into_iter().collect());
    }
}

/// A cube saved by a 26.1.2 Solaris carries size as a VarInt at index 16;
/// restoring it on 26.3 must leave only the index-18 size.
#[test]
fn restored_cube_drops_its_26_1_2_size_index() {
    for name in ["minecraft:slime", "minecraft:magma_cube"] {
        let mut state = state();
        let entity = mob(&mut state, name, 1.0);
        let slot = state.entities.slot_of(entity).unwrap();
        let row = state.entities.row_mut(slot);
        row.loot.slime_size = Some(2);
        std::sync::Arc::make_mut(&mut row.appearance)
            .extra
            .push((16, EntityDataValue::VarInt(2)));
        restore(&mut state, slot);
        let extra = &state.entities.row(slot).appearance.extra;
        assert!(extra.contains(&(18, EntityDataValue::VarInt(2))), "{name}");
        assert!(
            !extra.iter().any(|(index, _)| *index == 16),
            "{name}: stale 26.1.2 size at index 16 survived restore"
        );
    }
}

#[test]
fn villager_carry_drops_once_on_death() {
    let mut state = state();
    let villager = mob(&mut state, "minecraft:villager", 20.0);
    let slot = state.entities.slot_of(villager).unwrap();
    let cobble = state
        .registry
        .items
        .id_of(&id("minecraft:cobblestone"))
        .unwrap();
    let planks = state
        .registry
        .items
        .id_of(&id("minecraft:oak_planks"))
        .unwrap();
    let mut carrier = crate::villager::VillagerState::default();
    carrier.inventory[0] = Some(ItemStack::new(cobble, 12));
    carrier.inventory[3] = Some(ItemStack::new(planks, 5));
    state.entities.row_mut(slot).villager = Some(Box::new(carrier));
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (_, _, dropped, _) = state.commit_merge_effects();
    let mut items: Vec<_> = dropped
        .iter()
        .filter_map(|entity| state.entities.slot_of(*entity))
        .filter_map(|slot| state.entities.row(slot).item.clone())
        .map(|stack| (stack.item_id, stack.count))
        .collect();
    items.sort_unstable();
    let mut expected = vec![(cobble, 12), (planks, 5)];
    expected.sort_unstable();
    assert_eq!(items, expected);
    assert!(state.commit_merge_effects().2.is_empty());
}
