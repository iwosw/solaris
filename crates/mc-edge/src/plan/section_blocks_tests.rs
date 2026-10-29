//! Bulk edits share one world-queue credit per section, including deferred edits.
use super::*;
use mc_protocol::packets::play::{
    SectionBlocksUpdate, pack_section_pos, pack_section_relative_pos,
};

#[test]
fn fill_updates_are_batched_per_section_and_deferred_as_a_unit() {
    with_cache(|cache| {
        let (pos, chunk) = chunk_at(-1, -1);
        let mut viewer = viewer_at(1, [-8.0, 0.0, -8.0]);
        plan_viewer(
            &output(
                vec![(pos, Arc::clone(&chunk))],
                vec![],
                ChangeSet::default(),
            ),
            &mut viewer,
            0,
            cache,
        );
        let mut edited = (*chunk).clone();
        let mut changes = ChangeSet::default();
        for x in -16_i32..0 {
            for y in -16..0 {
                for z in -16_i32..0 {
                    edited.set_block(
                        x.rem_euclid(16) as u8,
                        y,
                        z.rem_euclid(16) as u8,
                        mc_world::BlockStateId(1),
                    );
                    changes.blocks.push(BlockChange {
                        position: mc_world::chunk::BlockPos { x, y, z },
                        state: 99, // The newest snapshot, not a queued post-image, is authoritative.
                    });
                }
            }
        }
        for x in [-16_i32, -15] {
            edited.set_block(x.rem_euclid(16) as u8, 0, 0, mc_world::BlockStateId(2));
            changes.blocks.push(BlockChange {
                position: mc_world::chunk::BlockPos { x, y: 0, z: -16 },
                state: 99,
            });
        }
        changes.blocks.push(changes.blocks[0]);
        let changed = output(vec![(pos, Arc::new(edited))], vec![], changes);
        viewer.world_credit = 0;
        assert!(
            plan_viewer(&changed, &mut viewer, 0, cache)
                .iter()
                .all(|p| p.reliable)
        );
        assert_eq!(viewer.pending_blocks.len(), 4098);
        let quiet = TickOutput {
            changes: Arc::new(ChangeSet::default()),
            ..changed
        };
        viewer.world_credit = 1;
        for (section_y, expected_count, expected_state, remaining) in
            [(-1, 4096, 1, 2), (0, 2, 2, 0)]
        {
            let packets = plan_viewer(&quiet, &mut viewer, 0, cache);
            let world: Vec<_> = packets.iter().filter(|p| !p.reliable).collect();
            assert_eq!(world.len(), 1);
            assert_eq!(
                world[0].id,
                SectionBlocksUpdate::ID,
                "a fill must not fan out one packet per block"
            );
            let update = SectionBlocksUpdate::decode(&mut &body_of(world[0])[..]).unwrap();
            assert_eq!(update.section_pos, pack_section_pos(-1, section_y, -1));
            assert_eq!(update.changes.len(), expected_count);
            assert!(
                update
                    .changes
                    .iter()
                    .all(|change| change.state_id == expected_state)
            );
            let positions: HashSet<_> = update
                .changes
                .iter()
                .map(|change| change.relative_pos)
                .collect();
            assert_eq!(positions.len(), expected_count);
            if section_y == 0 {
                assert_eq!(
                    positions,
                    HashSet::from_iter([
                        pack_section_relative_pos(-16, 0, -16),
                        pack_section_relative_pos(-15, 0, -16)
                    ])
                );
            }
            assert_eq!(viewer.pending_blocks.len(), remaining);
        }
        assert!(
            plan_viewer(&quiet, &mut viewer, 0, cache)
                .iter()
                .all(|p| p.reliable)
        );
    });
}
