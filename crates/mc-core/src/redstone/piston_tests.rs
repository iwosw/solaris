use super::*;
use crate::journal::{
    JournalEntry, JournalWriter, Replay, read_complete_batches_after, replay_batches,
};

fn cell(x: i32, z: i32) -> BlockPos {
    BlockPos { x, y: 200, z }
}

fn name(s: &TickState, pos: BlockPos) -> &str {
    at(s, pos).unwrap().block.id.path()
}

fn put(s: &mut TickState, pos: BlockPos, path: &str) {
    place(s, pos.x, pos.y, pos.z, path, &[]);
}

fn state_with_loot() -> TickState {
    let mut s = state();
    Arc::get_mut(&mut s.registry).unwrap().loot = mc_data::loot::load_vanilla_subset(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/vanilla/data/minecraft/loot_table"),
    )
    .unwrap();
    s
}

fn move_now(s: &mut TickState, pos: BlockPos, powered: bool) -> Result<(), piston::Refusal> {
    let edits = {
        let read = Read(s);
        let mut plan = planning::Settle::new(&s.registry.blocks, &read);
        let state = at(s, pos).unwrap();
        piston::plan_move(
            &mut plan,
            pos,
            state,
            state.block.id.path() == "sticky_piston",
            powered,
        )?;
        plan.into_edits()
    };
    piston::apply_edits(s, &mut Vec::new(), &mut Vec::new(), edits);
    s.world.fold_overlay();
    Ok(())
}

#[test]
fn protected_piston_footprints_refuse_push_pull_head_and_destroy_atomically() {
    for (pull, fragile, protected) in [
        (false, false, cell(4, 4)),
        (false, false, cell(5, 4)),
        (false, false, cell(6, 4)),
        (false, true, cell(5, 4)),
        (true, false, cell(4, 4)),
        (true, false, cell(5, 4)),
        (true, false, cell(6, 4)),
    ] {
        let mut s = state();
        let base = place(&mut s, 4, 200, 4, "sticky_piston", &[("facing", "east")]);
        put(&mut s, cell(5, 4), if fragile { "torch" } else { "stone" });
        if pull {
            move_now(&mut s, base, true).unwrap();
        }
        s.plugins.set_domain(
            "towns",
            "player-zones",
            serde_json::json!({
                "claim": {
                    "dimension": "minecraft:overworld",
                    "minimum": [protected.x, protected.y, protected.z],
                    "maximum": [protected.x, protected.y, protected.z],
                    "allowed_actors": [uuid::Uuid::nil().to_string()],
                    "public_use": true
                }
            }),
        );
        crate::plugin_player::restore(&mut s).unwrap();
        assert!(crate::plugin_player::protects_cell(&s, protected));
        let cells = [base, cell(5, 4), cell(6, 4)];
        let before = cells.map(|pos| s.world.block_at(pos.x, pos.y, pos.z));
        assert_eq!(
            move_now(&mut s, base, !pull),
            Err(piston::Refusal::Protected(protected)),
            "pull={pull}, fragile={fragile}, protected={protected:?}"
        );
        assert_eq!(
            before,
            cells.map(|pos| s.world.block_at(pos.x, pos.y, pos.z))
        );
    }
}

#[test]
fn complete_26_1_2_registry_reactions_match_the_native_server_oracle() {
    let mut s = state();
    let base = cell(4, 4);
    let head = cell(5, 4);
    let destination = cell(6, 4);
    let mut count = 0;
    // Independently extracted from all vanilla states, not from the Rust
    // classifier. The only deliberate divergence is retained block-entity
    // custody: DESTROY item/opaque entities still refuse instead of spilling.
    for row in include_str!("piston_reactions_26_1_2.tsv").lines() {
        if row.starts_with('#') {
            continue;
        }
        let mut fields = row.split('\t');
        let path = fields.next().unwrap();
        let reaction = fields.next().unwrap();
        let block_entity = fields.next().unwrap() == "true";
        let negative_hardness = fields.next().unwrap() == "true";
        assert!(fields.next().is_none());
        place(
            &mut s,
            base.x,
            base.y,
            base.z,
            "piston",
            &[("facing", "east")],
        );
        put(&mut s, head, path);
        put(&mut s, destination, "air");
        let unextended_piston = matches!(path, "piston" | "sticky_piston");
        let blocked = (block_entity && path != "comparator")
            || negative_hardness
            || (reaction == "BLOCK" && !unextended_piston)
            || matches!(
                path,
                "obsidian" | "crying_obsidian" | "respawn_anchor" | "reinforced_deepslate"
            );
        let result = move_now(&mut s, base, true);
        if blocked {
            assert_eq!(result, Err(piston::Refusal::Immovable(head)), "{path}");
            assert_eq!(value(&s, base, "extended"), "false", "{path}");
            assert_eq!(name(&s, head), path);
        } else {
            assert_eq!(result, Ok(()), "{path}");
            assert_eq!(name(&s, head), "piston_head", "{path}");
            let expected =
                if reaction == "DESTROY" || matches!(path, "air" | "cave_air" | "void_air") {
                    "air"
                } else {
                    path
                };
            assert_eq!(name(&s, destination), expected, "{path}");
        }
        count += 1;
    }
    assert_eq!(count, 1168);
}

#[test]
fn slime_and_honey_lines_push_and_pull_attached_blocks_in_all_six_directions() {
    for sticky in ["slime_block", "honey_block"] {
        for (facing, direction, side) in [
            ("east", (1, 0, 0), (0, 0, 1)),
            ("west", (-1, 0, 0), (0, 0, 1)),
            ("north", (0, 0, -1), (1, 0, 0)),
            ("south", (0, 0, 1), (1, 0, 0)),
            ("up", (0, 1, 0), (1, 0, 0)),
            ("down", (0, -1, 0), (1, 0, 0)),
        ] {
            let mut s = state();
            let base = place(&mut s, 7, 200, 7, "sticky_piston", &[("facing", facing)]);
            let head = offset(base, direction).unwrap();
            let middle = offset(head, direction).unwrap();
            let end = offset(middle, direction).unwrap();
            let branch = offset(head, side).unwrap();
            let back = offset(branch, (-direction.0, -direction.1, -direction.2)).unwrap();
            for pos in [head, middle, end, branch] {
                put(&mut s, pos, sticky);
            }
            put(&mut s, back, "stone");
            let original = [head, middle, end, branch, back];
            move_now(&mut s, base, true).unwrap();
            assert_eq!(name(&s, head), "piston_head");
            for (index, pos) in original.iter().enumerate() {
                assert_eq!(
                    name(&s, offset(*pos, direction).unwrap()),
                    if index == 4 { "stone" } else { sticky },
                    "{sticky} {facing} {pos:?}"
                );
            }
            move_now(&mut s, base, false).unwrap();
            for (index, pos) in original.iter().enumerate() {
                assert_eq!(name(&s, *pos), if index == 4 { "stone" } else { sticky });
            }
            assert_eq!(name(&s, offset(end, direction).unwrap()), "air");
            assert_eq!(name(&s, offset(branch, direction).unwrap()), "air");
        }
    }
}

#[test]
fn honey_and_slime_separate_and_glazed_terracotta_does_not_adhere() {
    for (sticky, other) in [
        ("slime_block", "honey_block"),
        ("honey_block", "slime_block"),
    ] {
        let mut s = state();
        let base = place(&mut s, 4, 200, 4, "sticky_piston", &[("facing", "east")]);
        put(&mut s, cell(5, 4), sticky);
        put(&mut s, cell(5, 5), other);
        put(&mut s, cell(5, 3), "blue_glazed_terracotta");
        put(&mut s, cell(5, 6), "stone");
        put(&mut s, cell(6, 6), "stone");
        move_now(&mut s, base, true).unwrap();
        assert_eq!(name(&s, cell(6, 4)), sticky);
        assert_eq!(name(&s, cell(5, 5)), other);
        assert_eq!(name(&s, cell(5, 3)), "blue_glazed_terracotta");
        // A different adhesive remains detached even on the return stroke.
        put(&mut s, cell(6, 5), other);
        move_now(&mut s, base, false).unwrap();
        assert_eq!(name(&s, cell(5, 4)), sticky);
        assert_eq!(name(&s, cell(6, 5)), other);
        assert_eq!(name(&s, cell(5, 3)), "blue_glazed_terracotta");
    }
}

#[test]
fn incompatible_and_glazed_blocks_still_move_when_collided_with_but_glazed_is_not_pulled() {
    for path in ["honey_block", "blue_glazed_terracotta"] {
        let mut s = state();
        let base = place(&mut s, 4, 200, 4, "sticky_piston", &[("facing", "east")]);
        put(&mut s, cell(5, 4), "slime_block");
        put(&mut s, cell(6, 4), path);
        move_now(&mut s, base, true).unwrap();
        assert_eq!(name(&s, cell(6, 4)), "slime_block");
        assert_eq!(name(&s, cell(7, 4)), path);
    }
    let mut s = state();
    let base = place(&mut s, 4, 200, 4, "sticky_piston", &[("facing", "east")]);
    put(&mut s, cell(5, 4), "blue_glazed_terracotta");
    move_now(&mut s, base, true).unwrap();
    move_now(&mut s, base, false).unwrap();
    assert_eq!(name(&s, cell(5, 4)), "air");
    assert_eq!(name(&s, cell(6, 4)), "blue_glazed_terracotta");
}

#[test]
fn attached_structure_shares_twelve_block_limit_and_refuses_thirteen_atomically() {
    for count in [12, 13] {
        let mut s = state();
        let base = place(&mut s, 4, 200, 1, "piston", &[("facing", "east")]);
        for z in 1..=count {
            put(&mut s, cell(5, z), "slime_block");
        }
        let result = move_now(&mut s, base, true);
        if count == 12 {
            result.unwrap();
            assert_eq!(value(&s, base, "extended"), "true");
            for z in 1..=count {
                assert_eq!(name(&s, cell(6, z)), "slime_block");
                if z != 1 {
                    assert_eq!(name(&s, cell(5, z)), "air");
                }
            }
        } else {
            assert_eq!(result, Err(piston::Refusal::PushLimit));
            assert_eq!(value(&s, base, "extended"), "false");
            for z in 1..=count {
                assert_eq!(name(&s, cell(5, z)), "slime_block");
                assert_eq!(name(&s, cell(6, z)), "air");
            }
        }
    }
}

#[test]
fn failed_sticky_pull_retracts_without_tearing_or_destroying_the_structure() {
    for obstruction in ["obsidian", "bedrock", "chest"] {
        let mut s = state();
        let base = place(&mut s, 4, 200, 4, "sticky_piston", &[("facing", "east")]);
        put(&mut s, cell(5, 4), "slime_block");
        put(&mut s, cell(5, 5), "stone");
        move_now(&mut s, base, true).unwrap();
        put(&mut s, cell(5, 5), obstruction);
        move_now(&mut s, base, false).unwrap();
        assert_eq!(value(&s, base, "extended"), "false");
        assert_eq!(name(&s, cell(5, 4)), "air");
        assert_eq!(name(&s, cell(6, 4)), "slime_block");
        assert_eq!(name(&s, cell(6, 5)), "stone");
        assert_eq!(name(&s, cell(5, 5)), obstruction);
    }
    let mut s = state();
    let base = place(
        &mut s,
        4,
        200,
        1,
        "sticky_piston",
        &[("facing", "east"), ("extended", "true")],
    );
    place(
        &mut s,
        5,
        200,
        1,
        "piston_head",
        &[("facing", "east"), ("type", "sticky")],
    );
    for z in 1..=13 {
        put(&mut s, cell(6, z), "slime_block");
    }
    move_now(&mut s, base, false).unwrap();
    assert_eq!(value(&s, base, "extended"), "false");
    assert_eq!(name(&s, cell(5, 1)), "air");
    for z in 1..=13 {
        assert_eq!(name(&s, cell(6, z)), "slime_block");
    }
}

#[test]
fn branch_destination_immovable_refuses_push_but_immovable_sides_do_not_adhere() {
    for obstruction in ["obsidian", "bedrock", "chest"] {
        let mut s = state();
        let base = place(&mut s, 4, 200, 4, "piston", &[("facing", "east")]);
        put(&mut s, cell(5, 4), "slime_block");
        put(&mut s, cell(5, 5), "stone");
        put(&mut s, cell(6, 5), obstruction);
        assert_eq!(
            move_now(&mut s, base, true),
            Err(piston::Refusal::Immovable(cell(6, 5)))
        );
        assert_eq!(value(&s, base, "extended"), "false");
        assert_eq!(name(&s, cell(5, 4)), "slime_block");
        assert_eq!(name(&s, cell(5, 5)), "stone");
        put(&mut s, cell(5, 5), obstruction);
        move_now(&mut s, base, true).unwrap();
        assert_eq!(name(&s, cell(6, 4)), "slime_block");
        assert_eq!(name(&s, cell(5, 5)), obstruction);
    }
}

#[test]
fn every_native_block_entity_identity_refuses_a_structure_destination() {
    use mc_data::block_entity_kinds::BlockEntityKind;
    let mut s = state();
    let base = place(&mut s, 4, 200, 4, "piston", &[("facing", "east")]);
    put(&mut s, cell(5, 4), "slime_block");
    put(&mut s, cell(5, 5), "stone");
    for kind in BlockEntityKind::ALL {
        // Comparators have vanilla DESTROY behaviour; their output metadata
        // does not contain movable item custody.
        if *kind == BlockEntityKind::Comparator {
            continue;
        }
        for path in kind.block_paths() {
            put(&mut s, cell(6, 5), path);
            assert_eq!(
                move_now(&mut s, base, true),
                Err(piston::Refusal::Immovable(cell(6, 5))),
                "{path}"
            );
            assert_eq!(name(&s, cell(6, 5)), *path);
            assert_eq!(name(&s, cell(5, 5)), "stone");
            assert_eq!(value(&s, base, "extended"), "false");
        }
    }
}

#[test]
fn cyclic_and_colliding_slime_branches_move_each_source_once() {
    let mut s = state();
    let base = place(&mut s, 4, 200, 4, "sticky_piston", &[("facing", "east")]);
    for x in 5..=7 {
        for z in 4..=6 {
            put(
                &mut s,
                cell(x, z),
                if x == 6 && z == 5 {
                    "stone"
                } else {
                    "slime_block"
                },
            );
        }
    }
    for powered in [true, false] {
        move_now(&mut s, base, powered).unwrap();
        let shift = i32::from(powered);
        for x in 5..=7 {
            for z in 4..=6 {
                assert_eq!(
                    name(&s, cell(x + shift, z)),
                    if x == 6 && z == 5 {
                        "stone"
                    } else {
                        "slime_block"
                    }
                );
            }
        }
    }
    for z in 4..=6 {
        assert_eq!(name(&s, cell(8, z)), "air");
    }
}

#[test]
fn fragile_destinations_pop_with_drops_and_do_not_join_adhesive_sides() {
    for fragile in [
        "redstone_wire",
        "redstone_torch",
        "lever",
        "comparator",
        "repeater",
        "dandelion",
        "moss_carpet",
    ] {
        let mut s = state_with_loot();
        let base = place(&mut s, 4, 200, 4, "piston", &[("facing", "east")]);
        put(&mut s, cell(5, 4), "slime_block");
        put(&mut s, cell(5, 5), "stone");
        put(&mut s, cell(6, 5), fragile);
        put(&mut s, cell(5, 3), fragile);
        move_now(&mut s, base, true).unwrap();
        assert_eq!(name(&s, cell(6, 5)), "stone");
        assert_eq!(name(&s, cell(5, 3)), fragile);
        let drops: Vec<_> = s
            .entities
            .slots()
            .filter_map(|slot| s.entities.row(slot).item.as_ref())
            .collect();
        assert_eq!(drops.len(), 1, "{fragile}");
        assert_eq!(drops[0].count, 1, "{fragile}");
    }
}

#[test]
fn java_pushable_attachments_are_not_treated_as_destroy_reactions() {
    for attachment in [
        "blue_carpet",
        "rail",
        "dead_tube_coral",
        "end_rod",
        "iron_chain",
    ] {
        let mut s = state();
        let base = place(&mut s, 4, 200, 4, "sticky_piston", &[("facing", "east")]);
        put(&mut s, cell(5, 4), "slime_block");
        put(&mut s, cell(5, 5), attachment);
        move_now(&mut s, base, true).unwrap();
        assert_eq!(name(&s, cell(6, 5)), attachment);
        move_now(&mut s, base, false).unwrap();
        assert_eq!(name(&s, cell(5, 5)), attachment);
        assert!(s.entities.is_empty());
    }
}

#[test]
fn adhesive_move_source_clears_destinations_and_destroyed_loot_share_tick_wal() {
    let mut s = state_with_loot();
    let base = place(&mut s, 4, 200, 4, "piston", &[("facing", "east")]);
    put(&mut s, cell(5, 4), "slime_block");
    put(&mut s, cell(5, 5), "stone");
    put(&mut s, cell(6, 5), "dandelion");
    put(&mut s, BlockPos { x: 6, y: 199, z: 5 }, "dirt");
    put(&mut s, BlockPos { x: 4, y: 199, z: 4 }, "redstone_block");
    wake(&mut s, base);
    let (output, _, _) = crate::tick::run_tick(&mut s, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut s, &output);
    let replay = replay_batches(Replay::default(), &[(output.tick, output.tick, entries)]);
    assert_eq!(replay.blocks[&(5, 200, 5)], s.registry.states.air);
    assert_eq!(replay.blocks[&(6, 200, 4)], block(&s, "slime_block", &[]));
    assert_eq!(replay.blocks[&(6, 200, 5)], block(&s, "stone", &[]));
    assert_eq!(replay.entities.len(), 1);
    let drop = replay.entities.values().next().unwrap();
    assert_eq!(drop.appearance.item.as_ref().unwrap().count, 1);
}

fn power_cycle_edge(s: &mut TickState, piston: BlockPos, on: bool) {
    let supply = offset(piston, (0, -1, 0)).unwrap();
    put(s, supply, if on { "redstone_block" } else { "air" });
    wake(s, piston);
    advance(s, 2);
    assert_eq!(
        value(s, piston, "extended"),
        if on { "true" } else { "false" }
    );
}

#[test]
fn two_piston_flying_machine_advances_exactly_one_block_per_four_stroke_cycle() {
    let mut s = state();
    // Normal rear engine leaves the chassis behind on retraction; the opposed
    // sticky engine then reaches back and pulls that unextended engine along.
    place(&mut s, 4, 200, 4, "piston", &[("facing", "east")]);
    place(&mut s, 6, 200, 5, "sticky_piston", &[("facing", "west")]);
    put(&mut s, cell(5, 4), "slime_block");
    put(&mut s, cell(5, 5), "slime_block");
    for cycle in 0..4 {
        let rear = cell(4 + cycle, 4);
        let front = cell(7 + cycle, 5);
        power_cycle_edge(&mut s, rear, true);
        assert_eq!(name(&s, front), "sticky_piston");
        power_cycle_edge(&mut s, rear, false);
        power_cycle_edge(&mut s, front, true);
        power_cycle_edge(&mut s, front, false);
        assert_eq!(name(&s, cell(5 + cycle, 4)), "piston");
        assert_eq!(name(&s, cell(6 + cycle, 4)), "slime_block");
        assert_eq!(name(&s, cell(6 + cycle, 5)), "slime_block");
        assert_eq!(name(&s, cell(7 + cycle, 5)), "sticky_piston");
        assert_eq!(name(&s, rear), "air");
    }
}

#[test]
fn queued_adhesive_piston_cycle_survives_native_checkpoint_and_wal_tail_replay() {
    let directory = tempfile::tempdir().unwrap();
    let mut original = state_in(Some(directory.path().to_owned()));
    let base = place(
        &mut original,
        4,
        200,
        4,
        "sticky_piston",
        &[("facing", "east")],
    );
    put(&mut original, cell(5, 4), "slime_block");
    put(&mut original, cell(5, 5), "stone");
    power_cycle_edge(&mut original, base, true);
    let checkpoint_tick = original.tick;
    let absent = original
        .block_ticks
        .checkpoint(&mut original.world, checkpoint_tick);
    let mut capture = original.world.capture_dirty(&Default::default());
    original
        .world
        .capture_scheduled_block_ticks(&mut capture, absent);
    capture.write().unwrap();
    let checkpoint_queue = original.block_ticks.image();
    // The tail removes power and queues a delayed return stroke. No animation
    // exists in this core: the durable mid-cycle state is the pending stroke.
    let supply = offset(base, (0, -1, 0)).unwrap();
    put(&mut original, supply, "air");
    schedule_cell(&mut original, base, 5, 0);
    let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
    let mut entries = crate::core::journal_entries_for_tick(&mut original, &output);
    entries.push(JournalEntry::Block {
        pos: [supply.x, supply.y, supply.z],
        state: original.registry.states.air,
    });
    let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
    writer.append(1, checkpoint_tick, &[]).unwrap();
    writer
        .append(checkpoint_tick + 1, output.tick, &entries)
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches =
        read_complete_batches_after(&directory.path().join("journal"), checkpoint_tick).unwrap();
    let replay = replay_batches(
        Replay {
            block_ticks: checkpoint_queue,
            tick: checkpoint_tick,
            ..Default::default()
        },
        &batches,
    );
    assert_eq!(value(&original, base, "extended"), "true");
    assert!(
        replay
            .block_ticks
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.ticks)
            .any(|tick| tick.pos == [base.x, base.y, base.z])
    );
    let expected_queue = replay.block_ticks.clone();
    let replay = replay_batches(replay, &batches);
    assert_eq!(replay.block_ticks, expected_queue);
    let mut restored = state_in(Some(directory.path().to_owned()));
    assert_eq!(name(&restored, cell(6, 4)), "slime_block");
    assert_eq!(name(&restored, cell(6, 5)), "stone");
    restored.tick = replay.tick;
    restored.block_ticks = crate::block_ticks::BlockTicks::from_image(replay.block_ticks, None);
    for ((x, y, z), state) in replay.blocks {
        restored.world.set_block(x, y, z, state);
    }
    restored.world.fold_overlay();
    advance(&mut original, 8);
    advance(&mut restored, 8);
    for pos in [base, cell(5, 4), cell(5, 5), cell(6, 4), cell(6, 5)] {
        assert_eq!(
            restored.world.block_at(pos.x, pos.y, pos.z),
            original.world.block_at(pos.x, pos.y, pos.z)
        );
    }
    assert_eq!(value(&restored, base, "extended"), "false");
    assert_eq!(name(&restored, cell(5, 4)), "slime_block");
    assert_eq!(name(&restored, cell(5, 5)), "stone");
    assert_eq!(name(&restored, cell(6, 4)), "air");
    assert_eq!(name(&restored, cell(6, 5)), "air");
}

fn advance_mixed_arena(state: &mut TickState, ticks: u64) {
    for _ in 0..ticks {
        state.refresh_active_chunks(state.tick);
        let mut writes = Vec::new();
        let mut spawned = Vec::new();
        run(state, &mut writes, &mut spawned);
        crate::fluids::run(state, &mut writes, &mut spawned);
        crate::fluids::schedule(state, &writes);
        state.world.fold_overlay();
        state.tick += 1;
    }
}

#[test]
fn mixed_arena_water_is_not_piston_loss_and_pending_overlap_survives_checkpoint() {
    for contained in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut original = state_in(Some(directory.path().to_owned()));
        original
            .tickets
            .insert("mixed-arena".into(), (ChunkPos { x: 0, z: 0 }, 2));
        original.world.ensure_blocking(&original.desired_chunks());
        let stone = original.registry.states.stone;
        let glass = block(&original, "glass", &[]);
        let barrier = block(&original, "barrier", &[]);
        let water = original.registry.states.water_levels[0];
        for x in -16..48 {
            for z in -16..48 {
                original.world.set_block(x, 63, z, stone);
            }
        }
        for y in [64, 72, 80, 88] {
            for x in 12..=24 {
                for z in 0..=20 {
                    original.world.set_block(x, y - 1, z, stone);
                }
            }
            original.world.set_block(20, y + 4, 4, water);
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                for py in (if contained { y } else { y + 4 })..=y + 4 {
                    original.world.set_block(20 + dx, py, 4 + dz, glass);
                }
            }
        }
        // MIX-21's roof catches the upper MIX-04 water spills. They flow
        // back over its west wall, creating a new falling column at x23.
        for z in -16..=32 {
            for y in 64..=69 {
                original.world.set_block(24, y, z, barrier);
            }
            for x in 24..=40 {
                original.world.set_block(x, 69, z, barrier);
            }
        }
        original.world.fold_overlay();
        place(&mut original, 15, 64, 15, "gold_block", &[]);
        for (z, adhesive) in [(14, "slime_block"), (16, "honey_block")] {
            place(&mut original, 15, 63, z, "air", &[]);
            place(&mut original, 14, 64, z, "piston", &[("facing", "east")]);
            place(&mut original, 15, 64, z, adhesive, &[]);
            place(&mut original, 15, 65, z, "gold_block", &[]);
            place(&mut original, 13, 64, z, "redstone_block", &[]);
            let base = BlockPos { x: 14, y: 64, z };
            schedule_cell(&mut original, base, if z == 14 { 0 } else { 2 }, 0);
        }
        let sources: Vec<_> = [64, 72, 80, 88]
            .into_iter()
            .map(|y| {
                (
                    BlockPos {
                        x: 20,
                        y: y + 4,
                        z: 4,
                    },
                    water,
                )
            })
            .collect();
        crate::fluids::schedule(&mut original, &sources);
        advance_mixed_arena(&mut original, 1);
        assert_eq!(
            value(
                &original,
                BlockPos {
                    x: 14,
                    y: 64,
                    z: 14
                },
                "extended"
            ),
            "true"
        );
        assert_eq!(
            value(
                &original,
                BlockPos {
                    x: 14,
                    y: 64,
                    z: 16
                },
                "extended"
            ),
            "false"
        );
        // There is no transient b36: checkpoint the accepted first stroke
        // and the pending second stroke through the normal durable queues.
        let tick = original.tick;
        let absent_blocks = original.block_ticks.checkpoint(&mut original.world, tick);
        let absent_fluids = original.fluid_ticks.checkpoint(&mut original.world, tick);
        let mut capture = original.world.capture_dirty(&Default::default());
        original
            .world
            .capture_scheduled_block_ticks(&mut capture, absent_blocks);
        original
            .world
            .capture_scheduled_fluid_ticks(&mut capture, absent_fluids);
        capture.write().unwrap();
        let mut restored = state_in(Some(directory.path().to_owned()));
        restored.tickets = original.tickets.clone();
        restored.world.ensure_blocking(&restored.desired_chunks());
        restored.tick = tick;
        restored.block_ticks = crate::block_ticks::BlockTicks::default();
        restored.fluid_ticks = crate::fluids::FluidTicks::default();
        for pos in restored.desired_chunks().keys() {
            let chunk = restored.world.chunk(*pos).unwrap();
            restored.block_ticks.hydrate_chunk(chunk, tick);
            restored.fluid_ticks.hydrate(chunk, tick);
        }
        assert!(
            restored
                .block_ticks
                .image()
                .chunks
                .iter()
                .flat_map(|chunk| &chunk.ticks)
                .any(|pending| pending.pos == [14, 64, 16])
        );
        advance_mixed_arena(&mut original, 1000);
        advance_mixed_arena(&mut restored, 1000);
        for z in [14, 16] {
            for x in 14..=17 {
                for y in 64..=65 {
                    assert_eq!(
                        original.world.block_at(x, y, z),
                        restored.world.block_at(x, y, z),
                        "contained={contained}, ({x}, {y}, {z})"
                    );
                }
            }
            let final_cell = BlockPos { x: 17, y: 64, z };
            assert_eq!(
                name(&restored, final_cell),
                if contained { "air" } else { "water" }
            );
            if !contained {
                assert_eq!(value(&restored, final_cell, "level"), "6");
            }
        }
        assert_eq!(
            name(
                &restored,
                BlockPos {
                    x: 16,
                    y: 64,
                    z: 15
                }
            ),
            "gold_block"
        );
        assert_eq!(name(&restored, BlockPos { x: 20, y: 64, z: 4 }), "water");
    }
}
