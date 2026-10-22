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

#[test]
fn miner_progress_persists_through_nbt_and_durable_projection() {
    use crate::villager::mining::Progress as MiningProgress;
    let items = mc_data::items::solaris_required_items();
    let mut state = fixture(&items);
    state.data.profession = VillagerProfession::Miner;
    state.mining_progress = MiningProgress {
        building: 9,
        revision: 3,
        bounds: Some(mc_script::ScriptWorldBounds::new([0, 0, 0], [8, 16, 8]).unwrap()),
        last_open: Some([1, 2, 3]),
        lane: 2,
        level: 1,
        pass: 2,
        cell: 42,
        layer: 2,
        returning: true,
        stalled_since: 777,
    };
    let fields = villager_to_fields(&items, &state, 100).unwrap();
    let Tag::Compound(runtime) = field(&fields, "solaris:VillagerRuntime").unwrap().unwrap() else {
        panic!("runtime compound");
    };
    let Some(Tag::String(json)) = runtime
        .iter()
        .find_map(|(name, tag)| (name == "SolarisMiningProgress").then_some(tag))
    else {
        panic!("SolarisMiningProgress tag");
    };
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "building": 9,
            "revision": 3,
            "bounds": {"min": [0, 0, 0], "max": [8, 16, 8]},
            "last_open": [1, 2, 3],
            "lane": 2,
            "level": 1,
            "pass": 2,
            "cell": 42,
            "layer": 2,
            "returning": true,
            "stalled_since": 777,
        })
    );
    // Default lumber progress stays absent; no backward-compat placeholders.
    assert!(
        runtime
            .iter()
            .all(|(name, _)| name != "SolarisLumberProgress"),
        "default lumber progress must not emit a tag"
    );
    let loaded = villager_from_fields(&items, &fields, 100).unwrap();
    assert_eq!(loaded, state);
    assert_eq!(loaded.mining_progress, state.mining_progress);
    // The durable projection retains the cursor and notices changes to it.
    let image = journal_image(&state);
    assert!(journal_eq(&state, &image));
    assert_eq!(image.mining_progress, state.mining_progress);
    let mut moved = image.clone();
    moved.mining_progress.cell += 1;
    assert!(!journal_eq(&moved, &image));
    // The serde journal/checkpoint image retains progress as well.
    let journal: VillagerState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(journal, state);
}

#[test]
fn lumberjack_progress_persists_through_nbt_and_durable_projection() {
    use crate::villager::lumber::Progress as LumberProgress;
    let items = mc_data::items::solaris_required_items();
    let mut state = fixture(&items);
    state.data.profession = VillagerProfession::Lumberjack;
    state.lumber_progress = LumberProgress {
        building: 5,
        revision: 2,
        cursor: 17,
        base: [3, 64, -8],
        sapling: 11,
        height: 6,
        layer: 3,
        has_target: true,
        returning: false,
    };
    let fields = villager_to_fields(&items, &state, 100).unwrap();
    let Tag::Compound(runtime) = field(&fields, "solaris:VillagerRuntime").unwrap().unwrap() else {
        panic!("runtime compound");
    };
    let Some(Tag::String(json)) = runtime
        .iter()
        .find_map(|(name, tag)| (name == "SolarisLumberProgress").then_some(tag))
    else {
        panic!("SolarisLumberProgress tag");
    };
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "building": 5,
            "revision": 2,
            "cursor": 17,
            "base": [3, 64, -8],
            "sapling": 11,
            "height": 6,
            "layer": 3,
            "has_target": true,
            "returning": false,
        })
    );
    let loaded = villager_from_fields(&items, &fields, 100).unwrap();
    assert_eq!(loaded, state);
    let image = journal_image(&state);
    assert!(journal_eq(&state, &image));
    assert_eq!(image.lumber_progress, state.lumber_progress);
    let mut moved = image.clone();
    moved.lumber_progress.cursor += 1;
    assert!(!journal_eq(&moved, &image));
    let journal: VillagerState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(journal, state);
}

#[test]
fn default_worker_progress_omitted_and_invalid_present_data_fails_closed() {
    let items = mc_data::items::solaris_required_items();
    let state = fixture(&items);
    let fields = villager_to_fields(&items, &state, 100).unwrap();
    let Tag::Compound(runtime) = field(&fields, "solaris:VillagerRuntime").unwrap().unwrap() else {
        panic!("runtime compound");
    };
    assert!(
        runtime
            .iter()
            .all(|(name, _)| name != "SolarisMiningProgress" && name != "SolarisLumberProgress"),
        "default worker progress must not emit tags"
    );
    let cases: [(&str, Tag); 8] = [
        ("SolarisMiningProgress", Tag::String("not json".into())),
        ("SolarisMiningProgress", Tag::String("{}".into())),
        (
            "SolarisMiningProgress",
            Tag::String(
                r#"{"building":1,"revision":1,"bounds":{"min":[0,0,0],"max":[8,16,8]},"last_open":[1,2,3],"lane":2,"level":0,"cell":1,"layer":1,"returning":false,"bogus":1}"#
                    .into(),
            ),
        ),
        ("SolarisMiningProgress", Tag::Int(1)),
        ("SolarisLumberProgress", Tag::String("not json".into())),
        ("SolarisLumberProgress", Tag::String("{}".into())),
        (
            "SolarisLumberProgress",
            Tag::String(
                r#"{"building":1,"revision":1,"cursor":1,"base":[0,0,0],"sapling":1,"height":1,"layer":1,"has_target":false,"returning":false,"bogus":1}"#
                    .into(),
            ),
        ),
        ("SolarisLumberProgress", Tag::Int(0)),
    ];
    for (index, (key, bad)) in cases.into_iter().enumerate() {
        let mut bad_fields = fields.clone();
        let Tag::Compound(runtime) = &mut bad_fields
            .iter_mut()
            .find(|(name, _)| name == "solaris:VillagerRuntime")
            .unwrap()
            .1
        else {
            panic!("runtime compound");
        };
        replace(runtime, key, bad);
        assert!(
            villager_from_fields(&items, &bad_fields, 100).is_err(),
            "reject present-but-invalid {key} case {index}"
        );
    }
    // Absent tags still decode to the default cursor.
    let loaded = villager_from_fields(&items, &fields, 100).unwrap();
    assert_eq!(loaded.mining_progress, state.mining_progress);
    assert_eq!(loaded.lumber_progress, state.lumber_progress);
}

/// Worlds saved before the miner gained `level` and the lumberjack lost
/// `vertical` must still load: both the villager NBT string and the serde
/// journal image keep decoding, level defaults to the lowest comb and the
/// removed band is ignored.
#[test]
fn legacy_worker_progress_shapes_still_decode() {
    let items = mc_data::items::solaris_required_items();
    let state = fixture(&items);
    let mining = serde_json::json!({
        "building": 9, "revision": 3,
        "bounds": {"min": [0, 0, 0], "max": [8, 16, 8]},
        "last_open": [1, 2, 3], "lane": 2, "cell": 42, "layer": 2, "returning": true,
    });
    let lumber = serde_json::json!({
        "building": 5, "revision": 2, "cursor": 17, "vertical": 2, "base": [3, 64, -8],
        "sapling": 11, "height": 6, "layer": 3, "has_target": true, "returning": false,
    });
    let mut fields = villager_to_fields(&items, &state, 100).unwrap();
    let Tag::Compound(runtime) = &mut fields
        .iter_mut()
        .find(|(name, _)| name == "solaris:VillagerRuntime")
        .unwrap()
        .1
    else {
        panic!("runtime compound");
    };
    replace(
        runtime,
        "SolarisMiningProgress",
        Tag::String(mining.to_string()),
    );
    replace(
        runtime,
        "SolarisLumberProgress",
        Tag::String(lumber.to_string()),
    );
    let loaded = villager_from_fields(&items, &fields, 100).unwrap();
    assert_eq!(loaded.mining_progress.level, 0);
    assert_eq!(loaded.mining_progress.cell, 42);
    assert_eq!(loaded.lumber_progress.cursor, 17);
    assert_eq!(loaded.lumber_progress.base, [3, 64, -8]);
    // The journal image is the same shapes inside the serde VillagerState.
    let mut image = serde_json::to_value(&state).unwrap();
    image["mining_progress"] = mining;
    image["lumber_progress"] = lumber;
    let journal: VillagerState = serde_json::from_value(image).unwrap();
    assert_eq!(journal.mining_progress, loaded.mining_progress);
    assert_eq!(journal.lumber_progress, loaded.lumber_progress);
}

#[test]
fn worker_anvil_projection_is_vanilla_and_runtime_role_wins_on_import() {
    let items = mc_data::items::solaris_required_items();
    for (profession, vanilla, vanilla_id, role) in [
        (
            VillagerProfession::Miner,
            VillagerProfession::Toolsmith,
            "minecraft:toolsmith",
            "miner",
        ),
        (
            VillagerProfession::Lumberjack,
            VillagerProfession::Fletcher,
            "minecraft:fletcher",
            "lumberjack",
        ),
        (
            VillagerProfession::Builder,
            VillagerProfession::Mason,
            "minecraft:mason",
            "builder",
        ),
    ] {
        let mut state = fixture(&items);
        state.data.profession = profession;
        let fields = villager_to_fields(&items, &state, 100).unwrap();
        let Tag::Compound(data) = field(&fields, "VillagerData").unwrap().unwrap() else {
            panic!("VillagerData compound");
        };
        assert_eq!(
            field(data, "profession").unwrap(),
            Some(&Tag::String(vanilla_id.into()))
        );
        let Tag::Compound(runtime) = field(&fields, "solaris:VillagerRuntime").unwrap().unwrap()
        else {
            panic!("runtime compound");
        };
        assert_eq!(
            field(runtime, "profession").unwrap(),
            Some(&Tag::String(role.into()))
        );
        assert_eq!(
            villager_from_fields(&items, &fields, 100)
                .unwrap()
                .data
                .profession,
            profession
        );
        let mut vanilla_only = fields.clone();
        let Tag::Compound(runtime) = &mut vanilla_only
            .iter_mut()
            .find(|(key, _)| key == "solaris:VillagerRuntime")
            .unwrap()
            .1
        else {
            panic!("runtime compound");
        };
        runtime.retain(|(key, _)| key != "profession");
        assert_eq!(
            villager_from_fields(&items, &vanilla_only, 100)
                .unwrap()
                .data
                .profession,
            vanilla,
            "a vanilla import without Solaris runtime must keep its actual vanilla role"
        );
        let mut overridden = fields.clone();
        let Tag::Compound(data) = &mut overridden
            .iter_mut()
            .find(|(key, _)| key == "VillagerData")
            .unwrap()
            .1
        else {
            panic!("VillagerData compound");
        };
        replace(
            data,
            "profession",
            Tag::String("minecraft:librarian".into()),
        );
        assert_eq!(
            villager_from_fields(&items, &overridden, 100)
                .unwrap()
                .data
                .profession,
            profession,
            "the native runtime role wins over a valid vanilla presentation"
        );
        let journal: VillagerState =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(journal.data.profession, profession);
    }
}

#[test]
fn custom_roles_are_rejected_in_vanilla_nbt_and_invalid_runtime_roles_fail_closed() {
    let items = mc_data::items::solaris_required_items();
    let state = fixture(&items);
    let fields = villager_to_fields(&items, &state, 100).unwrap();
    for name in [
        "minecraft:miner",
        "minecraft:lumberjack",
        "solaris:miner",
        "solaris:lumberjack",
        "miner",
        "lumberjack",
        "minecraft:archaeologist",
        "solaris:archaeologist",
    ] {
        let mut bad = fields.clone();
        let Tag::Compound(data) = &mut bad
            .iter_mut()
            .find(|(key, _)| key == "VillagerData")
            .unwrap()
            .1
        else {
            panic!("VillagerData compound");
        };
        replace(data, "profession", Tag::String(name.into()));
        assert!(
            villager_from_fields(&items, &bad, 100).is_err(),
            "reject vanilla registry id {name}"
        );
    }
    for bad_role in [
        Tag::Int(1),
        Tag::String("unknown".into()),
        Tag::String("toolsmith".into()),
        Tag::String("solaris:miner".into()),
    ] {
        let mut bad = fields.clone();
        let Tag::Compound(runtime) = &mut bad
            .iter_mut()
            .find(|(key, _)| key == "solaris:VillagerRuntime")
            .unwrap()
            .1
        else {
            panic!("runtime compound");
        };
        replace(runtime, "profession", bad_role);
        assert!(villager_from_fields(&items, &bad, 100).is_err());
    }
    assert_eq!(
        VillagerProfession::from_name("miner"),
        Some(VillagerProfession::Miner)
    );
    assert_eq!(
        VillagerProfession::from_name("lumberjack"),
        Some(VillagerProfession::Lumberjack)
    );
}

#[test]
fn journal_baseline_static_footprint() {
    // Exp-VillagerBox step 1: record the inline baseline footprint that every
    // occupied AND every empty hash bucket pays while the map value is inline.
    let inline = std::mem::size_of::<VillagerState>();
    let entry = std::mem::size_of::<(u64, VillagerState)>();
    let boxed_entry = std::mem::size_of::<(u64, Box<VillagerState>)>();
    eprintln!("size_of::<VillagerState>() = {inline}");
    eprintln!("size_of::<(u64, VillagerState)>() = {entry}");
    eprintln!("size_of::<(u64, Box<VillagerState>)>() = {boxed_entry}");
    eprintln!("boxed saving per map slot = {}", entry - boxed_entry);
    assert!(
        inline > 512,
        "expected a heavyweight inline VillagerState, got {inline}"
    );
    assert!(
        entry - boxed_entry >= 512,
        "boxing must save >= 512 bytes per slot, saved {}",
        entry - boxed_entry
    );
}

#[test]
fn journal_image_runtime_fields_are_zeroed() {
    // Exp-VillagerBox stretch input: enumerate the runtime-only fields that
    // `journal_image` zeroes/defaults, and price the cheapest durable
    // projection (remove them from the stored baseline type entirely).
    let items = mc_data::items::solaris_required_items();
    let mut live = fixture(&items);
    // Prove zeroing against a live state that actually holds runtime caches.
    live.navigation_path = vec![BlockPos { x: 1, y: 2, z: 3 }];
    live.navigation_target = Some(BlockPos { x: 4, y: 5, z: 6 });
    live.navigation_chunks = vec![(crate::boundary::ChunkPos { x: 0, z: 0 }, 9)];
    live.opened_doors = vec![BlockPos { x: 7, y: 8, z: 9 }];
    live.harvest_at = 123;
    live.navigation_repath_at = 456;
    live.navigation_can_open_doors = true;
    live.navigation_progress_len = 2;
    live.navigation_progress_next = Some(BlockPos { x: 1, y: 2, z: 3 });
    live.navigation_progress_dist_sq = 4.5;
    let image = journal_image(&live);
    assert!(image.navigation_path.is_empty());
    assert!(image.navigation_target.is_none());
    assert!(image.navigation_chunks.is_empty());
    assert!(image.opened_doors.is_empty());
    assert_eq!(image.harvest_at, 0);
    assert_eq!(image.navigation_repath_at, 0);
    assert!(!image.navigation_can_open_doors);
    assert_eq!(image.navigation_progress_len, 0);
    assert!(image.navigation_progress_next.is_none());
    assert_eq!(image.navigation_progress_dist_sq, 0.0);
    assert!(image.trade_work.is_none());
    // Fixed-size runtime records zeroed in place (would need removal, not
    // just zeroing, to shrink the durable type):
    let runtime_inline = std::mem::size_of_val(&image.motion_geometry)
        + std::mem::size_of_val(&image.harvest_progress)
        + std::mem::size_of_val(&image.poi_validation)
        + std::mem::size_of_val(&image.navigation_target)
        + std::mem::size_of_val(&image.navigation_progress_next);
    let runtime_vecs = std::mem::size_of_val(&image.navigation_path)
        + std::mem::size_of_val(&image.navigation_chunks)
        + std::mem::size_of_val(&image.opened_doors);
    let trade_work = std::mem::size_of::<Option<crate::villager::trades::WorkState>>();
    eprintln!("runtime fixed-size inline bytes in VillagerState = {runtime_inline}");
    eprintln!("runtime empty-Vec inline bytes (3x Vec = {runtime_vecs})");
    eprintln!("size_of Option<WorkState> = {trade_work}");
    eprintln!(
        "navigation/harvest/poi/trade removal estimate = {}",
        runtime_inline + runtime_vecs + trade_work
    );
    eprintln!(
        "size_of::<VillagerState>() = {}",
        std::mem::size_of::<VillagerState>()
    );
}

/// Deterministic footprint of N journal baselines built through the real
/// `journal_image` path into the real (boxed) map value type. The crate
/// forbids `unsafe_code`, so no counting allocator is possible here; instead
/// this reports exact, allocator-independent counters: hashbrown bucket
/// count, table bytes before/after, and the crate's own `journal_budget`
/// heap estimate for the same image. Net saving = empty-bucket table saving
/// minus the N box payloads (still on the heap) and malloc overhead.
/// Run explicitly:
/// `cargo test -p mc-core --lib journal_baseline_map_footprint -- --ignored --nocapture`
#[test]
#[ignore]
fn journal_baseline_map_footprint() {
    const N: usize = 50_000;
    /// glibc `request2size(2032)` rounds to a 2048-byte chunk: labeled,
    /// platform-specific estimate, not a measurement.
    const MALLOC_BOX_OVERHEAD: usize = 16;
    const N500: usize = 500_000;
    let items = mc_data::items::solaris_required_items();
    let template = fixture(&items);
    let image = journal_image(&template);
    let slot = std::mem::size_of::<(u64, VillagerState)>();
    let boxed_slot = std::mem::size_of::<(u64, Box<VillagerState>)>();
    let payload = std::mem::size_of::<VillagerState>();
    // Heap owned by one baseline image, priced by the crate's own budget model.
    // Identical before/after boxing, so it cancels out of the net saving.
    let heap_each = crate::journal_budget::villager_heap_for_test(&image);
    // The journal map stores the boxed baseline; this metered map uses the
    // same value type to report the post-change capacity footprint.
    let mut map: rustc_hash::FxHashMap<EntityId, (u64, Box<VillagerState>)> =
        rustc_hash::FxHashMap::default();
    for tick in 0..N as u64 {
        map.insert(
            EntityId(tick as i32),
            (tick, Box::new(journal_image(&template))),
        );
    }
    assert_eq!(map.len(), N);
    // hashbrown load factor is 7/8 and the bucket count is a power of two, so
    // buckets reconstruct exactly from `capacity()`; control bytes (1/bucket)
    // are identical before/after and cancel out.
    let buckets = map.capacity() + map.capacity() / 7;
    assert!(buckets.is_power_of_two());
    assert_eq!(buckets - buckets / 8, map.capacity());
    let empty = buckets - N;
    let table_before = buckets * slot;
    let table_after = buckets * boxed_slot + N * (payload + MALLOC_BOX_OVERHEAD);
    let net = table_before - table_after;
    // 500k projection at the bucket count hashbrown would actually use
    // (power of two >= len*8/7). Load-factor dependent: the same len just
    // before a resize keeps far fewer empty buckets (see report).
    let buckets500 = (N500 * 8 / 7).next_power_of_two();
    let net500 =
        buckets500 * slot - (buckets500 * boxed_slot + N500 * (payload + MALLOC_BOX_OVERHEAD));
    eprintln!("N={N} buckets={buckets} len={} empty={empty}", map.len());
    eprintln!("table before (buckets * inline value) = {table_before}");
    eprintln!("table after (buckets * ptr + N boxed payloads) = {table_after}");
    eprintln!("net saving N=50k = {net} bytes (~{} MiB)", net / (1 << 20));
    eprintln!(
        "500k: buckets={buckets500} empty={} net = {net500} bytes (~{} MiB)",
        buckets500 - N500,
        net500 / (1 << 20)
    );
    eprintln!("baseline-owned heap each (budget model, cancels out) = {heap_each}");
    drop(map);
}
