use super::{flat_player_fixture, run_tick};
use crate::boundary::{ConnectionId, EntityId, PlayerInput};
use crate::combat::{mace_attack, smash_damage};
use crate::effects::EffectId;
use crate::tick::TickState;
use mc_data::{Identifier, ItemEnchantment, ItemStack};

fn held(state: &TickState, enchants: &[(&str, i32)]) -> ItemStack {
    let mut held = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:mace").unwrap())
            .unwrap(),
        1,
    );
    held.enchantments = enchants
        .iter()
        .map(|(name, level)| ItemEnchantment {
            id: Identifier::parse(format!("minecraft:{name}")).unwrap(),
            level: *level,
        })
        .collect();
    held
}
fn cow(state: &mut TickState, pos: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    state.spawn_mob(type_id, pos, 100.0)
}
fn attack(
    height: f64,
    enchants: &[(&str, i32)],
    protected: bool,
) -> (TickState, EntityId, EntityId, EntityId) {
    let (mut state, player, pos) = flat_player_fixture();
    state.tick = 100;
    let mace = held(&state, enchants);
    let p = state.players.get_mut(&player).unwrap();
    p.inventory.slots[0] = mace;
    p.fall_distance = height;
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_ground(slot, false);
    let target = cow(&mut state, [pos[0], pos[1], pos[2] + 1.0]);
    let nearby = cow(&mut state, [pos[0] + 1.0, pos[1], pos[2] + 1.0]);
    if protected {
        let slot = state.entities.slot_of(target).unwrap();
        state.entities.row_mut(slot).protected_until = 200;
    }
    run_tick(
        &mut state,
        &[PlayerInput::Attack {
            connection: ConnectionId(1),
            target,
        }],
    );
    (state, player, target, nearby)
}

#[test]
fn mace_smash_threshold_height_tiers_and_density_are_authored_values() {
    let (state, _, _) = flat_player_fixture();
    let plain = held(&state, &[]);
    let dense = held(&state, &[("density", 5)]);
    for (height, damage) in [
        (1.5, 0.0),
        (2.0, 8.0),
        (3.0, 12.0),
        (5.0, 16.0),
        (8.0, 22.0),
        (10.0, 24.0),
    ] {
        let attack = mace_attack(&state.registry.item_rules(), &plain, height, false).unwrap();
        assert_eq!(smash_damage(&plain, attack), damage);
    }
    let attack = mace_attack(&state.registry.item_rules(), &dense, 5.0, false).unwrap();
    assert_eq!(smash_damage(&dense, attack), 28.5);
    let flying = mace_attack(&state.registry.item_rules(), &dense, 50.0, true).unwrap();
    assert_eq!(smash_damage(&dense, flying), 0.0);
}

#[test]
fn mace_smash_grows_with_height_resets_fall_and_pushes_only_nearby_bystanders() {
    let (standing, _, a, _) = attack(0.0, &[], false);
    let (mut falling, player, b, nearby) = attack(5.0, &[], false);
    assert!(
        falling
            .entities
            .row(falling.entities.slot_of(b).unwrap())
            .health
            < standing
                .entities
                .row(standing.entities.slot_of(a).unwrap())
                .health
    );
    assert_eq!(falling.players[&player].fall_distance, 0.0);
    assert!(
        falling
            .entities
            .vel(falling.entities.slot_of(nearby).unwrap())[1]
            > 0.0
    );
    let p = falling.players.get_mut(&player).unwrap();
    p.fall(-20.0, false, false);
    assert_eq!(p.fall(0.0, true, false), 0.0);
    p.fall(-20.0, false, false);
    assert!(p.fall(0.0, true, false) > 0.0);
}

#[test]
fn mace_rejected_hit_never_cancels_fall_or_emits_a_wind_burst() {
    let (state, player, _, nearby) = attack(5.0, &[("wind_burst", 1)], true);
    assert_eq!(state.players[&player].fall_distance, 5.0);
    assert!(state.players[&player].current_impulse_impact_pos.is_none());
    assert_eq!(
        state.entities.vel(state.entities.slot_of(nearby).unwrap())[1],
        0.0
    );
}

#[test]
fn mace_wind_burst_level_increases_the_attacker_bounce() {
    let (a, first, _, _) = attack(5.0, &[("wind_burst", 1)], false);
    let (b, third, _, _) = attack(5.0, &[("wind_burst", 3)], false);
    let low = a.entities.vel(a.entities.slot_of(first).unwrap())[1];
    let high = b.entities.vel(b.entities.slot_of(third).unwrap())[1];
    assert!(low > 0.2 && high > low, "{low} -> {high}");
}

#[test]
fn mace_breach_reduces_armor_fraction_without_bypassing_protection() {
    let (state, _, _) = flat_player_fixture();
    let mut armor = vec![ItemStack::EMPTY; 4];
    for (slot, name) in [
        "diamond_boots",
        "diamond_leggings",
        "diamond_chestplate",
        "diamond_helmet",
    ]
    .into_iter()
    .enumerate()
    {
        armor[slot] = ItemStack::new(
            state
                .registry
                .items
                .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
                .unwrap(),
            1,
        );
        armor[slot].enchantments.push(ItemEnchantment {
            id: Identifier::parse("minecraft:protection").unwrap(),
            level: 4,
        });
    }
    let rules = state.registry.item_rules();
    let normal = crate::survival::reduce_damage_with_breach(
        &rules,
        &armor,
        10.0,
        crate::survival::DamageKind::Melee,
        0,
    );
    let breached = crate::survival::reduce_damage_with_breach(
        &rules,
        &armor,
        10.0,
        crate::survival::DamageKind::Melee,
        4,
    );
    assert!(breached > normal && breached < 10.0);
    assert!((breached - normal - 6.0 * (1.0 - 16.0 / 25.0)).abs() < 0.0001);
    assert!(
        !state
            .players
            .values()
            .any(|p| p.active_effects.has(EffectId::Poison))
    );
}

#[test]
fn mace_impulse_round_trips_player_wal_and_native_checkpoint_and_clears_after_landing() {
    let (mut state, player, _, _) = attack(5.0, &[], false);
    let impact = state.players[&player].current_impulse_impact_pos;
    let record = crate::core::player_record_for(&state, player, state.tick).unwrap();
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid
        .to_string();
    let replay = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &[(1, output.tick, entries)],
    );
    assert_eq!(replay.players[&uuid].current_impulse_impact_pos, impact);
    let dir = std::env::temp_dir().join(format!("solaris-mace-{}", uuid::Uuid::new_v4()));
    let config = crate::CoreConfig::new(dir.clone(), 42);
    let read = crate::native_checkpoint::tests::roundtrip_player(
        &config,
        &uuid,
        &record,
        &state.registry.items,
    )
    .unwrap();
    assert_eq!(read.current_impulse_impact_pos, impact);
    let mut recovered = super::test_state();
    let uuid_value = uuid::Uuid::parse_str(&uuid).unwrap();
    recovered.offline_players.insert(uuid_value, read);
    run_tick(
        &mut recovered,
        &[PlayerInput::Join {
            connection: ConnectionId(9),
            uuid: uuid_value,
            name: "mace-rejoin".into(),
            permission_level: 0,
        }],
    );
    let id = recovered.connections.entity_of(ConnectionId(9)).unwrap();
    let restored = recovered.players.get_mut(&id).unwrap();
    restored.fall(-20.0, false, false);
    assert_eq!(restored.fall(0.0, true, false), 0.0);
    assert!(restored.current_impulse_impact_pos.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mace_impulse_corrupt_nbt_and_wal_fail_closed() {
    use mc_nbt::Tag;
    for invalid in [
        Tag::Byte(1),
        Tag::List(mc_nbt::ListTag {
            element_type: 6,
            elements: vec![Tag::Double(0.0)],
        }),
        Tag::List(mc_nbt::ListTag {
            element_type: 6,
            elements: vec![Tag::Double(0.0), Tag::Double(f64::NAN), Tag::Double(0.0)],
        }),
    ] {
        let error = crate::mace::read_impact(&[("current_explosion_impact_pos".into(), invalid)])
            .unwrap_err();
        assert!(["expected three-double list", "coordinate must be finite"].contains(&error));
    }
    let fields = vec![
        (
            "current_explosion_impact_pos".into(),
            Tag::List(mc_nbt::ListTag {
                element_type: 6,
                elements: vec![Tag::Double(0.0); 3]
            })
        );
        2
    ];
    assert_eq!(
        crate::mace::read_impact(&fields).unwrap_err(),
        "duplicate field"
    );
    let mut deserializer = serde_json::Deserializer::from_str("[0,1]");
    assert!(crate::mace::deserialize_impact(&mut deserializer).is_err());
}
