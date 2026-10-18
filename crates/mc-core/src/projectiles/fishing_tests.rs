use super::*;
use mc_data::Identifier;

fn rod(state: &TickState) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:fishing_rod").unwrap())
            .unwrap(),
        1,
    )
}

fn observe_fishing(state: &mut TickState) {
    let mut tree = mc_data::advancements::AdvancementTree::default();
    tree.definitions.insert(
        "test:catch".into(),
        serde_json::from_value(serde_json::json!({
            "criteria": { "catch": { "trigger": "minecraft:fishing_rod_hooked" } },
            "requirements": [["catch"]],
        }))
        .unwrap(),
    );
    tree.trigger_index.insert(
        "minecraft:fishing_rod_hooked".into(),
        vec![("test:catch".into(), "catch".into())],
    );
    Arc::get_mut(&mut state.registry).unwrap().advancements = Arc::new(tree);
}

#[test]
fn closed_water_never_rolls_treasure_and_open_water_uses_treasure_functions() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let items = &registry.items;
    let mut treasure = false;
    let mut enchanted = false;
    for seed in 0..512 {
        let closed = loot(&registry, 3, false, false, seed).unwrap();
        let name = items.name_of(closed.item_id).unwrap().path();
        assert!(!matches!(
            name,
            "name_tag" | "saddle" | "bow" | "enchanted_book" | "book" | "nautilus_shell"
        ));
        let open = loot(&registry, 3, true, false, seed).unwrap();
        let name = items.name_of(open.item_id).unwrap().path();
        treasure |= matches!(name, "name_tag" | "saddle" | "nautilus_shell");
        enchanted |=
            !open.enchantments.is_empty() || !open.extended().stored_enchantments.is_empty();
    }
    assert!(treasure && enchanted);
}

#[test]
fn open_water_checks_all_four_layers_and_rejects_flowing_or_mixed_cells() {
    let (mut state, _, pos) = super::super::flight_tests::setup();
    for dy in -1..=2 {
        for dx in -2..=2 {
            for dz in -2..=2 {
                let block = if dy <= 0 {
                    state.registry.states.water
                } else {
                    state.registry.states.air
                };
                state.world.set_block(
                    pos[0] as i32 + dx,
                    pos[1] as i32 + dy,
                    pos[2] as i32 + dz,
                    block,
                );
            }
        }
    }
    assert!(open_water(&state, pos));
    state.world.set_block(
        pos[0] as i32 + 2,
        pos[1] as i32,
        pos[2] as i32,
        state.registry.states.water_levels[1],
    );
    assert!(!open_water(&state, pos));
    state.world.set_block(
        pos[0] as i32 + 2,
        pos[1] as i32,
        pos[2] as i32,
        state.registry.states.air,
    );
    assert!(!open_water(&state, pos));
    let sign = state
        .registry
        .blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:oak_sign").unwrap(),
            &[
                ("rotation".into(), "0".into()),
                ("waterlogged".into(), "true".into()),
            ],
        )
        .unwrap()
        .0;
    state
        .world
        .set_block(pos[0] as i32 + 2, pos[1] as i32, pos[2] as i32, sign);
    assert!(
        open_water(&state, pos),
        "collisionless waterlogged blocks count as source water"
    );
    state.world.set_block(
        pos[0] as i32 + 2,
        pos[1] as i32 + 1,
        pos[2] as i32,
        state.registry.states.stone,
    );
    assert!(
        !open_water(&state, pos),
        "enclosed surface cannot produce treasure"
    );
}

#[test]
fn cast_bite_reel_transfers_loot_once_and_removes_the_retained_hook() {
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    observe_fishing(&mut state);
    let rod = rod(&state);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = rod;
    assert!(use_item(&mut state, actor, Hand::Main));
    assert!(
        state.advancement_events.is_empty(),
        "casting is not a catch"
    );
    let id = state
        .projectiles
        .spawned
        .iter()
        .copied()
        .find(|id| {
            state
                .entities
                .slot_of(*id)
                .is_some_and(|slot| state.entities.row(slot).projectile.is_some())
        })
        .unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, [pos[0] + 1.0, pos[1], pos[2]]);
    state.world.set_block(
        pos[0] as i32 + 1,
        pos[1] as i32,
        pos[2] as i32,
        state.registry.states.water,
    );
    let mut record = *state.entities.row_mut(slot).projectile.take().unwrap();
    let ProjectileData::Fishing(fishing) = &mut record.data else {
        panic!("fishing state")
    };
    fishing.delay = 1;
    assert!(step(&mut state, &mut Vec::new(), id, &mut record));
    state.tick += 1;
    assert!(step(&mut state, &mut Vec::new(), id, &mut record));
    let ProjectileData::Fishing(fishing) = &record.data else {
        panic!("fishing state")
    };
    assert!(fishing.bite_until.is_some());
    state.entities.row_mut(slot).projectile = Some(Box::new(record));
    let before = state.entities.len();
    use_item(&mut state, actor, Hand::Main);
    assert!(state.entities.slot_of(id).is_none());
    assert!(state.entities.len() > before - 1);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
    assert_eq!(state.advancement_events.len(), 1);
    assert_eq!(
        *state.players[&actor].advancement_progress,
        serde_json::json!({})
    );
    let drops = state
        .entities
        .slots()
        .filter(|slot| {
            matches!(
                state.entities.body(*slot).kind,
                crate::entities::EntityKind::Item { .. }
            )
        })
        .count();
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(
        state.advancement_events.len(),
        1,
        "a fresh cast cannot replay the catch"
    );
    assert_eq!(
        state
            .entities
            .slots()
            .filter(|slot| matches!(
                state.entities.body(*slot).kind,
                crate::entities::EntityKind::Item { .. }
            ))
            .count(),
        drops
    );
}

#[test]
fn hooked_target_reels_with_impulse_and_costs_five_durability() {
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    let rod = rod(&state);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = rod;
    use_item(&mut state, actor, Hand::Main);
    let id = *state.projectiles.spawned.last().unwrap();
    let cow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let target = state.spawn_mob(cow, [pos[0] + 4.0, pos[1], pos[2]], 20.0);
    let hook_slot = state.entities.slot_of(id).unwrap();
    let mut record = state.entities.row_mut(hook_slot).projectile.take().unwrap();
    impact(&mut state, id, &mut record, Some(target), pos);
    state.entities.row_mut(hook_slot).projectile = Some(record);
    use_item(&mut state, actor, Hand::Main);
    assert!(state.entities.vel(state.entities.slot_of(target).unwrap())[0] < 0.0);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(5));
    assert!(state.entities.slot_of(id).is_none());
    assert_eq!(
        state.players[&actor]
            .statistics
            .get_custom(mc_data::statistics::CustomStat::FishCaught),
        0
    );
}

#[test]
fn orphaned_hooks_are_removed_without_waiting_for_the_age_cap() {
    for age in [0, 1201] {
        for removed_owner in [false, true] {
            let (mut state, actor, _) = super::super::flight_tests::setup();
            let rod = rod(&state);
            state.players.get_mut(&actor).unwrap().inventory.slots[0] = rod;
            assert!(use_item(&mut state, actor, Hand::Main));
            let id = *state.projectiles.spawned.last().unwrap();
            let slot = state.entities.slot_of(id).unwrap();
            let mut record = *state.entities.row_mut(slot).projectile.take().unwrap();
            record.age = age;
            if removed_owner {
                let slot = state.entities.slot_of(actor).unwrap();
                state.remove_entity(slot);
            } else {
                record.owner = None;
            }
            assert!(step(&mut state, &mut Vec::new(), id, &mut record));
            assert!(
                state.entities.slot_of(id).is_none(),
                "age={age} removed_owner={removed_owner}"
            );
            assert!(state.projectiles.removed.contains(&id));
        }
    }
}

#[test]
fn active_luck_and_unluck_change_the_actual_reel_loot_context() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let seed = (0..4096)
        .find(|seed| {
            loot(&state.registry, 3, false, false, *seed)
                != loot(&state.registry, -3, false, false, *seed)
        })
        .unwrap();
    for (effect, amplifier, expected_luck) in [
        (crate::effects::EffectId::Luck, 1, 3),
        (crate::effects::EffectId::Unluck, 3, -3),
    ] {
        state
            .players
            .get_mut(&actor)
            .unwrap()
            .active_effects
            .clear(None);
        state
            .players
            .get_mut(&actor)
            .unwrap()
            .active_effects
            .add(crate::effects::ActiveEffect::new(effect, 100, amplifier));
        let mut stack = rod(&state);
        stack.enchantments.push(mc_data::ItemEnchantment {
            id: Identifier::parse("minecraft:luck_of_the_sea").unwrap(),
            level: 1,
        });
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack.clone();
        assert!(use_item(&mut state, actor, Hand::Main));
        let hook = *state.projectiles.spawned.last().unwrap();
        let slot = state.entities.slot_of(hook).unwrap();
        state.entities.row_mut(slot).uuid = uuid::Uuid::from_u128(u128::from(seed ^ state.tick));
        let record = state.entities.row_mut(slot).projectile.as_mut().unwrap();
        let ProjectileData::Fishing(fishing) = &mut record.data else {
            panic!("hook");
        };
        fishing.bite_until = Some(state.tick + 10);
        let expected = loot(&state.registry, expected_luck, false, false, seed).unwrap();
        let before = state.projectiles.spawned.len();
        reel(&mut state, actor, 0, &stack, hook);
        let actual = state.projectiles.spawned[before..]
            .iter()
            .find_map(|id| {
                state
                    .entities
                    .slot_of(*id)
                    .and_then(|slot| state.entities.row(slot).item.as_ref())
            })
            .unwrap();
        assert_eq!(actual.full_stack(), expected, "{effect:?}");
    }
}

#[test]
fn fishing_reel_from_source_pond_reaches_bank_and_is_picked_up() {
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    let held = rod(&state);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = held.clone();
    // Two source-water layers below the bank's feet, with the hook four blocks
    // out and .6 below those feet, matching the real-client receipt.
    for x in -4..=4 {
        for z in 1..=8 {
            for y in 199..=200 {
                state.world.set_block(
                    pos[0] as i32 + x,
                    y,
                    pos[2] as i32 + z,
                    state.registry.states.water,
                );
            }
            state.world.set_block(
                pos[0] as i32 + x,
                198,
                pos[2] as i32 + z,
                state.registry.states.stone,
            );
        }
    }
    state.world.fold_overlay();
    assert!(use_item(&mut state, actor, Hand::Main));
    let hook = *state.projectiles.spawned.last().unwrap();
    let hook_slot = state.entities.slot_of(hook).unwrap();
    let hook_pos = [pos[0], pos[1] - 0.6, pos[2] + 4.0];
    state.entities.set_pos(hook_slot, hook_pos);
    state.entities.set_vel(hook_slot, [0.0; 3]);
    let mut record = state.entities.row_mut(hook_slot).projectile.take().unwrap();
    let ProjectileData::Fishing(fishing) = &mut record.data else {
        panic!("fishing hook")
    };
    fishing.delay = 1;
    assert!(step(&mut state, &mut Vec::new(), hook, &mut record));
    state.tick += 1;
    assert!(step(&mut state, &mut Vec::new(), hook, &mut record));
    let ProjectileData::Fishing(fishing) = &record.data else {
        panic!("fishing hook")
    };
    assert!(fishing.bite_until.is_some(), "actual source-water bite");
    state.entities.row_mut(hook_slot).projectile = Some(record);
    let first_drop = state.projectiles.spawned.len();
    assert!(use_item(&mut state, actor, Hand::Main));
    assert!(state.entities.slot_of(hook).is_none());
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
    let drop = state.projectiles.spawned[first_drop..]
        .iter()
        .copied()
        .find(|id| {
            state
                .entities
                .slot_of(*id)
                .is_some_and(|slot| state.entities.row(slot).item.is_some())
        })
        .unwrap();
    let slot = state.entities.slot_of(drop).unwrap();
    let catch = state.entities.row(slot).item.as_ref().unwrap().full_stack();
    assert!(state.entities.vel(slot)[2] < -7.0);
    let first = crate::tick::run_tick(&mut state, &[]).0;
    let image = first
        .snapshot
        .entities
        .iter()
        .find(|view| view.id == drop)
        .unwrap();
    assert!(
        image.position[2] < hook_pos[2],
        "published item must move toward the bank"
    );
    for _ in 0..29 {
        crate::tick::run_tick(&mut state, &[]);
        if state.entities.slot_of(drop).is_none() {
            break;
        }
    }
    assert!(
        state.entities.slot_of(drop).is_none(),
        "catch must reach the bank and be picked up within thirty ticks"
    );
    assert_eq!(
        state.players[&actor]
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == catch.item_id)
            .map(|stack| stack.count)
            .sum::<i32>(),
        catch.count,
    );
}

#[test]
fn lure_reduces_each_seeded_wait_by_five_seconds_per_level() {
    for seed in 0..512 {
        let ordinary = FishingState::new(0, seed).delay;
        assert!((100..=600).contains(&ordinary));
        for lure in 1..=3 {
            assert_eq!(
                FishingState::new(lure, seed).delay,
                ordinary.saturating_sub(u64::from(lure) * 100).max(1)
            );
        }
    }
}

#[test]
fn luck_of_the_sea_shifts_vanilla_junk_into_treasure() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let mut junk = [0; 2];
    let mut treasure = [0; 2];
    for seed in 0..4096 {
        for (index, luck) in [0, 3].into_iter().enumerate() {
            let stack = loot(&registry, luck, true, false, seed).unwrap();
            let name = registry.items.name_of(stack.item_id).unwrap().path();
            if matches!(
                name,
                "name_tag" | "saddle" | "bow" | "enchanted_book" | "nautilus_shell"
            ) || name == "fishing_rod" && !stack.enchantments.is_empty()
            {
                treasure[index] += 1;
            } else if !matches!(name, "cod" | "salmon" | "tropical_fish" | "pufferfish") {
                junk[index] += 1;
            }
        }
    }
    assert!(junk[1] < junk[0] / 2, "{junk:?}");
    assert!(treasure[1] > treasure[0] * 3 / 2, "{treasure:?}");
}

#[test]
fn vanilla_catches_preserve_damage_potions_counts_and_stored_enchantments() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..8192 {
        let stack = loot(&registry, 0, true, true, seed).unwrap();
        let name = registry.items.name_of(stack.item_id).unwrap();
        seen.insert(name.path().to_owned());
        match name.path() {
            "bow" | "fishing_rod" | "leather_boots" => {
                let max = registry.item_facts.get(name).unwrap().max_damage.unwrap() as i32;
                let damage = stack.damage.unwrap();
                let treasure = !stack.enchantments.is_empty();
                let minimum = if treasure { 0.75 } else { 0.1 };
                assert!((f64::from(max) * minimum).floor() as i32 <= damage);
                assert!(damage <= max);
                assert!(stack.extended().stored_enchantments.is_empty());
            }
            "enchanted_book" => {
                assert!(stack.enchantments.is_empty());
                assert!(!stack.extended().stored_enchantments.is_empty());
            }
            "potion" => assert_eq!(stack.extended().potion.as_ref().unwrap().path(), "water"),
            "ink_sac" => assert_eq!(stack.count, 10),
            "book" => panic!("loot enchanting must convert book to enchanted_book"),
            _ => {}
        }
    }
    for name in [
        "bow",
        "fishing_rod",
        "enchanted_book",
        "potion",
        "ink_sac",
        "bamboo",
        "cod",
        "salmon",
        "tropical_fish",
        "pufferfish",
    ] {
        assert!(seen.contains(name), "missing vanilla catch {name}");
    }
}

fn bite_wait(weather: crate::commands::Weather, roof: bool) -> (u64, ProjectileState) {
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    let held = rod(&state);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = held;
    use_item(&mut state, actor, Hand::Main);
    let id = *state.projectiles.spawned.last().unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).uuid = Uuid::from_u128(42);
    state.entities.set_pos(slot, pos);
    let [x, y, z] = pos.map(|v| v.floor() as i32);
    state.world.set_block(x, y, z, state.registry.states.water);
    for above in y + 1..state.dimension.geometry().max_y() {
        state
            .world
            .set_block(x, above, z, state.registry.states.air);
    }
    if roof {
        state
            .world
            .set_block(x, y + 3, z, state.registry.states.stone);
    }
    state.weather.0 = weather;
    let mut record = *state.entities.row_mut(slot).projectile.take().unwrap();
    let ProjectileData::Fishing(fishing) = &mut record.data else {
        panic!("hook")
    };
    fishing.delay = 400;
    for tick in 0..=400 {
        state.tick = tick;
        step(&mut state, &mut Vec::new(), id, &mut record);
        let ProjectileData::Fishing(fishing) = &record.data else {
            panic!("hook")
        };
        if fishing.bite_until.is_some() {
            return (tick, record);
        }
    }
    panic!("no bite");
}

#[test]
fn exposed_rain_accelerates_bites_but_a_roof_does_not() {
    let dry = bite_wait(crate::commands::Weather::Clear, false).0;
    let wet = bite_wait(crate::commands::Weather::Rain, false).0;
    let covered = bite_wait(crate::commands::Weather::Rain, true).0;
    assert_eq!(dry, 400);
    assert_eq!(covered, dry);
    assert!((280..360).contains(&wet), "rain bite at {wet}");
}

#[test]
fn rain_adjusted_wait_survives_retained_entity_nbt() {
    let (_, record) = bite_wait(crate::commands::Weather::Rain, false);
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let fields = super::super::persistence::to_fields(&record, &registry.items);
    let entity = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:fishing_bobber".into(),
        uuid: Uuid::from_u128(42),
        pos: [0.5, 200.0, 0.5],
        extra_fields: fields,
        ..Default::default()
    };
    assert_eq!(
        super::super::persistence::from_entity(&entity, &registry.items).unwrap(),
        record
    );
}

#[test]
fn rain_wait_is_journaled_after_native_checkpoint_and_survives_anvil_export() {
    use crate::dimensions::Dimension;
    use crate::journal::JournalWriter;
    use crate::native_checkpoint::{encode_entities, record, refresh_metadata};
    use mc_world::native::NativeCheckpoint;

    let directory = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(directory.path().to_path_buf(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    let held = rod(&state);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = held;
    use_item(&mut state, actor, Hand::Main);
    let id = *state.projectiles.spawned.last().unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, pos);
    state.entities.row_mut(slot).uuid = Uuid::from_u128(42);
    let ProjectileData::Fishing(fishing) = &mut state
        .entities
        .row_mut(slot)
        .projectile
        .as_mut()
        .unwrap()
        .data
    else {
        panic!("hook")
    };
    fishing.delay = 400;
    fishing.water_since = Some(0);
    let [x, y, z] = pos.map(|v| v.floor() as i32);
    state.world.set_block(x, y, z, state.registry.states.water);
    for above in y + 1..state.dimension.geometry().max_y() {
        state
            .world
            .set_block(x, above, z, state.registry.states.air);
    }
    state.weather.0 = crate::commands::Weather::Rain;
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let checkpoint_tick = state.tick;
    let (_, captured, _) = crate::core::checkpoint_data_for(&state, checkpoint_tick);
    let entities = encode_entities(
        Dimension::Overworld,
        &captured,
        &state.registry.items,
        checkpoint_tick,
    )
    .unwrap();
    let mut image = store.read_checkpoint().unwrap();
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.tick = checkpoint_tick;
    for child in manifest.dimensions.values_mut() {
        child.tick = checkpoint_tick;
    }
    image.globals.insert(
        "checkpoint".into(),
        record(checkpoint_tick, serde_json::to_vec(&manifest).unwrap()),
    );
    refresh_metadata(&mut image.globals, &manifest).unwrap();
    for global in image.globals.values_mut() {
        global.captured_tick = checkpoint_tick;
    }
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: checkpoint_tick,
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
    drop(store);
    // Clear the birth image; subsequent entries must be caused by flight ticks.
    crate::core::journal_entries_for_tick(&mut state, &output);
    let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
    for _ in 0..20 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        assert!(entries.iter().any(|entry| matches!(entry, crate::journal::JournalEntry::Entity { id: journal_id, .. } if *journal_id == id.0)));
        writer.append(state.tick, state.tick, &entries).unwrap();
    }
    writer.flush().unwrap();
    drop(writer);
    let expected = state
        .entities
        .row(slot)
        .projectile
        .as_deref()
        .cloned()
        .unwrap();
    let ProjectileData::Fishing(fishing) = &expected.data else {
        panic!("hook")
    };
    assert!(fishing.delay < 398);
    let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
    let native = &recovered.dimensions[&Dimension::Overworld].entities[&id.0];
    assert_eq!(native.projectile.as_ref(), Some(&expected));
    let (_, exported, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let anvil = directory.path().join("anvil-entities");
    mc_world::anvil::write_entity_regions(
        &anvil,
        &state.registry.items,
        &exported,
        state.tick.try_into().unwrap(),
    )
    .unwrap();
    let imported = mc_world::anvil::read_entity_regions(&anvil, &state.registry.items).unwrap();
    let hook = imported
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == Uuid::from_u128(42))
        .unwrap();
    assert_eq!(
        super::super::persistence::from_entity(hook, &state.registry.items).unwrap(),
        expected
    );
}

#[test]
fn fish_caught_counts_fish_loot_not_junk_or_empty_reels() {
    use mc_data::statistics::CustomStat;
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    for wanted_fish in [true, false] {
        let held = rod(&state);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = held.clone();
        assert!(use_item(&mut state, actor, Hand::Main));
        let id = *state.projectiles.spawned.last().unwrap();
        let slot = state.entities.slot_of(id).unwrap();
        let uuid_seed = state.entities.row(slot).uuid.as_u128() as u64;
        let seed = (0..4096)
            .find(|seed| {
                if (uuid_seed ^ seed) == u64::MAX {
                    return false;
                }
                let item = loot(&state.registry, 0, false, false, *seed).unwrap();
                let name = state.registry.items.name_of(item.item_id).unwrap().path();
                matches!(name, "cod" | "salmon" | "pufferfish" | "tropical_fish") == wanted_fish
            })
            .expect("both fish and junk occur in the fishing table");
        state.tick = uuid_seed ^ seed;
        state.entities.set_pos(slot, pos);
        let row = state.entities.row_mut(slot);
        let ProjectileData::Fishing(fishing) = &mut row.projectile.as_mut().unwrap().data else {
            panic!("fishing state")
        };
        fishing.bite_until = Some(state.tick + 1);
        reel(&mut state, actor, 0, &held, id);
        assert_eq!(
            state.players[&actor]
                .statistics
                .get_custom(CustomStat::FishCaught),
            1
        );
    }
    let held = rod(&state);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = held.clone();
    assert!(use_item(&mut state, actor, Hand::Main));
    let id = *state.projectiles.spawned.last().unwrap();
    reel(&mut state, actor, 0, &held, id);
    assert_eq!(
        state.players[&actor]
            .statistics
            .get_custom(CustomStat::FishCaught),
        1
    );
}

#[test]
fn fishing_rod_use_stat_counts_accepted_casts_not_reels_or_refusals() {
    use mc_data::statistics::StatType;
    for mode in [
        crate::commands::GameMode::Survival,
        crate::commands::GameMode::Creative,
    ] {
        let (mut state, actor, _) = super::super::flight_tests::setup();
        let held = rod(&state);
        let item = held.item_id;
        let player = state.players.get_mut(&actor).unwrap();
        player.game_mode = mode;
        player.inventory.slots[0] = ItemStack::EMPTY;
        assert!(!use_item(&mut state, actor, Hand::Main));
        assert_eq!(
            state.players[&actor].statistics.get(StatType::Used, item),
            0
        );
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = held;
        assert!(use_item(&mut state, actor, Hand::Main));
        assert_eq!(
            state.players[&actor].statistics.get(StatType::Used, item),
            1
        );
        assert!(use_item(&mut state, actor, Hand::Main));
        assert_eq!(
            state.players[&actor].statistics.get(StatType::Used, item),
            1
        );
        assert!(use_item(&mut state, actor, Hand::Main));
        assert_eq!(
            state.players[&actor].statistics.get(StatType::Used, item),
            2
        );
        assert!(use_item(&mut state, actor, Hand::Main));
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
        assert!(!use_item(&mut state, actor, Hand::Main));
        assert_eq!(
            state.players[&actor].statistics.get(StatType::Used, item),
            2
        );
    }
}

#[test]
fn reeling_without_bite_or_hooked_entity_emits_no_catch_event() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    observe_fishing(&mut state);
    let rod = rod(&state);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = rod;
    assert!(use_item(&mut state, actor, Hand::Main));
    assert!(use_item(&mut state, actor, Hand::Main));
    assert!(state.advancement_events.is_empty());
}
