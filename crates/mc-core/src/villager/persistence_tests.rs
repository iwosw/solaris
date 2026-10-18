use super::*;
use mc_data::{
    Identifier, ItemEnchantment,
    item_stack::{ExtendedItemComponents, FireworkExplosion, Fireworks},
};

fn stack(items: &ItemRegistry, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        items.id_of(&Identifier::parse(name).unwrap()).unwrap(),
        count,
    )
}
fn fixture(items: &ItemRegistry) -> VillagerState {
    let mut sword = stack(items, "minecraft:diamond_sword", 1);
    sword.damage = Some(37);
    sword.custom_name = Some("Village heirloom".into());
    sword.item_model = Some(Identifier::parse("minecraft:diamond_sword").unwrap().into());
    sword.enchantments.push(ItemEnchantment {
        id: Identifier::parse("minecraft:sharpness").unwrap(),
        level: 4,
    });
    let rocket = ItemStack {
        components: Some(Box::new(ExtendedItemComponents {
            fireworks: Some(Fireworks {
                flight_duration: 3,
                explosions: vec![FireworkExplosion {
                    shape: 2,
                    colors: vec![0x123456, 0xabcdef],
                    fade_colors: vec![0x345678],
                    has_trail: true,
                    has_twinkle: true,
                }],
            }),
            ..ExtendedItemComponents::EMPTY
        })),
        ..stack(items, "minecraft:firework_rocket", 3)
    };
    let mut state = VillagerState {
        settlement: Some(crate::settlements::SettlementId(7)),
        household: Some("7-1".into()),
        birth_origin: Some(mc_script::ScriptNativeBirthOrigin::new(
            [
                uuid::Uuid::from_u128(1).to_string(),
                uuid::Uuid::from_u128(2).to_string(),
            ],
            5,
            40,
        )),
        birth_sequence: Some(40 * 65_536 + 1),
        courtship_started_tick: Some(100),
        data: VillagerData {
            kind: VillagerKind::Snow,
            profession: VillagerProfession::Weaponsmith,
            level: 4,
        },
        data_finalized: true,
        xp: 175,
        offers: vec![VillagerTradeOffer {
            cost_a: stack(items, "minecraft:emerald", 13),
            cost_b: Some(stack(items, "minecraft:wheat", 2)),
            result: sword.clone(),
            uses: 3,
            max_uses: 12,
            reward_exp: false,
            xp: 15,
            special_price: -3,
            demand: 2,
            price_multiplier: 0.2,
        }],
        gossips: vec![
            Gossip {
                target: uuid::Uuid::from_u128(0x123456789abcdeff),
                kind: "major_positive".into(),
                value: 100,
            },
            Gossip {
                target: uuid::Uuid::from_u128(0xabcdef),
                kind: "minor_negative".into(),
                value: 25,
            },
        ],
        job_site: Some(BlockPos {
            x: -33,
            y: 70,
            z: 91,
        }),
        home: Some(BlockPos { x: 2, y: 64, z: 5 }),
        meeting_point: Some(BlockPos {
            x: 10,
            y: 63,
            z: -20,
        }),
        job_site_dimension: "minecraft:the_nether".into(),
        age: -1234,
        food_level: 12,
        last_restock: 91,
        last_restock_day: 3,
        restocks_today: 2,
        last_work: 90,
        last_slept: Some(42),
        golem_detected_until: Some(500),
        last_gossip: 50,
        last_gossip_decay: 45,
        activity: Activity::Rest,
        sleeping: true,
        courtship_until: Some(400),
        partner: Some(EntityId(7)),
        panic_until: 333,
        ..VillagerState::default()
    };
    state.inventory[1] = Some(sword);
    state.inventory[6] = Some(rocket);
    state
}
fn replace(fields: &mut Vec<(String, Tag)>, key: &str, value: Tag) {
    if let Some((_, previous)) = fields.iter_mut().find(|(name, _)| name == key) {
        *previous = value;
    } else {
        fields.push((key.into(), value));
    }
}

#[test]
fn exact_vanilla_fields_roundtrip_components_gossip_pois_and_memories() {
    let items = mc_data::items::solaris_required_items();
    let state = fixture(&items);
    let fields = villager_to_fields(&items, &state, 100).unwrap();
    assert_eq!(field(&fields, "FoodLevel").unwrap(), Some(&Tag::Byte(12)));
    assert_eq!(field(&fields, "Age").unwrap(), Some(&Tag::Int(-1234)));
    assert_eq!(field(&fields, "LastRestock").unwrap(), Some(&Tag::Long(91)));
    assert_eq!(
        field(&fields, "sleeping_pos").unwrap(),
        Some(&Tag::IntArray(vec![2, 64, 5]))
    );
    let inventory = list(&fields, "Inventory").unwrap();
    assert_eq!(inventory.len(), 2);
    assert!(
        compound(&inventory[0], "Inventory")
            .unwrap()
            .iter()
            .all(|(key, _)| key != "Slot")
    );
    let offers = compound(field(&fields, "Offers").unwrap().unwrap(), "Offers").unwrap();
    let recipes = list(offers, "Recipes").unwrap();
    let offer = compound(&recipes[0], "Recipes").unwrap();
    assert!(
        field(
            compound(field(offer, "sell").unwrap().unwrap(), "sell").unwrap(),
            "components"
        )
        .unwrap()
        .is_some()
    );
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &Tag::Compound(fields)).unwrap();
    let (_, root) = mc_nbt::read_named(&mut std::io::Cursor::new(bytes)).unwrap();
    let back = villager_from_fields(&items, compound(&root, "root").unwrap(), 100).unwrap();
    assert_eq!(back, state);
}

#[test]
fn zombie_conversion_preserves_retained_villager_fields() {
    let items = mc_data::items::solaris_required_items();
    let mut state = fixture(&items);
    state.sleeping = false;
    state.conversion_time = Some(2345);
    state.conversion_player = Some(uuid::Uuid::from_u128(0xabcdef0102030405));
    let fields = villager_to_fields(&items, &state, 100).unwrap();
    assert_eq!(
        field(&fields, "ConversionTime").unwrap(),
        Some(&Tag::Int(2345))
    );
    assert_eq!(field(&fields, "IsBaby").unwrap(), Some(&Tag::Byte(1)));
    assert_eq!(villager_from_fields(&items, &fields, 100).unwrap(), state);
    state.conversion_time = Some(-1);
    state.conversion_player = None;
    assert_eq!(
        villager_from_fields(
            &items,
            &villager_to_fields(&items, &state, 100).unwrap(),
            100
        )
        .unwrap(),
        state
    );
}

#[test]
fn vanilla_without_solaris_fields_uses_compact_inventory_and_relative_ttl() {
    let items = mc_data::items::solaris_required_items();
    let original = fixture(&items);
    let mut fields = villager_to_fields(&items, &original, 100).unwrap();
    fields.retain(|(name, _)| !name.starts_with("solaris:"));
    let loaded = villager_from_fields(&items, &fields, 300).unwrap();
    assert_eq!(loaded.golem_detected_until, Some(700));
    assert_eq!(loaded.inventory[0], original.inventory[1]);
    assert_eq!(loaded.inventory[1], original.inventory[6]);
    assert!(loaded.inventory[6].is_none());
    assert_eq!(loaded.job_site_dimension, "minecraft:the_nether");
    assert_eq!(loaded.last_gossip_decay, 45);
    assert_eq!(loaded.activity, Activity::Rest);
    assert_eq!(loaded.last_restock_day, i64::MIN);
    assert_eq!(
        villager_from_fields(&items, &[], 0).unwrap(),
        VillagerState {
            brain_clock: false,
            ..Default::default()
        }
    );
}

#[test]
fn vanilla_villager_level_minimum_normalizes_without_mutating_input() {
    let items = mc_data::items::solaris_required_items();
    let original = fixture(&items);
    for level in [i32::MIN, -1, 0, 1, 5, 7, i32::from(u8::MAX)] {
        let mut fields = villager_to_fields(&items, &original, 100).unwrap();
        let (_, Tag::Compound(data)) = fields
            .iter_mut()
            .find(|(name, _)| name == "VillagerData")
            .unwrap()
        else {
            panic!("villager data compound");
        };
        replace(data, "level", Tag::Int(level));
        let before = fields.clone();
        let loaded = villager_from_fields(&items, &fields, 100).unwrap();
        let mut expected = original.clone();
        expected.data.level = u8::try_from(level.max(1)).unwrap();
        assert_eq!(loaded, expected, "vanilla minimum for {level}");
        assert_eq!(fields, before, "source fields must remain unchanged");
        let saved = villager_to_fields(&items, &loaded, 100).unwrap();
        assert_eq!(villager_from_fields(&items, &saved, 100).unwrap(), loaded);
    }
    let mut invalid_native = original;
    invalid_native.data.level = 0;
    assert!(validate_state(&invalid_native).is_err());
    assert!(villager_to_fields(&items, &invalid_native, 100).is_err());
    assert!(
        villager_from_fields(
            &items,
            &[(
                "VillagerData".into(),
                Tag::Compound(vec![("level".into(), Tag::String("0".into()))]),
            )],
            100,
        )
        .is_err()
    );
}

#[test]
#[ignore = "requires local .analysis/maps/falcons-rock/vanilla-input real map"]
fn real_vanilla_level_seven_villager_preserves_body_and_master_trades() {
    use crate::{
        boundary::{ClickMode, EntityId},
        inventory::{Menu, MenuKind, MerchantMenu, PlayerInventory},
        registry::CoreRegistry,
        villager::trades,
    };

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.analysis/maps/falcons-rock/vanilla-input")
        .join("dimensions/minecraft/overworld/region/r.-3.1.mca");
    assert!(
        path.is_file(),
        "requires the stopped real map: {}",
        path.display()
    );
    let source_bytes = std::fs::read(&path).unwrap();
    let payloads = mc_world::anvil::region::read_region(&path).unwrap();
    let payload = payloads
        .iter()
        .find(|payload| payload.local_x == 6 && payload.local_z == 8)
        .expect("real chunk (-90, 40)");
    let (_, root) =
        mc_nbt::read_named(&mut std::io::Cursor::new(&payload.uncompressed_nbt)).unwrap();
    let root = compound(&root, "chunk").unwrap();
    let bodies = if field(root, "entities").unwrap().is_some() {
        list(root, "entities").unwrap()
    } else {
        list(root, "Entities").unwrap()
    };
    let fields = compound(&bodies[2], "villager").unwrap();
    assert_eq!(
        uuid_from_tag(field(fields, "UUID").unwrap().unwrap(), "UUID").unwrap(),
        uuid::Uuid::parse_str("47d28308-5015-478e-94cb-b7969f06230e").unwrap()
    );
    let before = fields.to_vec();
    let registry = CoreRegistry::load(None).unwrap();
    let loaded = villager_from_fields(&registry.items, fields, 0).unwrap();
    assert_eq!(loaded.data.level, 7);
    assert_eq!(loaded.data.profession, VillagerProfession::Butcher);
    assert_eq!(loaded.xp, 150);
    assert_eq!(loaded.offers.len(), 5);
    assert_eq!(fields, before.as_slice());
    let saved = villager_to_fields(&registry.items, &loaded, 0).unwrap();
    assert_eq!(
        villager_from_fields(&registry.items, &saved, 0).unwrap(),
        loaded
    );
    let image: VillagerState =
        serde_json::from_slice(&serde_json::to_vec(&loaded).unwrap()).unwrap();
    validate_state(&image).unwrap();
    assert_eq!(image, loaded);

    let mut master = loaded.clone();
    master.data.level = 5;
    master.offers.clear();
    trades::generate(&mut master, &registry);
    for level in [7, u8::MAX] {
        let mut generated = loaded.clone();
        generated.data.level = level;
        generated.offers.clear();
        trades::generate(&mut generated, &registry);
        assert_eq!(generated.data.level, level);
        assert_eq!(generated.offers, master.offers);

        let mut villager = loaded.clone();
        villager.data.level = level;
        let authored_offers = villager.offers.len();
        let mut menu = Menu::new(1, MenuKind::Merchant);
        menu.merchant = Some(MerchantMenu {
            entity: EntityId(2),
            selected: 0,
            last_restock: 0,
            reputation: 0,
            hero: None,
        });
        menu.grid = vec![villager.offers[0].cost_a.clone(), ItemStack::EMPTY];
        trades::refresh(
            &mut menu,
            &villager,
            &registry.item_rules(),
            uuid::Uuid::nil(),
        );
        let outcome = trades::execute(
            &mut menu,
            &mut PlayerInventory::default(),
            &mut villager,
            &registry,
            uuid::Uuid::nil(),
            0,
            ClickMode::Pickup,
        );
        assert_eq!(outcome.trades, 1);
        assert_eq!(villager.data.level, level);
        assert_eq!(villager.offers.len(), authored_offers);
        assert_eq!(villager.offers[0].uses, loaded.offers[0].uses + 1);
        let saved = villager_to_fields(&registry.items, &villager, 0).unwrap();
        assert_eq!(
            villager_from_fields(&registry.items, &saved, 0).unwrap(),
            villager
        );
    }
    assert_eq!(std::fs::read(path).unwrap(), source_bytes);
}

#[test]
fn malformed_present_fields_never_decode_as_cleared_state() {
    let items = mc_data::items::solaris_required_items();
    let original = fixture(&items);
    let fields = villager_to_fields(&items, &original, 100).unwrap();
    for (key, value) in [
        ("VillagerData", Tag::Int(1)),
        ("Offers", Tag::String("bad".into())),
        ("Gossips", Tag::IntArray(vec![1])),
        ("Xp", Tag::Int(-1)),
        ("FoodLevel", Tag::String("12".into())),
        ("Inventory", Tag::Int(0)),
        ("Brain", Tag::Byte(0)),
        ("ConversionPlayer", Tag::IntArray(vec![0, 0, 0])),
        ("LastGossipDecay", Tag::Long(-1)),
        ("sleeping_pos", Tag::IntArray(vec![0, 1])),
    ] {
        let mut bad = fields.clone();
        replace(&mut bad, key, value);
        assert!(
            villager_from_fields(&items, &bad, 100).is_err(),
            "reject {key}"
        );
    }
    let mut duplicate = fields.clone();
    duplicate.push(("Xp".into(), Tag::Int(8)));
    assert!(villager_from_fields(&items, &duplicate, 100).is_err());
    let mut bad_slots = fields.clone();
    let runtime = bad_slots
        .iter_mut()
        .find(|(name, _)| name == "solaris:VillagerRuntime")
        .unwrap();
    let Tag::Compound(runtime) = &mut runtime.1 else {
        panic!("runtime");
    };
    replace(runtime, "inventory_slots", Tag::IntArray(vec![1, 1]));
    assert!(villager_from_fields(&items, &bad_slots, 100).is_err());
    assert_eq!(
        villager_from_fields(&items, &fields, 100).unwrap(),
        original
    );
}

#[test]
fn malformed_components_and_offer_prices_fail_before_saving() {
    let items = mc_data::items::solaris_required_items();
    let mut state = fixture(&items);
    state.offers[0].price_multiplier = f32::NAN;
    assert!(villager_to_fields(&items, &state, 100).is_err());
    let stack = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:emerald".into())),
        ("count".into(), Tag::String("one".into())),
    ]);
    assert!(decode_stack(&items, &stack, "buy").is_err());
    let stack = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:emerald".into())),
        ("components".into(), Tag::Int(42)),
    ]);
    assert!(decode_stack(&items, &stack, "buy").is_err());
}

#[test]
fn journal_images_ignore_only_navigation_cache() {
    let items = mc_data::items::solaris_required_items();
    let mut state = fixture(&items);
    state.navigation_path.push(BlockPos { x: 1, y: 2, z: 3 });
    state.navigation_target = Some(BlockPos { x: 8, y: 9, z: 10 });
    state
        .navigation_chunks
        .push((crate::boundary::ChunkPos { x: 0, z: 0 }, 99));
    state.navigation_repath_at = 123;
    let image = journal_image(&state);
    assert!(image.navigation_path.is_empty());
    assert!(image.navigation_target.is_none());
    assert!(image.navigation_chunks.is_empty());
    assert!(journal_eq(&state, &image));
    let json = serde_json::to_vec(&state).unwrap();
    assert!(
        !std::str::from_utf8(&json)
            .unwrap()
            .contains("navigation_chunks")
    );
    let loaded: VillagerState = serde_json::from_slice(&json).unwrap();
    assert_eq!(loaded, image);
    let mut changed = image.clone();
    changed.offers[0].uses += 1;
    assert!(!journal_eq(&changed, &image));
    changed = image.clone();
    changed.partner = Some(EntityId(88));
    assert!(!journal_eq(&changed, &image));
    changed = image.clone();
    changed.last_gossip_decay += 1;
    assert!(!journal_eq(&changed, &image));
    changed = image.clone();
    changed.household = Some("7-2".into());
    assert!(!journal_eq(&changed, &image));
    changed = image.clone();
    changed.birth_origin = None;
    assert!(!journal_eq(&changed, &image));
    changed = image.clone();
    changed.birth_sequence = None;
    assert!(!journal_eq(&changed, &image));
    changed = image.clone();
    changed.courtship_started_tick = Some(101);
    assert!(!journal_eq(&changed, &image));
}

#[test]
fn journal_clock_projection_preserves_events_and_paused_brains() {
    let mut working = VillagerState {
        age: -200,
        activity: Activity::Work,
        last_work: 99,
        ..Default::default()
    };
    advance_clocks(&mut working, 100, 150);
    assert_eq!(working.age, -150);
    assert_eq!(working.last_work, 149);
    let mut stopped = working.clone();
    stopped.last_work = 148;
    advance_clocks(&mut stopped, 150, 151);
    assert_eq!(stopped.last_work, 148, "a stopped interval stays stopped");
    let mut sleeping = VillagerState {
        sleeping: true,
        last_slept: Some(99),
        age: 20,
        ..Default::default()
    };
    advance_clocks(&mut sleeping, 100, 150);
    assert_eq!(sleeping.last_slept, Some(149));
    assert_eq!(sleeping.age, 0);
    let mut panic = VillagerState {
        activity: Activity::Panic,
        panic_until: 199,
        ..Default::default()
    };
    advance_clocks(&mut panic, 100, 150);
    assert_eq!(panic.panic_until, 249);
    let mut frozen = working.clone();
    frozen.brain_clock = false;
    let unchanged = frozen.clone();
    advance_clocks(&mut frozen, 150, 200);
    assert_eq!(frozen, unchanged);
    frozen.conversion_time = Some(100);
    advance_clocks(&mut frozen, 200, 225);
    assert_eq!(
        frozen.conversion_time,
        Some(100),
        "dead clocks stay stopped"
    );
    frozen.brain_clock = true;
    advance_clocks(&mut frozen, 200, 225);
    assert_eq!(frozen.conversion_time, Some(75), "live curing ignores NoAI");
    frozen.brain_clock = false;
    assert_eq!(frozen.age, unchanged.age);
    assert_eq!(frozen.last_gossip_decay, unchanged.last_gossip_decay);
    assert_eq!(frozen.last_gossip, unchanged.last_gossip);
    let items = mc_data::items::solaris_required_items();
    let fields = villager_to_fields(&items, &frozen, 225).unwrap();
    assert_eq!(villager_from_fields(&items, &fields, 225).unwrap(), frozen);
    let mut legacy_json = serde_json::to_value(&working).unwrap();
    legacy_json.as_object_mut().unwrap().remove("brain_clock");
    let legacy: VillagerState = serde_json::from_value(legacy_json).unwrap();
    assert!(!legacy.brain_clock);
    let mut legacy_fields = villager_to_fields(&items, &working, 225).unwrap();
    let runtime = legacy_fields
        .iter_mut()
        .find(|(key, _)| key == "solaris:VillagerRuntime")
        .unwrap();
    let Tag::Compound(runtime) = &mut runtime.1 else {
        panic!("runtime compound");
    };
    runtime.retain(|(key, _)| key != "brain_clock");
    assert!(
        !villager_from_fields(&items, &legacy_fields, 225)
            .unwrap()
            .brain_clock
    );
}

#[test]
fn entity_region_codec_preserves_species_fields_without_duplicate_age() {
    let items = mc_data::items::solaris_required_items();
    let state = fixture(&items);
    let record = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:zombie_villager".into(),
        age: 999,
        extra_fields: villager_to_fields(&items, &state, 100).unwrap(),
        ..Default::default()
    };
    let fields = mc_data::entity_nbt::entity_to_fields(&items, &record).unwrap();
    assert_eq!(fields.iter().filter(|(key, _)| key == "Age").count(), 1);
    let loaded =
        mc_data::entity_nbt::entity_from_fields(&items, Default::default(), &fields).unwrap();
    assert_eq!(loaded.age, state.age);
    assert_eq!(
        villager_from_fields(&items, &loaded.extra_fields, 100).unwrap(),
        state
    );
    let mut invalid = record;
    invalid
        .extra_fields
        .push(("Health".into(), Tag::Float(1.0)));
    assert!(mc_data::entity_nbt::entity_to_fields(&items, &invalid).is_err());
}

#[test]
fn golem_memory_ttl_matches_inclusive_brain_deadline_and_handles_expired_or_permanent() {
    let items = mc_data::items::solaris_required_items();
    for until in [Some(100), Some(99), Some(500), Some(u64::MAX), None] {
        let state = VillagerState {
            golem_detected_until: until,
            ..Default::default()
        };
        let fields = villager_to_fields(&items, &state, 100).unwrap();
        assert_eq!(villager_from_fields(&items, &fields, 100).unwrap(), state);
        let brain = compound(field(&fields, "Brain").unwrap().unwrap(), "Brain").unwrap();
        let memories = compound(field(brain, "memories").unwrap().unwrap(), "memories").unwrap();
        if until == Some(100) {
            let wrapper = memory_fields(memories, "minecraft:golem_detected_recently")
                .unwrap()
                .unwrap();
            assert_eq!(field(wrapper, "ttl").unwrap(), Some(&Tag::Long(1)));
        }
        if until == Some(99) {
            assert!(
                memory_fields(memories, "minecraft:golem_detected_recently")
                    .unwrap()
                    .is_none()
            );
        }
        if until == Some(u64::MAX) {
            let wrapper = memory_fields(memories, "minecraft:golem_detected_recently")
                .unwrap()
                .unwrap();
            assert!(field(wrapper, "ttl").unwrap().is_none());
        }
    }
}

#[test]
fn malformed_brain_values_and_known_component_shapes_fail_closed() {
    let items = mc_data::items::solaris_required_items();
    for value in [
        Tag::Int(1),
        Tag::Compound(vec![("value".into(), Tag::Int(1))]),
        Tag::Compound(vec![
            ("value".into(), Tag::Byte(1)),
            ("ttl".into(), Tag::Long(-1)),
        ]),
    ] {
        let fields = vec![(
            "Brain".into(),
            Tag::Compound(vec![(
                "memories".into(),
                Tag::Compound(vec![("minecraft:golem_detected_recently".into(), value)]),
            )]),
        )];
        assert!(villager_from_fields(&items, &fields, 100).is_err());
    }
    for (key, value) in [
        ("minecraft:damage", Tag::Int(-1)),
        ("minecraft:damage", Tag::String("broken".into())),
        ("minecraft:enchantments", Tag::Int(3)),
        ("minecraft:item_model", Tag::Byte(0)),
        ("minecraft:custom_name", Tag::Int(12)),
    ] {
        let tag = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:emerald".into())),
            (
                "components".into(),
                Tag::Compound(vec![(key.into(), value)]),
            ),
        ]);
        assert!(decode_stack(&items, &tag, "buy").is_err(), "reject {key}");
    }
}

#[test]
fn retained_trade_and_inventory_counts_must_fit_one_to_sixty_four() {
    let items = mc_data::items::solaris_required_items();
    for count in [0, 65] {
        for kind in 0..4 {
            let mut state = fixture(&items);
            let stack = match kind {
                0 => &mut state.offers[0].cost_a,
                1 => state.offers[0].cost_b.as_mut().unwrap(),
                2 => &mut state.offers[0].result,
                _ => state.inventory[1].as_mut().unwrap(),
            };
            stack.count = count;
            assert!(
                villager_to_fields(&items, &state, 100).is_err(),
                "stack {kind} count {count}"
            );
        }
    }
    let mut state = fixture(&items);
    state.offers[0].cost_a.count = 64;
    state.offers[0].cost_b.as_mut().unwrap().count = 64;
    state.offers[0].result.count = 64;
    state.inventory[1].as_mut().unwrap().count = 64;
    let fields = villager_to_fields(&items, &state, 100).unwrap();
    assert_eq!(villager_from_fields(&items, &fields, 100).unwrap(), state);
    for key in ["buy", "buyB", "sell"] {
        let mut bad = fields.clone();
        let Tag::Compound(offers) =
            &mut bad.iter_mut().find(|(name, _)| name == "Offers").unwrap().1
        else {
            panic!("Offers compound");
        };
        let Tag::List(recipes) = &mut offers
            .iter_mut()
            .find(|(name, _)| name == "Recipes")
            .unwrap()
            .1
        else {
            panic!("Recipes list");
        };
        let Tag::Compound(recipe) = &mut recipes.elements[0] else {
            panic!("Recipe compound");
        };
        let Tag::Compound(stack) = &mut recipe.iter_mut().find(|(name, _)| name == key).unwrap().1
        else {
            panic!("ItemCost compound");
        };
        replace(stack, "count", Tag::Int(65));
        assert!(
            villager_from_fields(&items, &bad, 100).is_err(),
            "reject {key}"
        );
    }
    for count in [0, 65] {
        let mut bad = fields.clone();
        let Tag::List(inventory) = &mut bad
            .iter_mut()
            .find(|(name, _)| name == "Inventory")
            .unwrap()
            .1
        else {
            panic!("Inventory list");
        };
        let Tag::Compound(stack) = &mut inventory.elements[0] else {
            panic!("Inventory stack");
        };
        replace(stack, "count", Tag::Int(count));
        assert!(
            villager_from_fields(&items, &bad, 100).is_err(),
            "reject Inventory count {count}"
        );
    }
}
