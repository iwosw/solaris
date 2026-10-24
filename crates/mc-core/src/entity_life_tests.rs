//! Native lifecycle behavior, transaction footprints and durable recovery.
use crate::boundary::{ConnectionId, EntityId, Hand, PlayerInput};
use crate::entities::EntityKind;
use crate::entity_life::{self, CHILD_TICKS, EntityLifeState};
use crate::intent::{Footprint, Intent, IntentKind};
use crate::tick::{TickState, run_tick};
use std::sync::Arc;

fn fixture() -> (TickState, EntityId, [f64; 3]) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "farmer".into(),
            permission_level: 0,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(actor).unwrap();
    let old = state.entities.pos(slot);
    let pos = [old[0].floor() + 0.5, old[1].ceil(), old[2].floor() + 0.5];
    let stone = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    for x in (pos[0] as i32 - 8)..=(pos[0] as i32 + 8) {
        for z in (pos[2] as i32 - 8)..=(pos[2] as i32 + 8) {
            state.world.set_block(x, pos[1] as i32 - 1, z, stone);
            for y in 0..8 {
                state
                    .world
                    .set_block(x, pos[1] as i32 + y, z, state.registry.states.air);
            }
        }
    }
    state.world.fold_overlay();
    state.entities.set_pos(slot, pos);
    state.entities.set_ground(slot, true);
    (state, actor, pos)
}
fn item(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
}
fn spawn(state: &mut TickState, name: &str, pos: [f64; 3]) -> EntityId {
    let id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    let health = state.registry.blueprint(id).unwrap().params.max_health;
    state.spawn_mob(id, pos, health)
}
fn use_animal(target: EntityId) -> PlayerInput {
    PlayerInput::InteractEntity {
        connection: ConnectionId(1),
        target,
        hand: Hand::Main,
        location: [0.0; 3],
    }
}
fn row(state: &TickState, id: EntityId) -> &crate::entities::ColdRow {
    state.entities.row(state.entities.slot_of(id).unwrap())
}
fn body(state: &TickState, id: EntityId) -> &crate::entities::Body {
    state.entities.body(state.entities.slot_of(id).unwrap())
}
fn hold(state: &mut TickState, actor: EntityId, name: &str, count: i32) {
    let id = item(state, name);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::new(id, count);
}

#[test]
fn child_is_visible_half_size_cannot_breed_and_feeding_advances_growth() {
    let (mut state, actor, pos) = fixture();
    let cow = spawn(&mut state, "cow", [pos[0], pos[1], pos[2] + 1.0]);
    let original = (body(&state, cow).half_width, body(&state, cow).height);
    entity_life::make_child(&mut state, cow);
    assert!(row(&state, cow).appearance.baby);
    assert_eq!(
        (body(&state, cow).half_width, body(&state, cow).height),
        (original.0 * 0.5, original.1 * 0.5)
    );
    assert!(!entity_life::breed_ready(
        body(&state, cow),
        row(&state, cow)
    ));
    hold(&mut state, actor, "wheat", 2);
    run_tick(&mut state, &[use_animal(cow)]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(row(&state, cow).entity_life.child_ticks < CHILD_TICKS - 2000);
    assert_eq!(row(&state, cow).love, 0);
    let slot = state.entities.slot_of(cow).unwrap();
    state.entities.row_mut(slot).entity_life.child_ticks = 1;
    entity_life::advance(&mut state);
    assert!(!row(&state, cow).appearance.baby);
    assert_eq!(
        (body(&state, cow).half_width, body(&state, cow).height),
        original
    );
    assert!(entity_life::can_feed(body(&state, cow), row(&state, cow)));
}

#[test]
fn one_pair_claim_wins_even_with_three_partners_and_two_duplicate_intents() {
    let (mut state, player, pos) = fixture();
    let a = spawn(&mut state, "rabbit", pos);
    let b = spawn(&mut state, "rabbit", pos);
    let c = spawn(&mut state, "rabbit", pos);
    for id in [a, b, c] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).love = 540;
    }
    let uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let slot = state.entities.slot_of(a).unwrap();
    state.entities.row_mut(slot).entity_life.taming.love_cause = Some(uuid);
    let type_id = match body(&state, a).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    let intent = |partner, seq| Intent {
        actor: a,
        seq,
        kind: IntentKind::Breed {
            partner,
            child_pos: pos,
            child_type: type_id,
        },
        footprint: Footprint {
            entity_reads: vec![a, partner],
            entity_writes: vec![a, partner],
            ..Default::default()
        },
    };
    let mut intents = vec![intent(b, 0), intent(b, 1), intent(c, 2)];
    assert_eq!(state.merge(&mut intents), 1);
    let (_, _, born, _) = state.commit_merge_effects();
    let children: Vec<_> = born
        .into_iter()
        .filter(|id| {
            state.entities.slot_of(*id).is_some() && row(&state, *id).entity_life.child_ticks > 0
        })
        .collect();
    assert_eq!(children.len(), 1);
    assert!(row(&state, children[0]).appearance.baby);
    assert_eq!(row(&state, a).breed_cooldown, 6000);
    assert_eq!(row(&state, b).breed_cooldown, 6000);
    assert_eq!(row(&state, c).love, 540);
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(mc_data::statistics::CustomStat::AnimalsBred),
        1
    );
}

#[test]
fn broader_food_catalog_feeds_and_breeds_native_species_after_courtship() {
    for (species, food) in [
        ("rabbit", "carrot"),
        ("goat", "wheat"),
        ("fox", "sweet_berries"),
        ("armadillo", "spider_eye"),
        ("hoglin", "crimson_fungus"),
    ] {
        let (mut state, actor, pos) = fixture();
        let a = spawn(&mut state, species, [pos[0], pos[1], pos[2] + 1.0]);
        let b = spawn(&mut state, species, [pos[0] + 1.0, pos[1], pos[2] + 1.0]);
        hold(&mut state, actor, food, 2);
        // Separate actors are unnecessary: inventory claims sequence the same food stack.
        run_tick(&mut state, &[use_animal(a)]);
        run_tick(&mut state, &[use_animal(b)]);
        assert!(
            state.players[&actor].inventory.slots[0].is_empty(),
            "{species}"
        );
        assert_eq!(
            state
                .entities
                .slots()
                .filter(|slot| state.entities.row(*slot).entity_life.child_ticks > 0)
                .count(),
            0,
            "courtship {species}"
        );
        for _ in 0..90 {
            run_tick(&mut state, &[]);
        }
        assert_eq!(
            state
                .entities
                .slots()
                .filter(|slot| state.entities.row(*slot).entity_life.child_ticks > 0)
                .count(),
            1,
            "offspring {species}"
        );
    }
}

#[test]
fn untamed_or_sitting_pets_and_unhealthy_or_mounted_horses_cannot_breed() {
    let (mut state, _, pos) = fixture();
    for species in ["wolf", "cat", "horse", "donkey", "llama"] {
        let id = spawn(&mut state, species, pos);
        assert!(
            !entity_life::can_breed(&state, body(&state, id), row(&state, id)),
            "wild {species}"
        );
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).entity_life.tamed = true;
        assert!(
            entity_life::can_breed(&state, body(&state, id), row(&state, id)),
            "tamed {species}"
        );
        if species == "wolf" {
            state.entities.row_mut(slot).entity_life.sitting = true;
            assert!(
                !entity_life::can_breed(&state, body(&state, id), row(&state, id)),
                "unavailable {species}"
            );
        } else if species == "cat" {
            // Vanilla feeds a sitting cat into love; only a both-sitting
            // pair refuses the birth (see breeding::valid).
            state.entities.row_mut(slot).entity_life.sitting = true;
            assert!(
                entity_life::can_breed(&state, body(&state, id), row(&state, id)),
                "sitting cat still feeds {species}"
            );
        } else {
            state.entities.row_mut(slot).entity_life.mounted = true;
            assert!(
                !entity_life::can_breed(&state, body(&state, id), row(&state, id)),
                "unavailable {species}"
            );
        }
    }
}

#[test]
fn milk_harvest_replaces_one_bucket_and_children_never_supply_milk() {
    for species in ["cow", "goat", "mooshroom"] {
        let (mut state, actor, pos) = fixture();
        let id = spawn(&mut state, species, [pos[0], pos[1], pos[2] + 1.0]);
        hold(&mut state, actor, "bucket", 1);
        run_tick(&mut state, &[use_animal(id), use_animal(id)]);
        assert_eq!(
            state.players[&actor].inventory.slots[0].item_id,
            item(&state, "milk_bucket")
        );
        entity_life::make_child(&mut state, id);
        hold(&mut state, actor, "bucket", 1);
        run_tick(&mut state, &[use_animal(id)]);
        assert_eq!(
            state.players[&actor].inventory.slots[0].item_id,
            item(&state, "bucket")
        );
    }
}

#[test]
fn brown_mooshroom_flower_stew_then_shearing_are_atomic_and_durable() {
    let (mut state, actor, pos) = fixture();
    let id = spawn(&mut state, "mooshroom", [pos[0], pos[1], pos[2] + 1.0]);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).entity_life.brown_mooshroom = true;
    hold(&mut state, actor, "dandelion", 2);
    let (mut output, _, _) = run_tick(&mut state, &[use_animal(id), use_animal(id)]);
    // This test injects the mob directly, outside run_tick's normal spawn path.
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_spawned
        .push(id);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(row(&state, id).entity_life.stew_effects.len(), 1);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|entry| matches!(entry,crate::journal::JournalEntry::Entity { id:record,state,.. } if *record==id.0 && state.entity_life.stew_effects.len()==1)));
    let actor_uuid = state
        .connections
        .uuid_of(ConnectionId(1))
        .unwrap()
        .to_string();
    assert!(entries.iter().any(|entry| matches!(entry, crate::journal::JournalEntry::Player { uuid, inventory, .. } if *uuid == actor_uuid && inventory[0].count == 1)));
    hold(&mut state, actor, "bowl", 1);
    run_tick(&mut state, &[use_animal(id)]);
    let soup = &state.players[&actor].inventory.slots[0];
    assert_eq!(soup.item_id, item(&state, "suspicious_stew"));
    assert_eq!(soup.stew_effects[0].id.as_str(), "minecraft:saturation");
    assert!(row(&state, id).entity_life.stew_effects.is_empty());
    hold(&mut state, actor, "shears", 1);
    run_tick(&mut state, &[use_animal(id), use_animal(id)]);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
    let cow = state.registry.cow_type_id;
    assert_eq!(body(&state, id).kind, EntityKind::Mob { type_id: cow });
    let mushroom = item(&state, "brown_mushroom");
    let dropped: i32 = state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .filter(|stack| stack.item_id == mushroom)
        .map(|stack| stack.count)
        .sum();
    let held: i32 = state.players[&actor]
        .inventory
        .slots
        .iter()
        .filter(|stack| !stack.is_empty() && stack.item_id == mushroom)
        .map(|stack| stack.count)
        .sum();
    assert_eq!(dropped + held, 5);
}

#[test]
fn last_shears_durability_is_spent_once_across_two_animals() {
    let (mut state, actor, pos) = fixture();
    let a = spawn(&mut state, "sheep", [pos[0], pos[1], pos[2] + 1.0]);
    let b = spawn(&mut state, "sheep", [pos[0] + 1.0, pos[1], pos[2] + 1.0]);
    hold(&mut state, actor, "shears", 1);
    let max = state
        .registry
        .item_rules()
        .facts
        .facts_for_stack(
            &state.players[&actor].inventory.slots[0],
            &state.registry.items,
        )
        .unwrap()
        .max_damage
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0].damage = Some(max as i32 - 1);
    run_tick(&mut state, &[use_animal(a), use_animal(b)]);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let sheared = [a, b]
        .into_iter()
        .filter(|id| crate::tick::wool_of(&row(&state, *id).appearance) & 0x10 != 0)
        .count();
    assert_eq!(sheared, 1);
}

#[test]
fn chicken_egg_countdown_lays_once_is_journaled_and_pauses_while_child() {
    let (mut state, _, pos) = fixture();
    let id = spawn(&mut state, "chicken", pos);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).entity_life.harvest_ticks = 1;
    let (output, _, _) = run_tick(&mut state, &[]);
    let egg = item(&state, "egg");
    let eggs: Vec<_> = state
        .entities
        .slots()
        .filter(|slot| {
            state
                .entities
                .row(*slot)
                .item
                .as_ref()
                .is_some_and(|stack| stack.item_id == egg)
        })
        .map(|slot| state.entities.body(slot).id)
        .collect();
    assert_eq!(eggs.len(), 1);
    assert!((6000..=12000).contains(&row(&state, id).entity_life.harvest_ticks));
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|entry|matches!(entry,crate::journal::JournalEntry::Entity { id:record,item:Some(stack),.. } if *record==eggs[0].0&&stack.item_id==egg)));
    let timer = row(&state, id).entity_life.harvest_ticks;
    entity_life::make_child(&mut state, id);
    entity_life::advance(&mut state);
    assert_eq!(row(&state, id).entity_life.harvest_ticks, timer);
    // Growth happens before harvest in the same tick.
    state.entities.row_mut(slot).entity_life.child_ticks = 1;
    state.entities.row_mut(slot).entity_life.harvest_ticks = 1;
    entity_life::advance(&mut state);
    assert!(!row(&state, id).appearance.baby);
    assert!((6000..=12000).contains(&row(&state, id).entity_life.harvest_ticks));
    assert_eq!(state.merge_life_drops.len(), 1);
}

#[test]
fn harvest_only_clocks_leave_motion_hints_clean_until_the_egg_event() {
    let (mut state, _, pos) = fixture();
    // Refresh activation after the fixture's blocking chunk loads.
    run_tick(&mut state, &[]);
    let mut chickens = Vec::new();
    for index in 0..32 {
        let id = spawn(&mut state, "chicken", pos);
        let slot = state.entities.slot_of(id).unwrap();
        let row = state.entities.row_mut(slot);
        row.controls.no_ai = index % 2 == 0;
        row.entity_life.harvest_ticks = 3;
        chickens.push(slot);
    }
    state.entities.prepare_motion_hints(&state.registry);
    let baseline = state.entities.views();
    state.entities.take_touched();
    for expected in [2, 1] {
        entity_life::advance(&mut state);
        assert_eq!(state.entities.dirty_ids_len_for_test(), 0);
        assert!(state.entities.touched_ids().is_empty());
        assert_eq!(state.entities.views().revision(), baseline.revision());
        assert!(state.merge_life_drops.is_empty());
        for &slot in &chickens {
            assert_eq!(state.entities.row(slot).entity_life.harvest_ticks, expected);
        }
    }
    entity_life::advance(&mut state);
    assert_eq!(state.merge_life_drops.len(), chickens.len());
    assert_eq!(state.entities.dirty_ids_len_for_test(), chickens.len());
    assert_eq!(state.entities.touched_ids().len(), chickens.len());
    for &slot in &chickens {
        assert!((6000..=12000).contains(&state.entities.row(slot).entity_life.harvest_ticks));
    }
}

#[test]
fn journal_file_replay_restores_child_cooldown_love_and_harvest_exactly() {
    let (mut state, _, pos) = fixture();
    let id = spawn(&mut state, "chicken", pos);
    entity_life::make_child(&mut state, id);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).entity_life.harvest_ticks = 77;
    let parent = spawn(&mut state, "cow", pos);
    let parent_slot = state.entities.slot_of(parent).unwrap();
    state.entities.row_mut(parent_slot).breed_cooldown = 321;
    state.entities.row_mut(parent_slot).love = 123;
    let (mut output, _, _) = run_tick(&mut state, &[]);
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_spawned
        .extend([id, parent]);
    let first = crate::core::journal_entries_for_tick(&mut state, &output);
    let dir = std::env::temp_dir().join(format!("solaris-life-{}", uuid::Uuid::new_v4()));
    let mut writer = crate::journal::JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, output.tick, &first).unwrap();
    let (second_output, _, _) = run_tick(&mut state, &[]);
    let expected = row(&state, id).entity_life.clone();
    let second = crate::core::journal_entries_for_tick(&mut state, &second_output);
    assert!(!second.iter().any(|entry| matches!(entry, crate::journal::JournalEntry::EntityState { id: record, .. } if *record == parent.0)), "ordinary love and cooldown decrements are derived from the batch clock");
    writer
        .append(second_output.tick, second_output.tick, &second)
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = crate::journal::read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let replay = crate::journal::replay_batches(crate::journal::Replay::default(), &batches);
    let record = &replay.entities[&id.0];
    assert_eq!(record.state.entity_life, expected);
    let (mut restarted, _, _) = fixture();
    crate::core::restore_entity(&mut restarted, id.0, record).unwrap();
    assert_eq!(row(&restarted, id).entity_life, expected);
    assert!(row(&restarted, id).appearance.baby);
    assert!(!entity_life::breed_ready(
        body(&restarted, id),
        row(&restarted, id)
    ));
    assert_eq!(body(&restarted, id).height, body(&state, id).height);
    let parent_image = &replay.entities[&parent.0];
    crate::core::restore_entity(&mut restarted, parent.0, parent_image).unwrap();
    assert_eq!(row(&restarted, parent).breed_cooldown, 319);
    assert_eq!(row(&restarted, parent).love, 121);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn vanilla_nbt_preserves_signed_age_stew_and_retained_prerequisites() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let effect = mc_data::item_stack::StewEffect {
        id: mc_data::Identifier::parse("minecraft:regeneration").unwrap(),
        duration: 140,
    };
    let life = EntityLifeState {
        child_ticks: 1234,
        harvest_ticks: 789,
        brown_mooshroom: true,
        tamed: true,
        sitting: true,
        stew_effects: vec![effect],
        ..Default::default()
    };
    let native = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:mooshroom".into(),
        age: -1234,
        extra_fields: entity_life::to_fields(&life, 321),
        ..Default::default()
    };
    let fields = mc_data::entity_nbt::entity_to_fields(&registry.items, &native).unwrap();
    let decoded =
        mc_data::entity_nbt::entity_from_fields(&registry.items, Default::default(), &fields)
            .unwrap();
    assert_eq!(decoded.age, -1234);
    assert_eq!(
        entity_life::from_record(&decoded, true),
        (life.clone(), 321)
    );
    let dir = std::env::temp_dir().join(format!("solaris-life-region-{}", uuid::Uuid::new_v4()));
    let chunks = std::collections::BTreeMap::from([(
        (0, 0),
        mc_world::anvil::EntityChunk {
            x: 0,
            z: 0,
            entities: vec![native],
        },
    )]);
    mc_world::anvil::entity_regions::write_entity_regions(&dir, &registry.items, &chunks, 1)
        .unwrap();
    let restored =
        mc_world::anvil::entity_regions::read_entity_regions(&dir, &registry.items).unwrap();
    let restored = &restored[&(0, 0)].entities[0];
    assert_eq!(restored.age, -1234);
    let retained = crate::journal::EntityStateImage::from_record(&registry, restored).unwrap();
    assert_eq!(retained.entity_life, life);
    assert_eq!(retained.love, 321);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sheep_offspring_inherits_recipe_mixed_wool_without_sheared_bit() {
    let (mut state, _, pos) = fixture();
    let a = spawn(&mut state, "sheep", pos);
    let b = spawn(&mut state, "sheep", pos);
    for (id, color) in [(a, 14), (b, 4)] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).love = 540;
        crate::tick::set_wool(
            Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
            color | 0x10,
        );
    }
    let type_id = match body(&state, a).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    let mut intents = vec![Intent {
        actor: a,
        seq: 0,
        kind: IntentKind::Breed {
            partner: b,
            child_pos: pos,
            child_type: type_id,
        },
        footprint: Footprint {
            entity_reads: vec![a, b],
            entity_writes: vec![a, b],
            ..Default::default()
        },
    }];
    assert_eq!(state.merge(&mut intents), 1);
    let (_, _, born, _) = state.commit_merge_effects();
    let child = born
        .into_iter()
        .find(|id| row(&state, *id).entity_life.child_ticks > 0)
        .unwrap();
    assert_eq!(crate::tick::wool_of(&row(&state, child).appearance), 1);
}

#[test]
fn breeding_without_a_love_cause_does_not_credit_nearby_player() {
    let (mut state, player, pos) = fixture();
    let a = spawn(&mut state, "rabbit", pos);
    let b = spawn(&mut state, "rabbit", pos);
    let type_id = match body(&state, a).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    entity_life::breed(&mut state, a, b, pos, type_id);
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(mc_data::statistics::CustomStat::AnimalsBred),
        0
    );
}

#[test]
fn passive_gap_held_food_lures_without_feeding_in_both_hands() {
    for hand in [Hand::Main, Hand::Off] {
        let (mut state, actor, pos) = fixture();
        let cow = spawn(&mut state, "cow", [pos[0] + 6.0, pos[1], pos[2]]);
        let wheat = item(&state, "wheat");
        let player = state.players.get_mut(&actor).unwrap();
        let index = crate::tick::held_slot_index(player, hand);
        player.inventory.slots[index] = mc_data::ItemStack::new(wheat, 1);
        for _ in 0..30 {
            run_tick(&mut state, &[]);
        }
        let slot = state.entities.slot_of(cow).unwrap();
        assert!(state.entities.pos(slot)[0] < pos[0] + 4.5, "{hand:?}");
        assert_eq!(state.players[&actor].inventory.slots[index].count, 1);
        assert_eq!(row(&state, cow).love, 0);
    }
}

#[test]
fn passive_gap_baby_follows_adult_without_food() {
    let (mut state, _, pos) = fixture();
    let adult = spawn(&mut state, "cow", pos);
    let slot = state.entities.slot_of(adult).unwrap();
    state.entities.row_mut(slot).controls.no_ai = true;
    let calf = spawn(&mut state, "cow", [pos[0] + 6.0, pos[1], pos[2]]);
    entity_life::make_child(&mut state, calf);
    for _ in 0..30 {
        run_tick(&mut state, &[]);
    }
    let slot = state.entities.slot_of(calf).unwrap();
    assert!(state.entities.pos(slot)[0] < pos[0] + 4.5);
}

#[test]
fn passive_gap_horse_donkey_courtship_produces_sterile_mule() {
    for reverse in [false, true] {
        let (mut state, actor, pos) = fixture();
        let species = if reverse {
            ["donkey", "horse"]
        } else {
            ["horse", "donkey"]
        };
        let a = spawn(&mut state, species[0], [pos[0], pos[1], pos[2] + 1.0]);
        let b = spawn(&mut state, species[1], [pos[0] + 1.0, pos[1], pos[2] + 1.0]);
        for id in [a, b] {
            let slot = state.entities.slot_of(id).unwrap();
            state.entities.row_mut(slot).entity_life.tamed = true;
        }
        hold(&mut state, actor, "golden_carrot", 2);
        run_tick(&mut state, &[use_animal(a)]);
        run_tick(&mut state, &[use_animal(b)]);
        for _ in 0..90 {
            run_tick(&mut state, &[]);
        }
        let children: Vec<_> = state
            .entities
            .slots()
            .filter(|slot| state.entities.row(*slot).entity_life.child_ticks > 0)
            .collect();
        assert_eq!(children.len(), 1);
        let slot = children[0];
        let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
            panic!("mob child")
        };
        assert_eq!(
            state.registry.blueprint(type_id).unwrap().name,
            "minecraft:mule"
        );
        assert!(!entity_life::can_breed(
            &state,
            state.entities.body(slot),
            state.entities.row(slot)
        ));
        assert!(row(&state, a).breed_cooldown > 0);
        assert!(row(&state, b).breed_cooldown > 0);
    }
}

#[test]
fn passive_gap_breeding_merge_rejects_wrong_child_and_different_species() {
    let (mut state, _, pos) = fixture();
    let a = spawn(&mut state, "cow", pos);
    let b = spawn(&mut state, "sheep", pos);
    for id in [a, b] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).love = 540;
    }
    let cow = state.registry.cow_type_id;
    assert!(!entity_life::breeding::valid(&state, a, b, cow));
    assert!(!entity_life::breeding::valid(&state, a, a, cow));
    let c = spawn(&mut state, "cow", pos);
    let slot = state.entities.slot_of(c).unwrap();
    state.entities.row_mut(slot).love = 540;
    assert!(!entity_life::breeding::valid(
        &state,
        a,
        c,
        state.registry.zombie_type_id
    ));
    assert!(entity_life::breeding::valid(&state, a, c, cow));
    state.entities.set_pos(slot, [pos[0] + 4.0, pos[1], pos[2]]);
    assert!(!entity_life::breeding::valid(&state, a, c, cow));
}

#[test]
fn passive_gap_temptation_respects_sitting_no_ai_and_spectator() {
    for mode in 0..3 {
        let (mut state, actor, pos) = fixture();
        let cow = spawn(&mut state, "cow", [pos[0] + 6.0, pos[1], pos[2]]);
        let slot = state.entities.slot_of(cow).unwrap();
        hold(&mut state, actor, "wheat", 1);
        match mode {
            0 => state.entities.row_mut(slot).controls.no_ai = true,
            1 => state.entities.row_mut(slot).entity_life.sitting = true,
            _ => {
                state.players.get_mut(&actor).unwrap().game_mode =
                    crate::commands::GameMode::Spectator
            }
        }
        for _ in 0..30 {
            run_tick(&mut state, &[]);
        }
        assert!(state.entities.pos(slot)[0] > pos[0] + 4.5, "mode={mode}");
    }
}

#[test]
fn sniffer_breeding_stages_an_egg_item_instead_of_a_live_child() {
    let (mut state, _, pos) = fixture();
    let a = spawn(&mut state, "sniffer", pos);
    let b = spawn(&mut state, "sniffer", pos);
    for id in [a, b] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).love = 540;
    }
    let type_id = match body(&state, a).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    entity_life::breed(&mut state, a, b, pos, type_id);
    assert_eq!(row(&state, a).breed_cooldown, 6000);
    assert_eq!(row(&state, b).breed_cooldown, 6000);
    assert!(
        state
            .entities
            .slots()
            .filter(|slot| state.entities.row(*slot).entity_life.child_ticks > 0)
            .count()
            == 0,
        "sniffer birth lays an egg, not a live child"
    );
    let egg = item(&state, "sniffer_egg");
    let dropped: i32 = state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .filter(|stack| stack.item_id == egg)
        .map(|stack| stack.count)
        .sum();
    assert_eq!(
        dropped
            + state
                .merge_life_drops
                .iter()
                .filter(|(_, stack)| stack.item_id == egg)
                .map(|(_, stack)| stack.count)
                .sum::<i32>(),
        1
    );
}

#[test]
fn turtle_breeding_lays_egg_blocks_on_home_sand() {
    let (mut state, _, pos) = fixture();
    let cell = [
        pos[0].floor() as i32,
        pos[1].floor() as i32 - 1,
        pos[2].floor() as i32,
    ];
    let sand = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:sand").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(cell[0], cell[1], cell[2], sand);
    state.world.fold_overlay();
    let a = spawn(&mut state, "turtle", pos);
    let b = spawn(&mut state, "turtle", pos);
    for id in [a, b] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).love = 540;
    }
    let type_id = match body(&state, a).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    entity_life::breed(&mut state, a, b, pos, type_id);
    assert!(
        state
            .merge_block_writes
            .iter()
            .any(|(at, id)| at.x == cell[0]
                && at.y == cell[1] + 1
                && at.z == cell[2]
                && state
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(*id))
                    .is_some_and(|block| block.block.id.path() == "turtle_egg")),
        "turtle birth places egg blocks on sand"
    );
    assert_eq!(
        state
            .entities
            .slots()
            .filter(|slot| state.entities.row(*slot).entity_life.child_ticks > 0)
            .count(),
        0
    );
}

#[test]
fn frog_breeding_places_frogspawn_on_nearby_water() {
    let (mut state, _, pos) = fixture();
    let water = state.registry.states.water_levels[0];
    let cell = [
        pos[0].floor() as i32,
        pos[1].floor() as i32 - 1,
        pos[2].floor() as i32,
    ];
    state.world.set_block(cell[0], cell[1], cell[2], water);
    state.world.fold_overlay();
    let a = spawn(&mut state, "frog", pos);
    let b = spawn(&mut state, "frog", pos);
    for id in [a, b] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).love = 540;
    }
    let type_id = match body(&state, a).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    entity_life::breed(&mut state, a, b, pos, type_id);
    assert!(
        state.merge_block_writes.iter().any(|(_, id)| state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(*id))
            .is_some_and(|block| block.block.id.path() == "frogspawn")),
        "frog birth places frogspawn"
    );
    assert_eq!(
        state
            .entities
            .slots()
            .filter(|slot| state.entities.row(*slot).entity_life.child_ticks > 0)
            .count(),
        0
    );
}

#[test]
fn axolotl_tropical_fish_bucket_returns_a_water_bucket() {
    let (mut state, actor, pos) = fixture();
    let id = spawn(&mut state, "axolotl", [pos[0], pos[1], pos[2] + 1.0]);
    hold(&mut state, actor, "tropical_fish_bucket", 1);
    run_tick(&mut state, &[use_animal(id)]);
    assert!(row(&state, id).love > 0, "fed axolotl is in love");
    assert_eq!(
        state.players[&actor].inventory.slots[0].item_id,
        item(&state, "water_bucket")
    );
}

#[test]
fn axolotl_stacked_fish_buckets_keep_a_water_bucket_remainder() {
    let (mut state, actor, pos) = fixture();
    let id = spawn(&mut state, "axolotl", [pos[0], pos[1], pos[2] + 1.0]);
    hold(&mut state, actor, "tropical_fish_bucket", 2);
    run_tick(&mut state, &[use_animal(id)]);
    let fish = item(&state, "tropical_fish_bucket");
    let water = item(&state, "water_bucket");
    let (fish_total, water_total): (i32, i32) =
        state.players[&actor].inventory.slots.iter().fold((0, 0), |(f, w), stack| {
            (
                f + (stack.item_id == fish) as i32 * stack.count,
                w + (stack.item_id == water) as i32 * stack.count,
            )
        });
    assert_eq!((fish_total, water_total), (1, 1));
}

#[test]
fn pandas_without_nearby_bamboo_cannot_breed() {
    let (mut state, _, pos) = fixture();
    let a = spawn(&mut state, "panda", pos);
    let b = spawn(&mut state, "panda", [pos[0] + 1.0, pos[1], pos[2]]);
    for id in [a, b] {
        let slot = state.entities.slot_of(id).unwrap();
        let row = state.entities.row_mut(slot);
        row.love = 540;
    }
    assert!(!entity_life::can_breed(
        &state,
        body(&state, a),
        row(&state, a)
    ));
    // A planted bamboo nearby restores birth eligibility (love still applies either way).
    let bamboo = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:bamboo").unwrap())
        .unwrap()
        .default
        .0;
    let cell = [
        pos[0].floor() as i32,
        pos[1].floor() as i32 - 1,
        pos[2].floor() as i32 + 2,
    ];
    state.world.set_block(cell[0], cell[1], cell[2], bamboo);
    state.world.fold_overlay();
    assert!(entity_life::can_breed(
        &state,
        body(&state, a),
        row(&state, a)
    ));
    let type_id = match body(&state, a).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    assert!(entity_life::breeding::valid(&state, a, b, type_id));
}

#[test]
fn trader_llamas_breed_hay_with_llamas_into_llamas() {
    let (mut state, actor, pos) = fixture();
    let trader = spawn(&mut state, "trader_llama", pos);
    let llama = spawn(&mut state, "llama", [pos[0] + 1.0, pos[1], pos[2]]);
    for id in [trader, llama] {
        let max = state
            .registry
            .blueprint(match body(&state, id).kind {
                EntityKind::Mob { type_id } => type_id,
                _ => unreachable!(),
            })
            .unwrap()
            .params
            .max_health;
        let slot = state.entities.slot_of(id).unwrap();
        let row = state.entities.row_mut(slot);
        row.entity_life.tamed = true;
        row.health = max;
    }
    hold(&mut state, actor, "hay_block", 2);
    run_tick(&mut state, &[use_animal(trader)]);
    run_tick(&mut state, &[use_animal(llama)]);
    assert!(row(&state, trader).love > 0, "fed trader llama is in love");
    assert!(row(&state, llama).love > 0, "fed llama is in love");
    let trader_type = match body(&state, trader).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    let llama_type = match body(&state, llama).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    let child = entity_life::breeding::offspring(&state.registry, trader_type, llama_type);
    assert_eq!(child, Some(llama_type));
    assert!(entity_life::breeding::offspring(&state.registry, trader_type, trader_type).is_some());
}

#[test]
fn sitting_cat_breeds_with_a_standing_partner_but_not_two_sitting() {
    let (mut state, actor, pos) = fixture();
    let a = spawn(&mut state, "cat", pos);
    let b = spawn(&mut state, "cat", [pos[0] + 1.0, pos[1], pos[2]]);
    let owner = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    for id in [a, b] {
        let slot = state.entities.slot_of(id).unwrap();
        let row = state.entities.row_mut(slot);
        row.entity_life.tamed = true;
        row.entity_life.taming.owner = Some(owner);
    }
    let slot = state.entities.slot_of(a).unwrap();
    state.entities.row_mut(slot).entity_life.sitting = true;
    // A sitting cat still accepts breeding fish into love.
    hold(&mut state, actor, "cod", 2);
    run_tick(&mut state, &[use_animal(a)]);
    run_tick(&mut state, &[use_animal(b)]);
    assert!(row(&state, a).love > 0, "sitting cat enters love");
    assert!(row(&state, b).love > 0, "standing cat enters love");
    let type_id = match body(&state, a).kind {
        EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    // Courtship: pairs commit only once love matures past the cooldown gate.
    for _ in 0..65 {
        run_tick(&mut state, &[]);
    }
    assert!(entity_life::breeding::valid(&state, a, b, type_id));
    let slot = state.entities.slot_of(b).unwrap();
    state.entities.row_mut(slot).entity_life.sitting = true;
    assert!(!entity_life::breeding::valid(&state, a, b, type_id));
}
