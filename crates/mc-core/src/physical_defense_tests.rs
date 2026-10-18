use super::*;
use mc_data::Identifier;
use mc_data::item_components::{CustomItemDefinition, ItemFacts};
use mc_data::item_profiles::{ArmorCoverage, ArmorProfile, DamageResistance, DegradationPoint};
use std::sync::Arc;

// The admitted armor/shield field values are the real authored physical values,
// with unrelated provider identity deliberately absent from native mechanics.
fn mail() -> PhysicalItemProfile {
    PhysicalItemProfile {
        attacks: Vec::new(),
        stamina_capacity: 100.0,
        stamina_recovery_per_tick: 0.5,
        shield: None,
        armor: Some(ArmorProfile {
            slot: ArmorSlot::Chest,
            coverage: vec![
                ArmorCoverage {
                    region: CoverageRegion::Torso,
                    fraction: 1.0,
                },
                ArmorCoverage {
                    region: CoverageRegion::Arms,
                    fraction: 0.75,
                },
                ArmorCoverage {
                    region: CoverageRegion::Legs,
                    fraction: 0.2,
                },
            ],
            resistance: [
                (DamageKind::Cut, 0.45),
                (DamageKind::Pierce, 0.30),
                (DamageKind::Crush, 0.12),
                (DamageKind::Fire, 0.05),
                (DamageKind::Blast, 0.10),
                (DamageKind::Hook, 0.0),
            ]
            .into_iter()
            .map(|(kind, fraction)| DamageResistance { kind, fraction })
            .collect(),
            common_cap: 0.60,
            mass_units: 8.0,
            stamina_cost: 2.0,
            degradation: vec![
                DegradationPoint {
                    remaining_fraction: 0.0,
                    resistance_multiplier: 0.2,
                },
                DegradationPoint {
                    remaining_fraction: 0.25,
                    resistance_multiplier: 0.6,
                },
                DegradationPoint {
                    remaining_fraction: 1.0,
                    resistance_multiplier: 1.0,
                },
            ],
            wear_per_damage_hp: 0.5,
        }),
    }
}
fn pavise() -> PhysicalItemProfile {
    PhysicalItemProfile {
        attacks: Vec::new(),
        armor: None,
        stamina_capacity: 100.0,
        stamina_recovery_per_tick: 0.5,
        shield: Some(ShieldProfile {
            width_blocks: 1.0,
            height_blocks: 1.75,
            forward_offset_blocks: 0.55,
            bottom_offset_blocks: 0.0,
            activation_ticks: 8,
            manual: true,
            deployable: true,
            block_stamina_cost: 14.0,
            block_wear_per_damage_hp: 0.75,
            max_durability: 360,
            hook_disables_ticks: 30,
            impulse_limit: 1.2,
        }),
    }
}
fn pose() -> DefensePose {
    DefensePose {
        position: [0.0; 3],
        yaw: 0.0,
        body_height: 2.0,
        half_width: 0.3,
    }
}
fn impact(kind: DamageKind) -> PhysicalImpact {
    PhysicalImpact {
        damage_hp: 10.0,
        kind,
        source: None,
        origin: [0.0, 1.2, 3.0],
        point: [0.0, 1.2, 0.0],
        impulse: 0.18,
        collision_radius: 0.0,
    }
}
fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
}

#[test]
fn authored_mail_applies_all_typed_resistances_and_resource_costs() {
    let profile = mail();
    let item = DefenseGear {
        slot: 4,
        profile: &profile,
        damage: 0,
        max_damage: 480,
    };
    for (kind, damage) in [
        (DamageKind::Cut, 5.5),
        (DamageKind::Pierce, 7.0),
        (DamageKind::Crush, 8.8),
        (DamageKind::Fire, 9.5),
        (DamageKind::Blast, 9.0),
    ] {
        let plan = plan_physical_defense(pose(), impact(kind), &[item], None, 100.0, 1).unwrap();
        close(plan.damage_hp, damage);
        close(plan.stamina, 98.0);
        close(plan.armor_mass, 8.0);
        assert_eq!(plan.wear[4].unwrap().updated_damage, 5);
    }
    let hook =
        plan_physical_defense(pose(), impact(DamageKind::Hook), &[item], None, 100.0, 1).unwrap();
    close(hook.damage_hp, 10.0);
    assert_eq!(hook.wear[4], None);
    close(hook.stamina, 100.0);
}

#[test]
fn coverage_real_body_regions_degradation_common_cap_and_native_slots() {
    let profile = mail();
    let mut item = DefenseGear {
        slot: 4,
        profile: &profile,
        damage: 0,
        max_damage: 480,
    };
    for (point, expected) in [
        ([0.25, 1.2, 0.0], 6.625),
        ([0.0, 0.6, 0.0], 9.1),
        ([0.0, 1.8, 0.0], 10.0),
        ([0.0, 0.1, 0.0], 10.0),
        ([1.0, 1.2, 0.0], 10.0),
    ] {
        let mut hit = impact(DamageKind::Cut);
        hit.point = point;
        close(
            plan_physical_defense(pose(), hit, &[item], None, 100.0, 1)
                .unwrap()
                .damage_hp,
            expected,
        );
    }
    item.damage = 360; // 25% remaining: authored multiplier 0.6.
    close(
        plan_physical_defense(pose(), impact(DamageKind::Cut), &[item], None, 100.0, 1)
            .unwrap()
            .damage_hp,
        7.3,
    );
    item.damage = 240; // Linear interpolation above the degradation knot.
    close(
        plan_physical_defense(pose(), impact(DamageKind::Cut), &[item], None, 100.0, 1)
            .unwrap()
            .damage_hp,
        6.7,
    );
    item.damage = 480;
    let broken =
        plan_physical_defense(pose(), impact(DamageKind::Cut), &[item], None, 100.0, 1).unwrap();
    close(broken.damage_hp, 10.0);
    close(broken.armor_mass, 0.0);
    item.damage = 0;
    item.slot = 3; // v1 chest index must never protect a native leg slot.
    close(
        plan_physical_defense(pose(), impact(DamageKind::Cut), &[item], None, 100.0, 1)
            .unwrap()
            .damage_hp,
        10.0,
    );
    let mut second = mail();
    second.armor.as_mut().unwrap().slot = ArmorSlot::Head;
    let head = DefenseGear {
        slot: 5,
        profile: &second,
        damage: 0,
        max_damage: 480,
    };
    item.slot = 4;
    close(
        plan_physical_defense(
            pose(),
            impact(DamageKind::Cut),
            &[item, head],
            None,
            100.0,
            1,
        )
        .unwrap()
        .damage_hp,
        4.0,
    );
    assert_eq!(
        [
            armor_equipment_index(ArmorSlot::Feet),
            armor_equipment_index(ArmorSlot::Legs),
            armor_equipment_index(ArmorSlot::Chest),
            armor_equipment_index(ArmorSlot::Head)
        ],
        [2, 3, 4, 5]
    );
}

#[test]
fn manual_pavise_actual_rectangle_activation_impulse_stamina_and_hook() {
    let profile = pavise();
    let item = DefenseGear {
        slot: 1,
        profile: &profile,
        damage: 0,
        max_damage: 360,
    };
    let active = Some(ShieldActivation {
        slot: 1,
        started_tick: 10,
        disabled_until: 0,
    });
    let plan = |hit, stamina, tick| {
        plan_physical_defense(pose(), hit, &[item], active, stamina, tick).unwrap()
    };
    assert!(!plan(impact(DamageKind::Pierce), 100.0, 17).shield_blocked);
    let blocked = plan(impact(DamageKind::Pierce), 100.0, 18);
    assert!(blocked.shield_blocked);
    close(blocked.damage_hp, 0.0);
    close(blocked.stamina, 86.0);
    assert_eq!(blocked.wear[1].unwrap().updated_damage, 8);
    assert!(!plan(impact(DamageKind::Pierce), 13.999, 18).shield_blocked);
    let mut hit = impact(DamageKind::Pierce);
    hit.origin[2] = -3.0; // Rear-facing contact.
    assert!(!plan(hit, 100.0, 18).shield_blocked);
    hit.origin = [3.0, 1.2, 0.0]; // A flank is not a paper carrier shield.
    assert!(!plan(hit, 100.0, 18).shield_blocked);
    hit = impact(DamageKind::Pierce);
    hit.origin[1] = 4.0;
    hit.point[1] = 4.0;
    assert!(!plan(hit, 100.0, 18).shield_blocked);
    hit = impact(DamageKind::Pierce);
    hit.impulse = 1.2001;
    assert!(!plan(hit, 100.0, 18).shield_blocked);
    hit.kind = DamageKind::Hook;
    let hooked = plan(hit, 100.0, 18);
    assert!(!hooked.shield_blocked);
    assert_eq!(hooked.shield_disabled_until, Some(48));
    let disabled = Some(ShieldActivation {
        slot: 1,
        started_tick: 10,
        disabled_until: 48,
    });
    assert!(
        !plan_physical_defense(
            pose(),
            impact(DamageKind::Pierce),
            &[item],
            disabled,
            100.0,
            47
        )
        .unwrap()
        .shield_blocked
    );
    assert!(
        plan_physical_defense(
            pose(),
            impact(DamageKind::Pierce),
            &[item],
            disabled,
            100.0,
            48
        )
        .unwrap()
        .shield_blocked
    );
}

#[test]
fn deployed_plan_has_same_geometry_without_duplicate_actor_stamina() {
    let profile = pavise();
    let shield = profile.shield.as_ref().unwrap();
    let active = ShieldActivation {
        slot: 0,
        started_tick: 10,
        disabled_until: 0,
    };
    let pose = DeployedShieldPose {
        position: [0.0; 3],
        yaw: 0.0,
    };
    let result =
        plan_deployed_shield_impact(shield, pose, impact(DamageKind::Hook), 354, 360, active, 18)
            .unwrap();
    assert!(result.shield_blocked);
    assert_eq!(result.shield_disabled_until, Some(48));
    assert!(result.wear[0].unwrap().broken);
    assert_eq!(result.wear[0].unwrap().updated_damage, 360);
    let mut hit = impact(DamageKind::Crush);
    hit.impulse = 1.3;
    let bypass = plan_deployed_shield_impact(shield, pose, hit, 0, 360, active, 18).unwrap();
    assert!(!bypass.shield_blocked);
    close(bypass.damage_hp, 10.0);
    assert_eq!(bypass.wear[0].unwrap().updated_damage, 8);
    assert_eq!(
        plan_deployed_item_damage(shield, hit, 0, 360)
            .unwrap()
            .updated_damage,
        8
    );
}

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}
fn fixture() -> (
    TickState,
    crate::boundary::EntityId,
    crate::boundary::EntityId,
) {
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    let definitions = [
        ("example:mail", mail(), 480, Some("chest")),
        ("example:shield", pavise(), 360, None),
    ]
    .into_iter()
    .map(|(name, profile, max_damage, slot)| CustomItemDefinition {
        id: id(name),
        carrier: id("minecraft:paper"),
        name: name.into(),
        crafting_ingredient: None,
        facts: ItemFacts {
            max_stack_size: Some(1),
            max_damage: Some(max_damage),
            equippable_slot: slot.map(str::to_owned),
            physical_profile: Some(Box::new(profile)),
            ..ItemFacts::default()
        },
    });
    registry.item_facts = registry
        .item_facts
        .clone()
        .with_custom_items(definitions, &registry.items)
        .unwrap();
    let registry = Arc::new(registry);
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(
        mc_worldgen::TerrainGenerator::new(42, registry.blocks.clone()),
    );
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 0, 512);
    crate::tick::run_tick(
        &mut state,
        &[crate::boundary::PlayerInput::Join {
            connection: crate::boundary::ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "Defender".into(),
            permission_level: 0,
        }],
    );
    let player = state
        .connections
        .entity_of(crate::boundary::ConnectionId(1))
        .unwrap();
    let npc = state.spawn_mob(state.registry.zombie_type_id, [0.0, 80.0, 0.0], 20.0);
    let actor = state.entities.slot_of(player).unwrap();
    state.entities.row_mut(actor).protected_until = 0;
    (state, player, npc)
}
fn stack(state: &TickState, name: &str) -> ItemStack {
    let mut stack = ItemStack::new(
        state.registry.items.id_of(&id("minecraft:paper")).unwrap(),
        1,
    );
    stack.item_model = Some(Arc::new(id(name)));
    stack.damage = Some(0);
    stack
}
fn piece(state: &TickState, slot: EntitySlot, kind: DamageKind) -> DamagePiece {
    let mut hit = impact(kind);
    hit.point = state.entities.pos(slot);
    hit.point[1] += state.entities.body(slot).height * 0.6;
    hit.origin = [hit.point[0], hit.point[1], hit.point[2] + 3.0];
    DamagePiece {
        amount: hit.damage_hp,
        kind: crate::survival::DamageKind::Melee,
        source: None,
        physical: Some(hit),
        custom_armor: false,
    }
}

#[test]
fn native_player_and_npc_armor_mutate_canonical_equipment_not_carrier_defaults() {
    let (mut state, player, npc) = fixture();
    let armor = stack(&state, "example:mail");
    state.players.get_mut(&player).unwrap().inventory.slots[38] = armor.clone();
    let npc_slot = state.entities.slot_of(npc).unwrap();
    Arc::make_mut(&mut state.entities.row_mut(npc_slot).appearance).equipment[4] = Some(armor);
    assert_eq!(state.entities.row(npc_slot).loot.base_armor, 2.0);
    for slot in [state.entities.slot_of(player).unwrap(), npc_slot] {
        state.entities.row_mut(slot).protected_until = 0;
        let mut hit = piece(&state, slot, DamageKind::Cut);
        assert!(reduce_damage(&mut state, slot, &mut hit));
        // Custom coverage composes with native innate armor; a zombie is not
        // an armor-free carrier for the authored mail profile.
        let expected = if slot == npc_slot {
            mc_data::armor::armor_reduced_damage(
                5.5,
                mc_data::armor::ArmorStats {
                    armor: 2.0,
                    toughness: 0.0,
                },
            )
        } else {
            5.5
        };
        close(hit.amount, expected);
        assert_eq!(equipment(&state, slot, 4).unwrap().damage, Some(5));
        close(
            crate::physical_attacks::stamina(&state, slot.0 as usize, 100.0, 0.5),
            98.0,
        );
        close(armor_mass(&state, slot.0 as usize), 8.0);
        close(armor_stamina_cost(&state, slot.0 as usize), 2.0);
        assert!(
            state
                .appearance_changed
                .contains(&state.entities.body(slot).id)
        );
    }
    let slot = state.entities.slot_of(player).unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[38].item_id =
        state.registry.items.id_of(&id("minecraft:stick")).unwrap();
    let mut wrong_carrier = piece(&state, slot, DamageKind::Cut);
    assert!(reduce_damage(&mut state, slot, &mut wrong_carrier));
    close(wrong_carrier.amount, 10.0);
}

#[test]
fn mixed_native_and_custom_armor_use_prebreak_mitigation_and_then_release_equipment() {
    let (mut state, player, npc) = fixture();
    let player_slot = state.entities.slot_of(player).unwrap();
    let npc_slot = state.entities.slot_of(npc).unwrap();
    let mut mail_stack = stack(&state, "example:mail");
    mail_stack.damage = Some(479);
    mail_stack.enchantments.push(mc_data::ItemEnchantment {
        id: id("minecraft:protection"),
        level: 2,
    });
    let mut helmet = ItemStack::new(
        state
            .registry
            .items
            .id_of(&id("minecraft:diamond_helmet"))
            .unwrap(),
        1,
    );
    let facts = state
        .registry
        .item_facts
        .facts_for_stack(&helmet, &state.registry.items)
        .unwrap();
    let native_armor = facts.armor.as_ref().unwrap();
    close(native_armor.armor, 3.0);
    close(native_armor.toughness, 2.0);
    helmet.damage = Some(facts.max_damage.unwrap() as i32 - 1);
    helmet.enchantments.push(mc_data::ItemEnchantment {
        id: id("minecraft:protection"),
        level: 1,
    });
    {
        let inventory = &mut state.players.get_mut(&player).unwrap().inventory;
        inventory.slots[38] = mail_stack.clone();
        inventory.slots[39] = helmet.clone();
    }
    {
        let appearance = Arc::make_mut(&mut state.entities.row_mut(npc_slot).appearance);
        appearance.equipment[4] = Some(mail_stack);
        appearance.equipment[5] = Some(helmet);
    }
    assert_eq!(state.entities.row(npc_slot).loot.base_armor, 2.0);
    let profile = mail();
    let custom_reduced = 10.0
        * (1.0
            - profile.armor.as_ref().unwrap().reduction(
                CoverageRegion::Torso,
                DamageKind::Cut,
                1.0 / 480.0,
            ));
    let expected = [3.0, 5.0].map(|armor| {
        mc_data::armor::protection_reduced_damage(
            mc_data::armor::armor_reduced_damage(
                custom_reduced,
                mc_data::armor::ArmorStats {
                    armor,
                    toughness: 2.0,
                },
            ),
            3,
        )
    });
    for actor in [player_slot, npc_slot] {
        state.entities.row_mut(actor).protected_until = 0;
        let hit = piece(&state, actor, DamageKind::Cut);
        state.merge_damage.push((actor, hit));
    }
    state.commit_merge_effects();
    for (actor, damage) in [player_slot, npc_slot].into_iter().zip(expected) {
        close(state.entities.row(actor).health, 20.0 - damage);
        assert!(equipment(&state, actor, 4).is_none());
        assert!(equipment(&state, actor, 5).is_none());
        close(armor_mass(&state, actor.0 as usize), 0.0);
        close(armor_stamina_cost(&state, actor.0 as usize), 0.0);
        close(
            crate::physical_attacks::stamina(&state, actor.0 as usize, 100.0, 0.5),
            98.0,
        );
        assert!(
            state
                .appearance_changed
                .contains(&state.entities.body(actor).id)
        );
    }
    // Neither vanished protection nor custom coverage can survive the break.
    state.tick += crate::entities::HURT_INVULNERABLE_TICKS;
    for actor in [player_slot, npc_slot] {
        let hit = piece(&state, actor, DamageKind::Cut);
        state.merge_damage.push((actor, hit));
    }
    state.commit_merge_effects();
    let bare_npc = mc_data::armor::armor_reduced_damage(
        10.0,
        mc_data::armor::ArmorStats {
            armor: 2.0,
            toughness: 0.0,
        },
    );
    close(
        state.entities.row(player_slot).health,
        20.0 - expected[0] - 10.0,
    );
    close(
        state.entities.row(npc_slot).health,
        20.0 - expected[1] - bare_npc,
    );
}

#[test]
fn absolute_manual_activation_costs_disable_and_equipment_are_disk_wal_replay_safe() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let (mut state, player, npc) = fixture();
    let shield = stack(&state, "example:shield");
    state.players.get_mut(&player).unwrap().inventory.slots[40] = shield.clone();
    let npc_slot = state.entities.slot_of(npc).unwrap();
    Arc::make_mut(&mut state.entities.row_mut(npc_slot).appearance).equipment[1] = Some(shield);
    let player_slot = state.entities.slot_of(player).unwrap();
    for actor in [player_slot, npc_slot] {
        state.entities.row_mut(actor).protected_until = 0;
        state.entities.set_rot(actor, 0.0, 0.0);
        assert!(set_item_use(&mut state, actor, Hand::Off, true));
    }
    let started = state.tick;
    let root = tempfile::tempdir().unwrap();
    let mut writer = JournalWriter::open(root.path(), 1).unwrap();
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_spawned
        .push(npc);
    let baseline = crate::core::journal_entries_for_tick(&mut state, &output);
    writer.append(output.tick, output.tick, &baseline).unwrap();
    let checkpoint = replay_batches(Replay::default(), &[(output.tick, output.tick, baseline)]);
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_spawned
        .clear();
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_appearance_changed
        .clear();
    state.appearance_changed.clear();
    state.tick = started + 8;
    output.tick = state.tick;
    for actor in [player_slot, npc_slot] {
        state.entities.set_rot(actor, 0.0, 0.0);
        let mut hit = piece(&state, actor, DamageKind::Hook);
        assert!(block_damage(&mut state, actor, &mut hit));
        close(hit.amount, 0.0);
        assert_eq!(equipment(&state, actor, 1).unwrap().damage, Some(8));
        assert_eq!(
            item_use(&state, actor).unwrap().disabled_until,
            state.tick + 30
        );
        assert_eq!(item_use(&state, actor).unwrap().equipment_slot, None);
    }
    assert!(state.appearance_changed.contains(&player));
    assert!(state.appearance_changed.contains(&npc));
    // These hits run directly at the merge boundary, after the baseline
    // run_tick sealed its output. Publish their actual dirty equipment IDs
    // exactly as run_dimension_tick does before sealing this new tick.
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_appearance_changed
        .append(&mut state.appearance_changed);
    let after = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(
        after
            .iter()
            .any(|entry| matches!(entry, crate::journal::JournalEntry::Plugins { .. }))
    );
    assert!(after.iter().any(
        |entry| matches!(entry, crate::journal::JournalEntry::Entity { id, .. } if *id == npc.0)
    ));
    assert!(
        after
            .iter()
            .any(|entry| matches!(entry, crate::journal::JournalEntry::Player { .. }))
    );
    // Idle clock advancement still belongs to a contiguous native tick WAL.
    for tick in started + 2..output.tick {
        writer.append(tick, tick, &[]).unwrap();
    }
    writer.append(output.tick, output.tick, &after).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&root.path().join("journal"), started + 1).unwrap();
    let replay = replay_batches(checkpoint, &batches);
    let replay = replay_batches(replay, &batches);
    let uuid = state.entities.row(player_slot).uuid.to_string();
    assert_eq!(replay.players[&uuid].inventory[40].damage, Some(8));
    assert_eq!(
        replay.entities[&npc.0].appearance.equipment[1]
            .as_ref()
            .unwrap()
            .damage,
        Some(8)
    );
    assert_eq!(
        &replay.players[&uuid].inventory[40],
        equipment(&state, player_slot, 1).unwrap(),
    );
    assert_eq!(
        replay.entities[&npc.0].appearance.equipment[1].as_ref(),
        equipment(&state, npc_slot, 1),
    );
    assert_eq!(replay.plugins, state.plugins);
    let mut restored = fixture().0;
    let replaced = restored.entities.slot_of(npc).unwrap();
    restored.entities.remove(replaced);
    crate::core::restore_entity(&mut restored, npc.0, &replay.entities[&npc.0]).unwrap();
    restored.plugins = replay.plugins;
    restored.tick = state.tick;
    let actor = restored
        .entities
        .slot_of_uuid(state.entities.row(npc_slot).uuid)
        .unwrap();
    assert_eq!(equipment(&restored, actor, 1).unwrap().damage, Some(8));
    assert_eq!(
        item_use(&restored, actor).unwrap().disabled_until,
        started + 38
    );
    assert!(!set_item_use(&mut restored, actor, Hand::Off, true));
    close(
        crate::physical_attacks::stamina(&restored, actor.0 as usize, 100.0, 0.5),
        86.0,
    );
}

#[test]
fn shield_stop_does_not_claim_native_weapon_release() {
    let (mut state, player, _) = fixture();
    let actor = state.entities.slot_of(player).unwrap();
    for name in ["minecraft:bow", "minecraft:crossbow", "minecraft:trident"] {
        let item = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap();
        state.players.get_mut(&player).unwrap().inventory.slots[0] =
            mc_data::ItemStack::new(item, 1);
        assert!(
            !set_item_use(&mut state, actor, Hand::Main, false),
            "{name}"
        );
        assert!(
            state
                .plugins
                .get_domain(OWNER, &key(&state, actor))
                .is_none()
        );
    }
    let shield = stack(&state, "example:shield");
    state.players.get_mut(&player).unwrap().inventory.slots[40] = shield;
    assert!(set_item_use(&mut state, actor, Hand::Off, false));
}

#[test]
fn shield_exact_stack_fence_does_not_resume_after_foreign_component_mutation() {
    let (mut state, player, _) = fixture();
    let shield = stack(&state, "example:shield");
    state.players.get_mut(&player).unwrap().inventory.slots[40] = shield;
    let actor = state.entities.slot_of(player).unwrap();
    assert!(set_item_use(&mut state, actor, Hand::Off, true));
    let start = item_use(&state, actor).unwrap().started_tick;
    state.tick += 4;
    assert!(set_item_use(&mut state, actor, Hand::Off, true));
    assert_eq!(item_use(&state, actor).unwrap().started_tick, start);
    state.players.get_mut(&player).unwrap().inventory.slots[40].custom_name =
        Some("different stack".into());
    assert!(item_use(&state, actor).is_none());
    assert!(set_item_use(&mut state, actor, Hand::Off, true));
    assert_eq!(
        item_use(&state, actor).unwrap().started_tick,
        Some(state.tick)
    );
    assert!(set_item_use(&mut state, actor, Hand::Off, false));
    assert!(item_use(&state, actor).is_none());
}

#[test]
fn armor_mass_charges_only_accepted_displacement_against_shared_actor_stamina() {
    let (mut state, player, npc) = fixture();
    let armor = stack(&state, "example:mail");
    state.players.get_mut(&player).unwrap().inventory.slots[38] = armor.clone();
    let npc_slot = state.entities.slot_of(npc).unwrap();
    Arc::make_mut(&mut state.entities.row_mut(npc_slot).appearance).equipment[4] = Some(armor);
    for actor in [state.entities.slot_of(player).unwrap(), npc_slot] {
        let before = state.plugins.clone();
        accepted_movement(&mut state, actor, 0.0);
        accepted_movement(&mut state, actor, f64::NAN);
        assert_eq!(state.plugins, before);
        accepted_movement(&mut state, actor, 2.0);
        close(
            crate::physical_attacks::stamina(&state, actor.0 as usize, 100.0, 0.5),
            84.0,
        );
        assert!(crate::physical_attacks::spend_stamina(
            &mut state,
            actor.0 as usize,
            100.0,
            0.5,
            12.0
        ));
        accepted_movement(&mut state, actor, 100.0);
        close(
            crate::physical_attacks::stamina(&state, actor.0 as usize, 100.0, 0.5),
            0.0,
        );
    }
    state.tick += 2;
    close(
        crate::physical_attacks::stamina(&state, npc_slot.0 as usize, 100.0, 0.5),
        1.0,
    );
}

#[test]
fn restored_guard_rejects_foreign_profiles_future_activation_and_invalid_status() {
    let (mut state, player, _) = fixture();
    let shield = stack(&state, "example:shield");
    state.players.get_mut(&player).unwrap().inventory.slots[40] = shield.clone();
    let actor = state.entities.slot_of(player).unwrap();
    assert!(set_item_use(&mut state, actor, Hand::Off, true));
    assert!(validate_restore(&state).is_ok());
    let valid = state
        .plugins
        .get_domain(OWNER, &key(&state, actor))
        .unwrap()
        .clone();
    for (field, value) in [
        ("slot", serde_json::json!(2)),
        ("started_tick", serde_json::json!(state.tick + 1)),
        ("unknown_status", serde_json::json!(true)),
    ] {
        let mut malformed = valid.clone();
        malformed["active"][field] = value;
        state
            .plugins
            .set_domain(OWNER, &key(&state, actor), malformed);
        assert!(validate_restore(&state).is_err());
    }
    let mut foreign = valid.clone();
    foreign["active"]["stack"]["item_id"] =
        serde_json::json!(state.registry.items.id_of(&id("minecraft:stick")).unwrap());
    state
        .plugins
        .set_domain(OWNER, &key(&state, actor), foreign);
    assert!(validate_restore(&state).is_err());
    let mut disabled_active = valid.clone();
    disabled_active["disabled_until"] = serde_json::json!(state.tick + 30);
    state
        .plugins
        .set_domain(OWNER, &key(&state, actor), disabled_active);
    assert!(validate_restore(&state).is_err());
    save(
        &mut state,
        actor,
        &DefenseState {
            active: None,
            disabled_until: 30,
        },
    );
    assert!(validate_restore(&state).is_ok());
}

#[test]
fn authored_shield_width_swept_radius_offsets_and_modes_are_real_mechanics() {
    let mut profile = pavise();
    let active = ShieldActivation {
        slot: 0,
        started_tick: 0,
        disabled_until: 0,
    };
    let plan = |profile: &PhysicalItemProfile, hit| {
        plan_physical_defense(
            pose(),
            hit,
            &[DefenseGear {
                slot: 0,
                profile,
                damage: 0,
                max_damage: 360,
            }],
            Some(active),
            100.0,
            8,
        )
        .unwrap()
    };
    let mut edge = impact(DamageKind::Pierce);
    edge.origin[0] = 0.55;
    edge.point[0] = 0.55;
    assert!(!plan(&profile, edge).shield_blocked);
    edge.collision_radius = 0.12;
    assert!(plan(&profile, edge).shield_blocked);
    profile.shield.as_mut().unwrap().bottom_offset_blocks = 1.5;
    assert!(!plan(&profile, impact(DamageKind::Pierce)).shield_blocked);
    profile.shield.as_mut().unwrap().bottom_offset_blocks = 0.0;
    profile.shield.as_mut().unwrap().forward_offset_blocks = 4.0;
    assert!(!plan(&profile, impact(DamageKind::Pierce)).shield_blocked);
    profile.shield.as_mut().unwrap().forward_offset_blocks = 0.55;
    profile.shield.as_mut().unwrap().manual = false;
    assert!(!plan(&profile, impact(DamageKind::Pierce)).shield_blocked);
    profile.shield.as_mut().unwrap().deployable = false;
    assert!(
        plan_deployed_shield_impact(
            profile.shield.as_ref().unwrap(),
            DeployedShieldPose {
                position: [0.0; 3],
                yaw: 0.0
            },
            impact(DamageKind::Pierce),
            0,
            360,
            active,
            8
        )
        .is_none()
    );
}

#[test]
fn accepted_manual_block_breaks_actual_mainhand_stack_and_releases_activation() {
    let (mut state, player, _) = fixture();
    let mut shield = stack(&state, "example:shield");
    shield.damage = Some(359);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = shield;
    let actor = state.entities.slot_of(player).unwrap();
    state.entities.set_rot(actor, 0.0, 0.0);
    assert!(set_item_use(&mut state, actor, Hand::Main, true));
    state.tick += 8;
    let mut hit = piece(&state, actor, DamageKind::Pierce);
    assert!(block_damage(&mut state, actor, &mut hit));
    close(hit.amount, 0.0);
    assert!(state.players[&player].inventory.slots[0].is_empty());
    assert!(item_use(&state, actor).is_none());
    close(
        crate::physical_attacks::stamina(&state, actor.0 as usize, 100.0, 0.5),
        86.0,
    );
    assert!(state.appearance_changed.contains(&player));
}

#[test]
fn unarmored_npc_movement_does_not_enter_physical_stamina_custody() {
    let (mut state, _, npc) = fixture();
    let actor = state.entities.slot_of(npc).unwrap();
    Arc::make_mut(&mut state.entities.row_mut(actor).appearance).equipment = Default::default();
    let before = state.plugins.clone();
    accepted_movement(&mut state, actor, 2.0);
    assert_eq!(state.plugins, before);
    let shield = stack(&state, "example:shield");
    Arc::make_mut(&mut state.entities.row_mut(actor).appearance).equipment[1] = Some(shield);
    accepted_movement(&mut state, actor, 2.0);
    assert_eq!(
        state.plugins, before,
        "hand gear alone has no armor movement mass"
    );
}
