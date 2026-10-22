//! Plugin-authored block tools through the real tick: a chisel-style tool turns a
//! full block into a slab (top/bottom by face) or stairs (facing the clicked side),
//! stairs into a slab, conserving material and wearing like any durable tool.

use std::sync::Arc;

use mc_data::block_tools::{BlockToolRule, parse_block_tool_rules};
use mc_data::item_components::{CustomItemDefinition, ItemFacts};
use mc_data::{Identifier, ItemStack};

use crate::block_tools::BlockToolTable;
use crate::boundary::{BlockPos, ConnectionId, EntityId, Hand, PlayerInput};
use crate::farming_tests::{Y, block, put, setup};
use crate::tick::{TickState, run_tick};

const TOOL: &str = "chisel:chisel";
const OWNER: &str = "chisel";

fn id(name: &str) -> Identifier {
    Identifier::parse(name).expect("identifier")
}

fn rules() -> Vec<BlockToolRule> {
    let rule = |face: &str, result: &str, set: &str, give: &str| {
        format!(
            r#"{{"item":"{TOOL}","block":"minecraft:stone_bricks","face":"{face}",
                "result":"minecraft:{result}","set":{set},"give":{give}}}"#
        )
    };
    let slab = "minecraft:stone_brick_slab";
    let definitions = vec![
        rule(
            "up",
            "stone_brick_slab",
            r#"{"type":"top"}"#,
            &format!("\"{slab}\""),
        ),
        rule(
            "down",
            "stone_brick_slab",
            r#"{"type":"bottom"}"#,
            &format!("\"{slab}\""),
        ),
        rule(
            "side",
            "stone_brick_stairs",
            r#"{"facing":"$face"}"#,
            "null",
        ),
        format!(
            r#"{{"item":"{TOOL}","block":"minecraft:stone_brick_stairs","face":"any",
                "result":"minecraft:stone_brick_slab",
                "set":{{"type":"@half","waterlogged":"@waterlogged"}}}}"#
        ),
    ];
    parse_block_tool_rules(OWNER, &definitions).expect("rules parse")
}

/// A survival player on the farming platform holding a fresh chisel in slot 0.
fn chisel_setup() -> (TickState, EntityId) {
    let (mut state, player) = setup();
    {
        let registry = Arc::make_mut(&mut state.registry);
        let facts = std::mem::take(&mut registry.item_facts)
            .with_custom_items(
                [CustomItemDefinition {
                    id: id(TOOL),
                    carrier: id("minecraft:stick"),
                    name: "Chisel".into(),
                    crafting_ingredient: None,
                    facts: ItemFacts {
                        max_stack_size: Some(1),
                        max_damage: Some(250),
                        ..ItemFacts::default()
                    },
                }],
                &registry.items,
            )
            .expect("custom chisel");
        registry.item_facts = facts;
        registry.block_tools =
            BlockToolTable::admit(&registry.blocks, rules()).expect("rules admitted");
    }
    let stick = state.registry.items.id_of(&id("minecraft:stick")).unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] =
        ItemStack::new(stick, 1).with_item_model(id(TOOL));
    (state, player)
}

fn use_on(state: &mut TickState, x: i32, y: i32, z: i32, face: u8) {
    let _ = run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: BlockPos { x, y, z },
            face,
            cursor: [0.5, 0.5, 0.5],
            sequence: 1,
        }],
    );
}

fn at(state: &TickState, x: i32, y: i32, z: i32) -> u32 {
    state.world.block_at(x, y, z).expect("resident")
}

fn count_of(state: &TickState, player: EntityId, name: &str) -> i32 {
    let item = state.registry.items.id_of(&id(name)).unwrap();
    state.players[&player]
        .inventory
        .slots
        .iter()
        .filter(|stack| !stack.is_empty() && stack.item_id == item)
        .map(|stack| stack.count)
        .sum()
}

fn wear(state: &TickState, player: EntityId) -> Option<i32> {
    state.players[&player].inventory.slots[0].damage
}

const SLAB: &str = "minecraft:stone_brick_slab";

#[test]
fn chisel_converts_blocks_conserves_material_and_wears() {
    let (mut state, player) = chisel_setup();
    let bricks = block(&state, "stone_bricks", &[]);
    for x in 1..=4 {
        put(&mut state, x, Y + 1, 0, bricks);
    }
    let top = block(
        &state,
        "stone_brick_slab",
        &[("type", "top"), ("waterlogged", "false")],
    );
    let bottom = block(
        &state,
        "stone_brick_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );

    // Top face -> top slab, one slab item conserved, one point of wear.
    use_on(&mut state, 1, Y + 1, 0, 1);
    assert_eq!(at(&state, 1, Y + 1, 0), top);
    assert_eq!(count_of(&state, player, SLAB), 1);
    assert_eq!(wear(&state, player), Some(1));

    // Bottom face -> bottom slab, a second slab item.
    use_on(&mut state, 2, Y + 1, 0, 0);
    assert_eq!(at(&state, 2, Y + 1, 0), bottom);
    assert_eq!(count_of(&state, player, SLAB), 2);
    assert_eq!(wear(&state, player), Some(2));

    // A side face -> stairs facing that side, nothing granted.
    use_on(&mut state, 3, Y + 1, 0, 5);
    let east_stairs = block(
        &state,
        "stone_brick_stairs",
        &[
            ("facing", "east"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    assert_eq!(at(&state, 3, Y + 1, 0), east_stairs);
    assert_eq!(count_of(&state, player, SLAB), 2);
    assert_eq!(wear(&state, player), Some(3));

    // Stairs -> slab keeping the half, nothing granted.
    use_on(&mut state, 3, Y + 1, 0, 1);
    assert_eq!(at(&state, 3, Y + 1, 0), bottom);
    assert_eq!(count_of(&state, player, SLAB), 2);
    assert_eq!(wear(&state, player), Some(4));

    // A slab has no rule: nothing changes and the tool is not worn.
    use_on(&mut state, 3, Y + 1, 0, 1);
    assert_eq!(at(&state, 3, Y + 1, 0), bottom);
    assert_eq!(wear(&state, player), Some(4));

    // A plain stick (no model) is not the tool.
    let stick = state.registry.items.id_of(&id("minecraft:stick")).unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = ItemStack::new(stick, 1);
    use_on(&mut state, 4, Y + 1, 0, 1);
    assert_eq!(at(&state, 4, Y + 1, 0), bricks);
}

#[test]
fn full_inventory_drops_the_conserved_slab_and_creative_pays_nothing() {
    let (mut state, player) = chisel_setup();
    let bricks = block(&state, "stone_bricks", &[]);
    put(&mut state, 1, Y + 1, 0, bricks);
    put(&mut state, 2, Y + 1, 0, bricks);
    let dirt = state.registry.items.id_of(&id("minecraft:dirt")).unwrap();
    for slot in 1..state.players[&player].inventory.slots.len() {
        state.players.get_mut(&player).unwrap().inventory.slots[slot] = ItemStack::new(dirt, 64);
    }
    use_on(&mut state, 1, Y + 1, 0, 1);
    assert_eq!(
        count_of(&state, player, SLAB),
        0,
        "no room in the inventory"
    );
    let slab = state.registry.items.id_of(&id(SLAB)).unwrap();
    let dropped = state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .filter(|stack| stack.item_id == slab)
        .map(|stack| stack.count)
        .sum::<i32>();
    assert_eq!(
        dropped, 1,
        "the slab drops at the player instead of being lost"
    );

    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
    use_on(&mut state, 2, Y + 1, 0, 1);
    assert_eq!(wear(&state, player), Some(1), "creative pays no wear");
}

#[test]
fn admission_refuses_rules_that_cannot_resolve() {
    let (state, _) = setup();
    let blocks = &state.registry.blocks;
    let bad = |json: &str| {
        let parsed = parse_block_tool_rules(OWNER, &[json.to_owned()]).expect("parses");
        BlockToolTable::admit(blocks, parsed)
    };
    let head = format!(r#"{{"item":"{TOOL}","block":"minecraft:stone_bricks","face":"side","#);
    assert!(bad(&format!(r#"{head}"result":"minecraft:not_a_block"}}"#)).is_err());
    assert!(
        bad(&format!(
            r#"{head}"result":"minecraft:stone_brick_slab","set":{{"facing":"$face"}}}}"#
        ))
        .is_err(),
        "a property the result lacks"
    );
    assert!(
        bad(&format!(
            r#"{head}"result":"minecraft:stone_brick_slab","set":{{"type":"$face"}}}}"#
        ))
        .is_err(),
        "a clicked face is not a slab type"
    );
    assert!(
        bad(&format!(
            r#"{head}"result":"minecraft:stone_brick_slab","set":{{"type":"@missing"}}}}"#
        ))
        .is_err(),
        "a copied property the source lacks"
    );
}
