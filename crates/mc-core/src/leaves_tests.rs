use super::{FAST, Y, at, block, plant, put, run, setup};
use crate::boundary::BlockPos;
use crate::leaves;

fn pos(x: i32, z: i32) -> BlockPos {
    BlockPos { x, y: Y + 1, z }
}
fn foliage(state: &mut crate::tick::TickState, x: i32, z: i32, distance: &str, persistent: &str) {
    plant(
        state,
        x,
        Y + 1,
        z,
        "oak_leaves",
        &[
            ("distance", distance),
            ("persistent", persistent),
            ("waterlogged", "false"),
        ],
    );
}

#[test]
fn distance_propagates_both_log_attachment_and_removal() {
    let (mut state, _) = setup();
    plant(&mut state, 2, Y + 1, 2, "oak_log", &[("axis", "y")]);
    for x in 3..=8 {
        foliage(&mut state, x, 2, "7", "false");
    }
    let mut writes = Vec::new();
    for x in 3..=8 {
        assert!(leaves::scheduled(&mut state, pos(x, 2), &mut writes));
    }
    for x in 3..=8 {
        assert_eq!(leaves::distance(&state, pos(x, 2)), Some((x - 2) as u8));
        assert_eq!(
            crate::farming::prop(&state.registry.blocks, at(&state, x, Y + 1, 2), "distance"),
            Some((x - 2).to_string().as_str())
        );
        assert!(!leaves::decays(&state, pos(x, 2), at(&state, x, Y + 1, 2)));
    }
    let air = state.registry.states.air;
    put(&mut state, 2, Y + 1, 2, air);
    for _ in 0..7 {
        for x in 3..=8 {
            leaves::scheduled(&mut state, pos(x, 2), &mut writes);
        }
    }
    for x in 3..=8 {
        assert!(leaves::decays(&state, pos(x, 2), at(&state, x, Y + 1, 2)));
    }
}

#[test]
fn persistent_foliage_does_not_decay_and_new_log_prevents_stale_decay() {
    let (mut state, _) = setup();
    foliage(&mut state, 2, 2, "7", "true");
    assert!(!leaves::decays(&state, pos(2, 2), at(&state, 2, Y + 1, 2)));
    foliage(&mut state, 2, 2, "7", "false");
    assert!(leaves::decays(&state, pos(2, 2), at(&state, 2, Y + 1, 2)));
    plant(&mut state, 3, Y + 1, 2, "oak_log", &[("axis", "y")]);
    assert!(!leaves::decays(&state, pos(2, 2), at(&state, 2, Y + 1, 2)));
    leaves::scheduled(&mut state, pos(2, 2), &mut Vec::new());
    assert_eq!(
        crate::farming::prop(&state.registry.blocks, at(&state, 2, Y + 1, 2), "distance"),
        Some("1")
    );
}

#[test]
fn unloaded_leaf_input_defers_without_destroying_or_consuming_identity() {
    let (mut state, _) = setup();
    let want = [(crate::boundary::ChunkPos { x: 10, z: 10 }, 0)]
        .into_iter()
        .collect();
    state.world.ensure_blocking(&want);
    foliage(&mut state, 175, 175, "7", "false");
    let at = pos(175, 175);
    let id = state.world.block_at(at.x, at.y, at.z).unwrap();
    assert_eq!(leaves::distance(&state, at), None);
    assert!(!leaves::decays(&state, at, id));
    assert!(leaves::scheduled(&mut state, at, &mut Vec::new()));
    assert_eq!(state.world.block_at(at.x, at.y, at.z), Some(id));
    let name = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .unwrap()
        .block
        .id
        .clone();
    assert!(state.block_ticks.contains(crate::plants::old(at), &name, 0));
    let image = state.block_ticks.image();
    let restored = crate::block_ticks::BlockTicks::from_image(image.clone(), None);
    assert_eq!(
        restored.image(),
        image,
        "absolute pending identity survives checkpoint image"
    );
}

#[test]
fn natural_loot_family_chances_are_deterministic_and_never_drop_leaf_blocks() {
    let (state, _) = setup();
    for (family, sapling, apple) in [
        ("oak", "oak_sapling", true),
        ("jungle", "jungle_sapling", false),
        ("birch", "birch_sapling", false),
        ("dark_oak", "dark_oak_sapling", true),
        ("azalea", "azalea", false),
        ("spruce", "spruce_sapling", false),
        ("acacia", "acacia_sapling", false),
        ("cherry", "cherry_sapling", false),
        ("pale_oak", "pale_oak_sapling", false),
        ("flowering_azalea", "flowering_azalea", false),
    ] {
        let id = block(
            &state,
            &format!("{family}_leaves"),
            &[
                ("distance", "7"),
                ("persistent", "false"),
                ("waterlogged", "false"),
            ],
        );
        let mut saplings = 0;
        let mut sticks = 0;
        let mut apples = 0;
        for seed in 0..4096 {
            let drops = crate::block_drop_context::roll(
                &state,
                pos(2, 2),
                id,
                &mc_data::ItemStack::EMPTY,
                seed,
            )
            .unwrap();
            assert_eq!(
                drops,
                crate::block_drop_context::roll(
                    &state,
                    pos(2, 2),
                    id,
                    &mc_data::ItemStack::EMPTY,
                    seed,
                )
                .unwrap()
            );
            for stack in drops {
                let name = state.registry.items.name_of(stack.item_id).unwrap().path();
                match name {
                    "stick" => {
                        sticks += 1;
                        assert!((1..=2).contains(&stack.count));
                    }
                    "apple" if apple => {
                        apples += 1;
                        assert_eq!(stack.count, 1);
                    }
                    name if name == sapling => {
                        saplings += 1;
                        assert_eq!(stack.count, 1);
                    }
                    _ => panic!("unexpected natural leaf drop {name}"),
                }
            }
        }
        assert!(saplings > 0 && sticks > 0);
        assert_eq!(apples > 0, apple);
    }
}

#[test]
fn random_leaf_decay_and_block_drop_rule_use_native_world() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    state.gamerules.insert("block_drops".into(), "false".into());
    foliage(&mut state, 2, 2, "7", "false");
    foliage(&mut state, 3, 3, "7", "true");
    run(&mut state, 20);
    assert_eq!(at(&state, 2, Y + 1, 2), state.registry.states.air);
    let persistent = at(&state, 3, Y + 1, 3);
    assert_eq!(
        crate::farming::prop(&state.registry.blocks, persistent, "persistent"),
        Some("true")
    );
    assert!(
        state
            .entities
            .slots()
            .all(|slot| state.entities.row(slot).item.is_none())
    );
}

#[test]
fn native_neighbor_ticks_recompute_removed_logs_then_decay_with_table_loot() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), "0".into());
    plant(&mut state, 2, Y + 1, 2, "oak_log", &[("axis", "y")]);
    for x in 3..=8 {
        foliage(&mut state, x, 2, "7", "false");
    }
    crate::plants::neighbor_updates(&mut state, pos(2, 2), &mut Vec::new(), &mut Vec::new());
    run(&mut state, 8);
    for x in 3..=8 {
        assert_eq!(
            crate::farming::prop(&state.registry.blocks, at(&state, x, Y + 1, 2), "distance"),
            Some((x - 2).to_string().as_str()),
        );
    }
    let air = state.registry.states.air;
    put(&mut state, 2, Y + 1, 2, air);
    crate::plants::neighbor_updates(&mut state, pos(2, 2), &mut Vec::new(), &mut Vec::new());
    run(&mut state, 14);
    for x in 3..=8 {
        assert_eq!(
            crate::farming::prop(&state.registry.blocks, at(&state, x, Y + 1, 2), "distance"),
            Some("7"),
        );
    }
    // A large canopy gives native random loot enough independent samples;
    // no seed special-case and no direct call into the decay helper.
    for x in -9..=9 {
        for z in -9..=9 {
            foliage(&mut state, x, z, "7", "false");
        }
    }
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    run(&mut state, 20);
    for x in -9..=9 {
        for z in -9..=9 {
            assert_eq!(at(&state, x, Y + 1, z), air);
        }
    }
    let names: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| {
            state
                .entities
                .row(slot)
                .item
                .as_ref()
                .and_then(|item| state.registry.items.name_of(item.item_id))
                .map(|id| id.path())
        })
        .collect();
    assert!(names.contains(&"oak_sapling"), "{names:?}");
    assert!(names.contains(&"stick"), "{names:?}");
    assert!(!names.contains(&"oak_leaves"));
}

#[test]
fn player_placement_marks_foliage_persistent_before_random_decay() {
    let (mut state, player) = setup();
    super::give(&mut state, player, "oak_leaves", 2);
    super::use_on(&mut state, 1, Y, 0, 1);
    let id = at(&state, 1, Y + 1, 0);
    assert_eq!(
        crate::farming::prop(&state.registry.blocks, id, "persistent"),
        Some("true")
    );
    assert_eq!(super::held(&state, player).count, 1);
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    run(&mut state, 20);
    assert_eq!(at(&state, 1, Y + 1, 0), id);
}

#[test]
fn leaf_tool_loot_uses_shears_silk_touch_and_fortune_tables() {
    let (state, _) = setup();
    let id = block(
        &state,
        "oak_leaves",
        &[
            ("distance", "7"),
            ("persistent", "false"),
            ("waterlogged", "false"),
        ],
    );
    let item = |name: &str| {
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap()
    };
    let shears = mc_data::ItemStack::new(item("shears"), 1);
    let mut silk = mc_data::ItemStack::new(item("diamond_hoe"), 1);
    silk.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:silk_touch").unwrap(),
        level: 1,
    });
    for tool in [shears, silk] {
        let drops = crate::block_drop_context::roll(&state, pos(2, 2), id, &tool, 42).unwrap();
        assert_eq!(drops, vec![mc_data::ItemStack::new(item("oak_leaves"), 1)]);
    }
    let mut fortune = mc_data::ItemStack::new(item("diamond_hoe"), 1);
    fortune.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:fortune").unwrap(),
        level: 3,
    });
    let count = |tool: &mc_data::ItemStack| {
        (0..2048)
            .map(|seed| {
                crate::block_drop_context::roll(&state, pos(2, 2), id, tool, seed)
                    .unwrap()
                    .iter()
                    .filter(|stack| stack.item_id == item("oak_sapling"))
                    .map(|stack| stack.count)
                    .sum::<i32>()
            })
            .sum::<i32>()
    };
    assert!(count(&fortune) > count(&mc_data::ItemStack::EMPTY));
}
