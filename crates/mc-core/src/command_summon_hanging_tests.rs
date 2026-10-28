//! `/summon` must never create a hanging entity that the native store later refuses to load.
use crate::boundary::{ChangeSet, ConnectionId, EntityDataValue, EntityId};
use crate::journal::Replay;
use crate::tick::TickState;

fn run(state: &mut TickState, actor: EntityId, command: &str) -> Vec<EntityId> {
    let parsed = crate::commands::parse(command, true).unwrap();
    let mut changes = ChangeSet::default();
    crate::command_execution::execute(state, ConnectionId(1), actor, &parsed, &mut changes);
    changes.entities_spawned
}

/// Checkpoint the entity into native storage, hydrate it back with full validation and restore it.
fn reload(state: &mut TickState, id: EntityId) {
    let (_, chunks, _) = crate::core::checkpoint_data_for(state, state.tick);
    let entity = chunks
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.wire_id == id.0)
        .expect("summoned entity is checkpointed")
        .clone();
    let payload = mc_nbt::Tag::Compound(
        mc_data::entity_nbt::entity_to_fields(&state.registry.items, &entity).unwrap(),
    );
    let mut replay = Replay::default();
    crate::dimensions::recovery::hydrate_entity(&state.registry, &mut replay, &payload, state.tick)
        .expect("native store accepts the summoned entity");
    let record = &replay.entities[&id.0];
    crate::dimensions::recovery::validate_entity(state, id.0, record)
        .expect("replay validation accepts the summoned entity");
    state.remove_entity(state.entities.slot_of(id).unwrap());
    crate::core::restore_entity(state, id.0, record).expect("entity restores");
}

fn painting(state: &TickState, id: EntityId) -> (i32, Option<u32>) {
    let appearance = &state
        .entities
        .row(state.entities.slot_of(id).unwrap())
        .appearance;
    let variant = appearance
        .extra
        .iter()
        .find_map(|(index, value)| match value {
            EntityDataValue::PaintingVariant(variant) if *index == 9 => Some(*variant),
            _ => None,
        });
    (appearance.spawn_data, variant)
}

fn variant_named(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .paintings
        .iter()
        .position(|variant| variant.name.as_str() == name)
        .unwrap() as u32
}

#[test]
fn bare_painting_summon_is_valid_south_kebab_and_reloads() {
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    let spawned = run(&mut state, actor, "summon minecraft:painting ~ ~ ~");
    assert_eq!(spawned.len(), 1);
    let id = spawned[0];
    let kebab = variant_named(&state, "minecraft:kebab");
    // add_entity data is Direction.get3DDataValue: SOUTH is 3.
    assert_eq!(painting(&state, id), (3, Some(kebab)));
    reload(&mut state, id);
    assert_eq!(painting(&state, id), (3, Some(kebab)));
}

#[test]
fn authored_painting_facing_and_variant_are_honoured_and_reload() {
    for (facing, spawn_data) in [(0, 3), (1, 4), (2, 2), (3, 5)] {
        let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
        let spawned = run(
            &mut state,
            actor,
            &format!("summon painting ~ ~ ~ {{facing:{facing}b,variant:'minecraft:water'}}"),
        );
        assert_eq!(spawned.len(), 1, "facing {facing}");
        let water = variant_named(&state, "minecraft:water");
        assert_eq!(painting(&state, spawned[0]), (spawn_data, Some(water)));
        reload(&mut state, spawned[0]);
        assert_eq!(painting(&state, spawned[0]), (spawn_data, Some(water)));
    }
    // A bare resource location resolves in the minecraft namespace.
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    let spawned = run(
        &mut state,
        actor,
        "summon painting ~ ~ ~ {variant:'wither'}",
    );
    let wither = variant_named(&state, "minecraft:wither");
    assert_eq!(painting(&state, spawned[0]), (3, Some(wither)));
    reload(&mut state, spawned[0]);
}

#[test]
fn invalid_painting_nbt_is_refused_without_creating_an_entity() {
    for nbt in [
        "{facing:4b}",
        "{facing:-1b}",
        "{facing:'north'}",
        "{variant:'minecraft:no_such_painting'}",
        "{variant:'Bad Id'}",
        "{variant:3}",
    ] {
        let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
        let before = state.entities.len();
        let spawned = run(&mut state, actor, &format!("summon painting ~ ~ ~ {nbt}"));
        assert!(spawned.is_empty(), "{nbt} must be refused");
        assert_eq!(state.entities.len(), before, "{nbt} allocated an entity");
    }
}

#[test]
fn painting_nbt_is_refused_on_other_entities() {
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    let before = state.entities.len();
    assert!(
        run(
            &mut state,
            actor,
            "summon cow ~ ~ ~ {variant:'minecraft:kebab'}"
        )
        .is_empty()
    );
    assert_eq!(state.entities.len(), before);
}

#[test]
fn other_hanging_summons_reload_from_native_storage() {
    for kind in ["item_frame", "glow_item_frame", "leash_knot"] {
        let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
        let spawned = run(&mut state, actor, &format!("summon minecraft:{kind} ~ ~ ~"));
        assert_eq!(spawned.len(), 1, "{kind}");
        reload(&mut state, spawned[0]);
    }
}
