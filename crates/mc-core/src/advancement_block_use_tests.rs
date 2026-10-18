use std::sync::Arc;

use crate::bees::tests::{POS, item, joined, place, state};
use crate::boundary::{ChangeSet, EntityId, Hand};
use crate::placement::Face;
use crate::tick::TickState;

fn fixture(block: &str, held: &str) -> (TickState, EntityId) {
    let mut state = state(None);
    let actor = joined(&mut state);
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    let mut tree = mc_data::advancements::AdvancementTree::default();
    tree.definitions.insert(
        "test:block_use".into(),
        serde_json::from_value(serde_json::json!({
            "criteria": {"use": {"trigger": "minecraft:item_used_on_block"}},
            "requirements": [["use"]],
        }))
        .unwrap(),
    );
    registry.advancements = Arc::new(tree);
    state.registry = Arc::new(registry);
    place(&mut state, POS, block);
    let mut stack = item(&state, held, 2);
    stack.count = state.registry.item_rules().max_stack(&stack).min(2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack;
    (state, actor)
}

fn evidence(state: &TickState, actor: EntityId, block: &str, item_id: &str) {
    assert_eq!(state.advancement_events.len(), 1);
    let event = &state.advancement_events[0];
    assert_eq!(event.entity, actor);
    assert_eq!(event.trigger, "minecraft:item_used_on_block");
    assert_eq!(event.evidence["location"]["block"]["id"], block);
    assert_eq!(event.evidence["item"]["id"], item_id);
    assert!(event.evidence.get("tool").is_none());
    let count = state
        .registry
        .item_rules()
        .max_stack(&item(state, item_id, 2))
        .min(2);
    assert_eq!(event.evidence["item"]["count"], count);
}

#[test]
fn wax_and_scrape_capture_unmodified_block_and_tool_and_refuse_no_change() {
    let (mut state, actor) = fixture("minecraft:copper_chest", "minecraft:honeycomb");
    assert!(crate::copper::use_on(&mut state, actor, Hand::Main, POS));
    evidence(
        &state,
        actor,
        "minecraft:copper_chest",
        "minecraft:honeycomb",
    );
    state.advancement_events.clear();
    assert!(!crate::copper::use_on(&mut state, actor, Hand::Main, POS));
    assert!(state.advancement_events.is_empty());
    state.players.get_mut(&actor).unwrap().inventory.slots[0] =
        item(&state, "minecraft:iron_axe", 1);
    assert!(crate::copper::use_on(&mut state, actor, Hand::Main, POS));
    evidence(
        &state,
        actor,
        "minecraft:waxed_copper_chest",
        "minecraft:iron_axe",
    );
    state.advancement_events.clear();
    assert!(!crate::copper::use_on(&mut state, actor, Hand::Main, POS));
    assert!(state.advancement_events.is_empty());
}

#[test]
fn honey_harvest_captures_empty_hive_after_effect_and_pre_debit_bottle() {
    let (mut state, actor) = fixture("minecraft:bee_nest", "minecraft:glass_bottle");
    let held = state.players[&actor].inventory.slots[0].clone();
    let mut changes = ChangeSet::default();
    assert!(crate::bees::use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &held,
        &mut changes
    ));
    assert!(state.advancement_events.is_empty());
    let full = crate::bees::property(&state, POS, "honey_level", "5").unwrap();
    state.world.set_block(POS.x, POS.y, POS.z, full);
    assert!(crate::bees::use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &held,
        &mut changes
    ));
    evidence(
        &state,
        actor,
        "minecraft:bee_nest",
        "minecraft:glass_bottle",
    );
    assert_eq!(
        state.advancement_events[0].evidence["location"]["block"]["state"]["honey_level"],
        "0"
    );
    state.advancement_events.clear();
    assert!(crate::bees::use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &held,
        &mut changes
    ));
    assert!(state.advancement_events.is_empty());
}

#[test]
fn sign_glow_and_record_insertion_award_only_real_mutations() {
    let (mut state, actor) = fixture("minecraft:oak_sign", "minecraft:glow_ink_sac");
    assert!(crate::signs::use_on(&mut state, actor, Hand::Main, POS));
    evidence(
        &state,
        actor,
        "minecraft:oak_sign",
        "minecraft:glow_ink_sac",
    );
    state.advancement_events.clear();
    crate::signs::use_on(&mut state, actor, Hand::Main, POS);
    assert!(state.advancement_events.is_empty());

    let (mut state, actor) = fixture("minecraft:jukebox", "minecraft:music_disc_cat");
    let mut changes = ChangeSet::default();
    assert!(crate::jukebox::use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &mut changes
    ));
    evidence(
        &state,
        actor,
        "minecraft:jukebox",
        "minecraft:music_disc_cat",
    );
    assert_eq!(
        state.advancement_events[0].evidence["location"]["block"]["state"]["has_record"],
        "true"
    );
    state.advancement_events.clear();
    crate::jukebox::use_on(&mut state, actor, Hand::Main, POS, &mut changes);
    assert!(state.advancement_events.is_empty());
}

#[test]
fn vault_keys_and_end_frame_eyes_capture_successful_after_images() {
    let (mut state, actor) = fixture("minecraft:vault", "minecraft:trial_key");
    let held = state.players[&actor].inventory.slots[0].clone();
    let mut changes = ChangeSet::default();
    let wrong = item(&state, "minecraft:stick", 1);
    assert!(crate::vault::use_key(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &wrong,
        &mut changes
    ));
    assert!(state.advancement_events.is_empty());
    assert!(crate::vault::use_key(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &held,
        &mut changes
    ));
    evidence(&state, actor, "minecraft:vault", "minecraft:trial_key");
    assert_eq!(
        state.advancement_events[0].evidence["location"]["block"]["state"]["vault_state"],
        "unlocking"
    );
    state.advancement_events.clear();
    crate::vault::use_key(&mut state, actor, Hand::Main, POS, &held, &mut changes);
    assert!(state.advancement_events.is_empty());

    let (mut state, actor) = fixture("minecraft:end_portal_frame", "minecraft:ender_eye");
    assert!(crate::end_portals::use_eye(
        &mut state,
        actor,
        Hand::Main,
        POS
    ));
    evidence(
        &state,
        actor,
        "minecraft:end_portal_frame",
        "minecraft:ender_eye",
    );
    assert_eq!(
        state.advancement_events[0].evidence["location"]["block"]["state"]["eye"],
        "true"
    );
    state.advancement_events.clear();
    assert!(crate::end_portals::use_eye(
        &mut state,
        actor,
        Hand::Main,
        POS
    ));
    assert!(state.advancement_events.is_empty());
}

#[test]
fn accepted_ignition_uses_clicked_location_not_adjacent_fire_and_rejects_repeat() {
    let (mut state, actor) = fixture("minecraft:stone", "minecraft:fire_charge");
    let above = crate::boundary::BlockPos {
        y: POS.y + 1,
        ..POS
    };
    place(&mut state, above, "minecraft:air");
    let mut changes = ChangeSet::default();
    assert!(crate::fire::use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        Face::Up,
        &mut changes
    ));
    evidence(&state, actor, "minecraft:stone", "minecraft:fire_charge");
    state.advancement_events.clear();
    assert!(crate::fire::use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        Face::Up,
        &mut changes
    ));
    assert!(state.advancement_events.is_empty());
}

#[test]
#[ignore = "requires local vanilla 26.1.2 advancement sidecars"]
fn vanilla_wax_on_completes_only_after_accepted_honeycomb_use() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let vanilla = mc_data::advancements::AdvancementTree::load(&root).unwrap();
    let id = "minecraft:husbandry/wax_on";
    let mut tree = mc_data::advancements::AdvancementTree::default();
    tree.definitions
        .insert(id.into(), vanilla.definitions[id].clone());
    tree.rebuild_trigger_index();
    let (mut state, actor) = fixture("minecraft:copper_chest", "minecraft:honeycomb");
    Arc::get_mut(&mut state.registry).unwrap().advancements = Arc::new(tree);
    crate::advancements::run(&mut state, &mut ChangeSet::default());
    assert!(state.players[&actor].advancement_progress.get(id).is_none());
    assert!(crate::copper::use_on(&mut state, actor, Hand::Main, POS));
    crate::advancements::run(&mut state, &mut ChangeSet::default());
    assert_eq!(state.players[&actor].advancement_progress[id]["done"], true);
    assert_eq!(
        crate::advancements::revoke(&mut state, actor, &[id.into()], None),
        1
    );
    assert!(!crate::copper::use_on(&mut state, actor, Hand::Main, POS));
    crate::advancements::run(&mut state, &mut ChangeSet::default());
    assert_ne!(state.players[&actor].advancement_progress[id]["done"], true);
}

#[test]
#[ignore = "requires local vanilla 26.1.2 advancement sidecars"]
fn vanilla_play_jukebox_in_meadows_completes_from_actual_record_and_biome() {
    struct Meadow(u32);
    impl mc_world::ChunkGenerator for Meadow {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            mc_world::Chunk::empty(
                pos,
                mc_world::BlockStateId(self.0),
                mc_data::Identifier::parse(if pos.x < 0 {
                    "minecraft:plains"
                } else {
                    "minecraft:meadow"
                })
                .unwrap(),
            )
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let vanilla = mc_data::advancements::AdvancementTree::load(&root).unwrap();
    let id = "minecraft:adventure/play_jukebox_in_meadows";
    let mut tree = mc_data::advancements::AdvancementTree::default();
    tree.definitions
        .insert(id.into(), vanilla.definitions[id].clone());
    tree.rebuild_trigger_index();
    let (mut state, actor) = fixture("minecraft:jukebox", "minecraft:music_disc_cat");
    Arc::get_mut(&mut state.registry).unwrap().advancements = Arc::new(tree);
    state.world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Meadow(state.registry.states.air)),
        &state.registry,
        mc_world::BlockStateId(state.registry.states.air),
        [4, 201, 6],
        mc_world::OVERWORLD_GEOMETRY,
    );
    state.world.ensure_blocking(
        &[
            (crate::boundary::ChunkPos { x: -1, z: 0 }, 0),
            (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
        ]
        .into_iter()
        .collect(),
    );
    let plains = crate::boundary::BlockPos { x: -4, ..POS };
    place(&mut state, plains, "minecraft:jukebox");
    place(&mut state, POS, "minecraft:jukebox");
    let disc =
        crate::advancements::item_snapshot(&state, &state.players[&actor].inventory.slots[0]);
    assert_eq!(
        disc["components"]["minecraft:jukebox_playable"]["song"],
        "minecraft:cat"
    );
    let mut changes = ChangeSet::default();
    assert!(crate::jukebox::use_on(
        &mut state,
        actor,
        Hand::Main,
        plains,
        &mut changes,
    ));
    crate::advancements::run(&mut state, &mut changes);
    assert!(state.players[&actor].advancement_progress.get(id).is_none());
    state.players.get_mut(&actor).unwrap().inventory.slots[0] =
        item(&state, "minecraft:music_disc_cat", 1);
    assert!(crate::jukebox::use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &mut changes,
    ));
    crate::advancements::run(&mut state, &mut changes);
    assert_eq!(state.players[&actor].advancement_progress[id]["done"], true);
}
