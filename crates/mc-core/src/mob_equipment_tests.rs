use super::*;
use crate::boundary::EntityId;
use crate::registry::CoreRegistry;
use mc_data::loot::LootEnchantments;
use mc_data::loot::entity_26_1_2::{
    EntityDeathCause, EntityLootAttack, EntityLootEntity, EntityLootPlayerAttribution,
};

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
fn mob(state: &mut TickState, name: &str) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    state.spawn_mob(type_id, [0.5, 80.0, 0.5], 20.0)
}
fn held(state: &TickState, id: EntityId) -> Option<&str> {
    state
        .entities
        .row(state.entities.slot_of(id).unwrap())
        .appearance
        .equipment[0]
        .as_ref()
        .and_then(|stack| state.registry.items.name_of(stack.item_id))
        .map(|id| id.as_str())
}

#[test]
fn every_default_held_kit_is_visible_on_spawn() {
    let mut state = state();
    for (name, expected) in [
        ("skeleton", "bow"),
        ("stray", "bow"),
        ("bogged", "bow"),
        ("wither_skeleton", "stone_sword"),
        ("zombified_piglin", "golden_sword"),
        ("piglin_brute", "golden_axe"),
        ("vindicator", "iron_axe"),
        ("pillager", "crossbow"),
        ("vex", "iron_sword"),
        ("illusioner", "bow"),
    ] {
        let id = mob(&mut state, name);
        assert_eq!(
            held(&state, id),
            Some(format!("minecraft:{expected}").as_str()),
            "{name}"
        );
    }
    let mut sword = 0;
    for _ in 0..512 {
        let id = mob(&mut state, "piglin");
        match held(&state, id).unwrap() {
            "minecraft:golden_sword" => sword += 1,
            "minecraft:crossbow" => (),
            other => panic!("unexpected piglin item {other}"),
        }
    }
    assert!((200..312).contains(&sword), "50/50 piglin kit: {sword}");
    for _ in 0..128 {
        let id = mob(&mut state, "drowned");
        assert_eq!(held(&state, id), None, "non-natural drowned has no weapon");
    }
}

#[test]
fn inhabited_time_raises_live_regional_difficulty_before_checkpoint() {
    let mut state = state();
    state.difficulty = Difficulty::Hard;
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    state
        .world
        .ensure_blocking(&[(pos, 0)].into_iter().collect());
    let mob = mob(&mut state, "skeleton");
    let slot = state.entities.slot_of(mob).unwrap();
    let before = factor(&state, slot);
    for tick in 1..=100 {
        state.world.tick_inhabited_time(pos, tick);
        state.world.finish_inhabited_tick(tick);
    }
    assert_eq!(state.world.inhabited_time(pos), 100);
    assert_eq!(state.world.chunk(pos).unwrap().inhabited_time(), 0);
    assert!(factor(&state, slot) > before);
}

#[test]
fn deterministic_armor_distribution_and_difficulty_bounds() {
    assert_eq!(
        difficulty_factor(Difficulty::Peaceful, u64::MAX, i64::MAX, i64::MAX),
        0.0
    );
    assert_eq!(
        difficulty_factor(Difficulty::Easy, u64::MAX, i64::MAX, i64::MAX),
        0.0
    );
    assert_eq!(difficulty_factor(Difficulty::Normal, 0, 0, 0), 0.0);
    assert_eq!(difficulty_factor(Difficulty::Hard, 0, 0, 0), 0.125);
    assert_eq!(
        difficulty_factor(Difficulty::Hard, 1_920_000, 1_920_000, 3_600_000),
        1.0
    );
    let mut rng = Rng(17);
    let mut armored = 0;
    let mut tiers = [0; 6];
    let mut counts = [0; 4];
    for _ in 0..1_000_000 {
        if let Some((tier, count)) = armor_roll(Difficulty::Hard, 1.0, &mut rng) {
            armored += 1;
            tiers[tier] += 1;
            counts[count - 1] += 1;
        }
    }
    assert!((148_000..152_000).contains(&armored));
    for (actual, expected) in tiers
        .into_iter()
        .zip([0.2360, 0.3223, 0.3329, 0.0973, 0.0110, 0.000429])
    {
        assert!((actual as f64 / armored as f64 - expected).abs() < 0.006);
    }
    for (actual, expected) in counts.into_iter().zip([0.1, 0.09, 0.081, 0.729]) {
        assert!((actual as f64 / armored as f64 - expected).abs() < 0.006);
    }
    assert!((0..1000).all(|_| armor_roll(Difficulty::Easy, 0.0, &mut rng).is_none()));
}

#[test]
fn zombie_weapons_drowned_natural_kits_shell_and_enchantment_rolls() {
    let mut state = state();
    state.difficulty = Difficulty::Hard;
    state.day_time = 1_920_000;
    state.tick = 1_920_000;
    let mut tridents = 0;
    let mut rods = 0;
    let mut shells = 0;
    let mut weapons = 0;
    let mut swords = 0;
    let mut enchants = 0;
    for _ in 0..8192 {
        let drowned = mob(&mut state, "drowned");
        let slot = state.entities.slot_of(drowned).unwrap();
        natural(&mut state, slot);
        match held(&state, drowned) {
            Some("minecraft:trident") => tridents += 1,
            Some("minecraft:fishing_rod") => rods += 1,
            None => (),
            other => panic!("unexpected drowned weapon {other:?}"),
        }
        if state.entities.row(slot).appearance.equipment[1].is_some() {
            shells += 1;
            assert_eq!(state.entities.row(slot).loot.drop_chances[1], 2.0);
        }
        let zombie = mob(&mut state, "zombie");
        let row = state.entities.row(state.entities.slot_of(zombie).unwrap());
        if let Some(item) = &row.appearance.equipment[0] {
            weapons += 1;
            swords += usize::from(
                state.registry.items.name_of(item.item_id).unwrap().path() == "iron_sword",
            );
        }
        enchants += row
            .appearance
            .equipment
            .iter()
            .flatten()
            .filter(|item| !item.enchantments.is_empty())
            .count();
    }
    assert!((430..600).contains(&tridents), "trident 6.25%: {tridents}");
    assert!((240..390).contains(&rods), "rod 3.75%: {rods}");
    assert!((180..310).contains(&shells), "shell 3%: {shells}");
    assert!((330..490).contains(&weapons), "zombie hard 5%: {weapons}");
    assert!(
        (90..190).contains(&swords),
        "one third iron swords: {swords}"
    );
    assert!(enchants > 20, "regional enchantments are rolled");
}

fn cause(looting: u32, player: bool) -> EntityDeathCause {
    if !player {
        return EntityDeathCause::new(Default::default(), EntityLootAttack::None, None);
    }
    let mut attacker = EntityLootEntity::new(Identifier::parse("minecraft:player").unwrap());
    attacker.active_enchantments = LootEnchantments::try_from_levels(
        (looting > 0).then(|| (Identifier::parse("minecraft:looting").unwrap(), looting)),
    )
    .unwrap();
    EntityDeathCause::new(
        Default::default(),
        EntityLootAttack::Direct(attacker.clone()),
        Some(EntityLootPlayerAttribution::try_new(attacker).unwrap()),
    )
}
#[test]
fn natural_equipment_drops_85_per_thousand_plus_looting_and_picked_up_always() {
    let mut state = state();
    let mut totals = [0; 3];
    for seed in 0..8192 {
        for (mode, total) in totals.iter_mut().enumerate() {
            state.tick = seed;
            let id = mob(&mut state, "skeleton");
            let slot = state.entities.slot_of(id).unwrap();
            state.entities.body_mut(slot).alive = false;
            if mode == 2 {
                state.entities.row_mut(slot).loot.drop_chances[0] = 2.0;
            }
            let mut dropped = Vec::new();
            crate::entity_loot::commit_death(
                &mut state,
                slot,
                cause(if mode == 1 { 3 } else { 0 }, mode != 2),
                &mut dropped,
            );
            *total += dropped
                .iter()
                .filter(|id| {
                    state
                        .entities
                        .row(state.entities.slot_of(**id).unwrap())
                        .item
                        .as_ref()
                        .is_some_and(|item| {
                            state.registry.items.name_of(item.item_id).unwrap().path() == "bow"
                        })
                })
                .count();
        }
    }
    assert!((600..800).contains(&totals[0]), "8.5% base: {totals:?}");
    assert!(
        (830..1060).contains(&totals[1]),
        "11.5% looting III: {totals:?}"
    );
    assert_eq!(
        totals[2], 8192,
        "picked-up equipment drops without player credit"
    );
}

#[test]
fn checkpoint_native_and_vanilla_equipment_keys_preserve_default_kit() {
    let mut state = state();
    let id = mob(&mut state, "skeleton");
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).loot.can_pick_up_loot = true;
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, 1);
    let record = &chunks.values().next().unwrap().entities[0];
    let fields = &record.extra_fields;
    let equipment = fields.iter().find(|(key, _)| key == "equipment").unwrap();
    assert!(
        matches!(&equipment.1, mc_nbt::Tag::Compound(slots) if slots.iter().any(|(key, _)| key == "mainhand"))
    );
    assert!(
        fields
            .iter()
            .any(|(key, value)| key == "drop_chances" && matches!(value, mc_nbt::Tag::Compound(_)))
    );
    assert_eq!(
        crate::entity_loot::equipment_from_fields(&state.registry.items, fields).unwrap(),
        state.entities.row(slot).appearance.equipment
    );
    let restored = crate::journal::EntityStateImage::from_record(&state.registry, record).unwrap();
    assert_eq!(
        restored.loot.drop_chances,
        state.entities.row(slot).loot.drop_chances
    );
    assert!(restored.loot.can_pick_up_loot);
}

#[test]
fn crossbow_animation_keeps_authored_equipment_and_components() {
    let mut state = state();
    let id = mob(&mut state, "pillager");
    let slot = state.entities.slot_of(id).unwrap();
    let mut custom = stack(&state, "minecraft:crossbow").unwrap();
    custom.custom_name = Some("Retained bow".into());
    custom.damage = Some(13);
    custom
        .extended_mut()
        .charged_projectiles
        .push(stack(&state, "minecraft:arrow").unwrap());
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] = Some(custom.clone());
    crate::special_mobs::restore(&mut state, slot);
    assert_eq!(
        state.entities.row(slot).appearance.equipment[0],
        Some(custom)
    );
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] = None;
    crate::special_mobs::restore(&mut state, slot);
    assert!(state.entities.row(slot).appearance.equipment[0].is_none());
}
