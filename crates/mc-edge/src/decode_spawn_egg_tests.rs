//! Ordinary spawn eggs must reach authority through the native UseItemOn decoder.
use super::{entity_slot, fixture, packet, stack};
use mc_core::boundary::EntityId;
use mc_core::commands::GameMode;
use mc_core::entities::EntityKind;
use mc_core::tick::TickState;
use mc_data::{Identifier, ItemStack};
use mc_nbt::Tag;
use mc_protocol::packets::play::{
    Direction, InteractionHand, ServerboundUseItemOn, pack_block_pos,
};

fn click(position: [i32; 3], direction: Direction, hand: InteractionHand) -> ServerboundUseItemOn {
    ServerboundUseItemOn {
        hand,
        position: pack_block_pos(position[0], position[1], position[2]),
        direction,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 1,
    }
}
fn mobs(state: &TickState, name: &str) -> Vec<EntityId> {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(name).unwrap())
        .unwrap();
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            (body.kind == EntityKind::Mob { type_id }).then_some(body.id)
        })
        .collect()
}
fn glass_pen(state: &mut TickState) {
    let glass = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:glass").unwrap())
        .unwrap()
        .default
        .0;
    for y in 201..=203 {
        for (x, z) in [(9, 8), (11, 8), (10, 7), (10, 9)] {
            state.world.set_block(x, y, z, glass);
        }
    }
}
fn equip(state: &mut TickState, actor: EntityId, name: &str, hand: InteractionHand, count: i32) {
    let egg = stack(state, name, count);
    let index = if hand == InteractionHand::MainHand {
        0
    } else {
        mc_core::inventory::OFF_HAND
    };
    state.players.get_mut(&actor).unwrap().inventory.slots[index] = egg;
}
fn assert_position(state: &TickState, id: EntityId, expected: [f64; 3]) {
    let actual = state.entities.pos(entity_slot(state, id));
    for axis in 0..3 {
        assert!(
            (actual[axis] - expected[axis]).abs() < 1e-4,
            "{actual:?} != {expected:?}"
        );
    }
}

fn assert_walking_spawn_cell(state: &TickState, id: EntityId, expected: [f64; 3]) {
    let slot = entity_slot(state, id);
    let actual = state.entities.pos(slot);
    // This is a complete tick: a walking cow already takes its first AI step.
    let step = f64::from(state.entities.row(slot).speed) / 20.0 + 1e-4;
    assert!((actual[1] - expected[1]).abs() < 1e-4);
    for axis in [0, 2] {
        assert_eq!(actual[axis].floor(), expected[axis].floor());
        assert!((actual[axis] - expected[axis]).abs() <= step);
    }
}

#[test]
fn spawn_egg_villager_on_stone_top_in_glass_pen_debits_survival_once() {
    let (mut state, mut decoder, actor) = fixture();
    glass_pen(&mut state);
    equip(
        &mut state,
        actor,
        "minecraft:villager_spawn_egg",
        InteractionHand::MainHand,
        1,
    );
    let output = packet(
        &mut state,
        &mut decoder,
        click([10, 200, 8], Direction::Up, InteractionHand::MainHand),
    );
    let villagers = mobs(&state, "minecraft:villager");
    assert_eq!(villagers.len(), 1);
    assert_eq!(output.changes.entities_spawned, villagers);
    assert_position(&state, villagers[0], [10.5, 201.0, 8.5]);
    assert!(
        state
            .entities
            .row(entity_slot(&state, villagers[0]))
            .villager
            .is_some()
    );
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    packet(
        &mut state,
        &mut decoder,
        click([10, 200, 8], Direction::Up, InteractionHand::MainHand),
    );
    assert_eq!(
        mobs(&state, "minecraft:villager").len(),
        1,
        "empty stack cannot spawn twice"
    );
}

#[test]
fn spawn_egg_villager_in_creative_preserves_egg() {
    let (mut state, mut decoder, actor) = fixture();
    state.players.get_mut(&actor).unwrap().game_mode = GameMode::Creative;
    glass_pen(&mut state);
    equip(
        &mut state,
        actor,
        "minecraft:villager_spawn_egg",
        InteractionHand::MainHand,
        1,
    );
    let before = state.players[&actor].inventory.slots[0].clone();
    packet(
        &mut state,
        &mut decoder,
        click([10, 200, 8], Direction::Up, InteractionHand::MainHand),
    );
    let villagers = mobs(&state, "minecraft:villager");
    assert_eq!(villagers.len(), 1);
    assert_position(&state, villagers[0], [10.5, 201.0, 8.5]);
    assert_eq!(state.players[&actor].inventory.slots[0], before);
}

#[test]
fn spawn_egg_cow_offhand_uses_same_admission() {
    let (mut state, mut decoder, actor) = fixture();
    glass_pen(&mut state);
    equip(
        &mut state,
        actor,
        "minecraft:cow_spawn_egg",
        InteractionHand::OffHand,
        2,
    );
    packet(
        &mut state,
        &mut decoder,
        click([10, 200, 8], Direction::Up, InteractionHand::OffHand),
    );
    let cows = mobs(&state, "minecraft:cow");
    assert_eq!(cows.len(), 1);
    assert_walking_spawn_cell(&state, cows[0], [10.5, 201.0, 8.5]);
    assert_eq!(
        state.players[&actor].inventory.slots[mc_core::inventory::OFF_HAND].count,
        1
    );
}

#[test]
fn spawn_egg_collision_empty_clicked_block_is_not_offset_by_face() {
    let (mut state, mut decoder, actor) = fixture();
    equip(
        &mut state,
        actor,
        "minecraft:villager_spawn_egg",
        InteractionHand::MainHand,
        1,
    );
    packet(
        &mut state,
        &mut decoder,
        click([10, 201, 8], Direction::East, InteractionHand::MainHand),
    );
    let villagers = mobs(&state, "minecraft:villager");
    assert_eq!(villagers.len(), 1);
    assert_position(&state, villagers[0], [10.5, 201.0, 8.5]);
}

#[test]
fn spawn_egg_solid_side_face_offsets_to_adjacent_cell() {
    let (mut state, mut decoder, actor) = fixture();
    state
        .world
        .set_block(9, 201, 8, state.registry.states.stone);
    equip(
        &mut state,
        actor,
        "minecraft:cow_spawn_egg",
        InteractionHand::MainHand,
        1,
    );
    packet(
        &mut state,
        &mut decoder,
        click([9, 201, 8], Direction::East, InteractionHand::MainHand),
    );
    let cows = mobs(&state, "minecraft:cow");
    assert_eq!(cows.len(), 1);
    assert_walking_spawn_cell(&state, cows[0], [10.5, 201.0, 8.5]);
}

#[test]
fn spawn_egg_obstructed_body_does_not_debit_or_spawn() {
    let (mut state, mut decoder, actor) = fixture();
    state
        .world
        .set_block(10, 202, 8, state.registry.states.stone);
    equip(
        &mut state,
        actor,
        "minecraft:villager_spawn_egg",
        InteractionHand::MainHand,
        1,
    );
    let before = state.players[&actor].inventory.slots[0].clone();
    packet(
        &mut state,
        &mut decoder,
        click([10, 200, 8], Direction::Up, InteractionHand::MainHand),
    );
    assert!(mobs(&state, "minecraft:villager").is_empty());
    assert_eq!(state.players[&actor].inventory.slots[0], before);
}

fn entity_data_egg(state: &TickState, id: Option<&str>) -> ItemStack {
    let name = if id.is_some() {
        "minecraft:entity_data"
    } else {
        "!minecraft:entity_data"
    };
    let value = Tag::Compound(
        id.map(|id| vec![("id".into(), Tag::String(id.into()))])
            .unwrap_or_default(),
    );
    mc_data::item_nbt::stack_from_fields(
        &state.registry.items,
        &[
            (
                "id".into(),
                Tag::String("minecraft:villager_spawn_egg".into()),
            ),
            ("count".into(), Tag::Int(1)),
            (
                "components".into(),
                Tag::Compound(vec![(name.into(), value)]),
            ),
        ],
    )
    .unwrap()
    .unwrap()
}

#[test]
fn spawn_egg_entity_data_override_selects_registered_cow_not_item_filename() {
    let (mut state, mut decoder, actor) = fixture();
    let egg = entity_data_egg(&state, Some("minecraft:cow"));
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = egg;
    packet(
        &mut state,
        &mut decoder,
        click([10, 200, 8], Direction::Up, InteractionHand::MainHand),
    );
    assert_eq!(mobs(&state, "minecraft:cow").len(), 1);
    assert!(mobs(&state, "minecraft:villager").is_empty());
    assert!(state.players[&actor].inventory.slots[0].is_empty());
}

#[test]
fn spawn_egg_removed_or_unknown_entity_data_fails_closed_without_debit() {
    for id in [None, Some("minecraft:not_a_mob")] {
        let (mut state, mut decoder, actor) = fixture();
        let egg = entity_data_egg(&state, id);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = egg.clone();
        packet(
            &mut state,
            &mut decoder,
            click([10, 200, 8], Direction::Up, InteractionHand::MainHand),
        );
        assert!(mobs(&state, "minecraft:villager").is_empty());
        assert_eq!(state.players[&actor].inventory.slots[0], egg);
    }
}

#[test]
fn spawn_egg_spectator_and_peaceful_monster_refuse_without_debit() {
    for spectator in [false, true] {
        let (mut state, mut decoder, actor) = fixture();
        let name = if spectator {
            "minecraft:villager_spawn_egg"
        } else {
            "minecraft:zombie_spawn_egg"
        };
        if spectator {
            state.players.get_mut(&actor).unwrap().game_mode = GameMode::Spectator;
        } else {
            state.difficulty = mc_core::survival::Difficulty::Peaceful;
        }
        equip(&mut state, actor, name, InteractionHand::MainHand, 1);
        let before = state.players[&actor].inventory.slots[0].clone();
        let output = packet(
            &mut state,
            &mut decoder,
            click([10, 200, 8], Direction::Up, InteractionHand::MainHand),
        );
        assert!(output.changes.entities_spawned.is_empty());
        assert_eq!(state.players[&actor].inventory.slots[0], before);
    }
}
