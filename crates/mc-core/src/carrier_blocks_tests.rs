//! Carrier-backed custom blocks (ADR 0018): admission, placement, persistence as
//! the carrier state, and the custom-item drop.

use std::sync::Arc;

use mc_data::item_components::{CustomItemDefinition, ItemFacts};
use mc_data::{Identifier, ItemStack};
use mc_loader::CarrierBlockDeclaration;

use crate::boundary::{BlockPos, ConnectionId, EntityId, Hand, PlayerInput};
use crate::carrier_blocks::CarrierBlocks;
use crate::farming_tests::{Y, block, put, setup};
use crate::tick::{TickState, run_tick};

const MONITOR: &str = "solaris:monitor";
const CARRIER: &str = "minecraft:black_glazed_terracotta";

fn id(name: &str) -> Identifier {
    Identifier::parse(name).expect("identifier")
}

fn declaration(carrier: &str, drop_item: &str) -> CarrierBlockDeclaration {
    CarrierBlockDeclaration {
        id: MONITOR.into(),
        name: "Monitor".into(),
        model: "solaris:block/monitor".into(),
        carrier: carrier.into(),
        drop_item: drop_item.into(),
    }
}

fn monitor_setup() -> (TickState, EntityId) {
    let (mut state, player) = setup();
    {
        let registry = Arc::make_mut(&mut state.registry);
        registry.item_facts = std::mem::take(&mut registry.item_facts)
            .with_custom_items(
                [CustomItemDefinition {
                    id: id(MONITOR),
                    carrier: id(CARRIER),
                    name: "Monitor".into(),
                    crafting_ingredient: None,
                    facts: ItemFacts {
                        max_stack_size: Some(64),
                        ..ItemFacts::default()
                    },
                }],
                &registry.items,
            )
            .expect("custom monitor item");
        registry.carrier_blocks = CarrierBlocks::admit(
            &[declaration(CARRIER, MONITOR)],
            &registry.blocks,
            &registry.item_facts,
        )
        .expect("monitor admitted");
    }
    (state, player)
}

fn monitor_stack(state: &TickState) -> ItemStack {
    crate::carrier_blocks::custom_stack(
        &state.registry.item_facts,
        &state.registry.items,
        &id(MONITOR),
    )
    .expect("monitor stack")
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

#[test]
fn only_a_reserved_horizontal_carrier_with_an_installed_drop_item_is_admitted() {
    let (state, _) = monitor_setup();
    let registry = &state.registry;
    for (carrier, drop_item) in [
        ("minecraft:stone", MONITOR),
        ("minecraft:chain_command_block", MONITOR),
        (CARRIER, "solaris:absent"),
    ] {
        assert!(
            CarrierBlocks::admit(
                &[declaration(carrier, drop_item)],
                &registry.blocks,
                &registry.item_facts
            )
            .is_err(),
            "{carrier} {drop_item}"
        );
    }
}

#[test]
fn custom_item_places_a_carrier_state_that_drops_the_custom_item() {
    let (mut state, player) = monitor_setup();
    let stone = block(&state, "stone", &[]);
    put(&mut state, 1, Y + 1, 0, stone);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = monitor_stack(&state);

    use_on(&mut state, 1, Y + 1, 0, 1);
    let placed = state.world.block_at(1, Y + 2, 0).expect("resident");
    assert_eq!(
        state.registry.carrier_block_id(placed),
        Some(&id(MONITOR)),
        "the persisted state is a reserved carrier state"
    );
    assert_eq!(
        placed,
        block(&state, "black_glazed_terracotta", &[("facing", "north")])
    );
    // Survival break rolls the custom item, not the carrier's vanilla loot.
    let drops = crate::block_drop_context::roll(
        &state,
        BlockPos {
            x: 1,
            y: Y + 2,
            z: 0,
        },
        placed,
        &ItemStack::EMPTY,
        0,
    )
    .expect("drops");
    assert_eq!(drops, vec![monitor_stack(&state)]);
}

#[test]
fn a_plain_carrier_item_cannot_place_the_reserved_block() {
    let (mut state, player) = monitor_setup();
    let stone = block(&state, "stone", &[]);
    put(&mut state, 1, Y + 1, 0, stone);
    let plain = state
        .registry
        .items
        .id_of(&id(CARRIER))
        .expect("carrier item");
    state.players.get_mut(&player).unwrap().inventory.slots[0] = ItemStack::new(plain, 1);

    use_on(&mut state, 1, Y + 1, 0, 1);
    assert_eq!(
        state.world.block_at(1, Y + 2, 0),
        Some(state.registry.states.air)
    );
}
