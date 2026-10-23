use super::*;
use crate::boundary::{ChangeSet, ConnectionId, EntityId, Hand, PlayerInput};
use crate::dimensions::{Dimension, travel as dimensions_travel};
use crate::tick::TickState;
use std::collections::BTreeMap;
use std::sync::Arc;

struct Empty {
    air: u32,
    geometry: mc_world::ChunkGeometry,
}
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty_with_geometry(
            pos,
            mc_world::BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
            self.geometry,
        )
    }
}
fn local(dimension: Dimension, registry: Arc<crate::registry::CoreRegistry>) -> TickState {
    local_with_directory(dimension, registry, None)
}

fn local_with_directory(
    dimension: Dimension,
    registry: Arc<crate::registry::CoreRegistry>,
    directory: Option<std::path::PathBuf>,
) -> TickState {
    let generator = Arc::new(Empty {
        air: registry.states.air,
        geometry: dimension.geometry(),
    });
    let world = if let Some(directory) = directory {
        crate::world::ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&crate::CoreConfig::new(directory, 42))
                .unwrap(),
            crate::native_checkpoint::dimension_id(dimension),
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 70, 8],
            dimension.geometry(),
        )
    } else {
        crate::world::ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 70, 8],
            dimension.geometry(),
        )
    };
    let mut state = TickState::new(registry, world, 0, 4096);
    state.dimension = dimension;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    load(&mut state, [8.0, 70.0, 8.0], 1);
    state
}

fn publish_fixture_manifest(directory: &std::path::Path) {
    let manifest = crate::journal::CheckpointManifest {
        world_spawn: Some([8, 70, 8]),
        dimensions: [Dimension::Nether, Dimension::End]
            .into_iter()
            .map(|dimension| {
                (
                    dimension,
                    Box::new(crate::journal::CheckpointManifest {
                        world_spawn: Some([8, 70, 8]),
                        ..Default::default()
                    }),
                )
            })
            .collect(),
        ..Default::default()
    };
    crate::native_checkpoint::tests::write_manifest(
        &crate::CoreConfig::new(directory.to_owned(), 42),
        &manifest,
    )
    .unwrap();
}

fn crash_batches(
    directory: &std::path::Path,
    segment: &std::path::Path,
    complete: &[u8],
    cut: usize,
) -> Vec<(u64, u64, Vec<crate::journal::JournalEntry>)> {
    std::fs::write(segment, &complete[..cut]).unwrap();
    let temporary = segment.with_extension("bin.tmp");
    if cut != complete.len() {
        let error = crate::journal::read_complete_batches_after(&directory.join("journal"), 0)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!("journal {}: record ", segment.display()))
                && error.contains(" offset "),
            "{error}"
        );
        std::fs::rename(segment, &temporary).unwrap();
    }
    let batches =
        crate::journal::read_complete_batches_after(&directory.join("journal"), 0).unwrap();
    if cut != complete.len() {
        std::fs::rename(temporary, segment).unwrap();
    }
    batches
}
fn load(state: &mut TickState, pos: [f64; 3], radius: i32) {
    let center = mc_world::ChunkPos {
        x: (pos[0].floor() as i32).div_euclid(16),
        z: (pos[2].floor() as i32).div_euclid(16),
    };
    let want = crate::world::chunks_in_radius(center, radius)
        .into_iter()
        .map(|pos| (pos, 0))
        .collect();
    state.world.ensure_blocking(&want);
}
fn worlds() -> (TickState, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = local(Dimension::Overworld, Arc::clone(&registry));
    for dimension in [Dimension::Nether, Dimension::End] {
        state
            .other_dimensions
            .insert(dimension, Box::new(local(dimension, Arc::clone(&registry))));
    }
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::from_u128(1),
            name: "Portal".into(),
            permission_level: 0,
        }],
    );
    let id = state.connections.entity_of(ConnectionId(1)).unwrap();
    (state, id)
}
fn frame(
    state: &mut TickState,
    bottom: BlockPos,
    width: i32,
    height: i32,
    x_axis: bool,
) -> nether::Frame {
    let shape = nether::Frame {
        bottom,
        width,
        height,
        x_axis,
    };
    let obsidian = state_id(state, "minecraft:obsidian", &[]);
    for y in -1..=height {
        for x in -1..=width {
            if (y == -1 || y == height) && (x == -1 || x == width) {
                continue;
            }
            let pos = shape.cell(x, y);
            state.world.set_block(
                pos.x,
                pos.y,
                pos.z,
                if y == -1 || y == height || x == -1 || x == width {
                    obsidian
                } else {
                    state.registry.states.air
                },
            );
        }
    }
    state.world.fold_overlay();
    shape
}
fn light(state: &mut TickState, shape: nether::Frame) {
    assert!(nether::ignite(state, shape.bottom, &mut Vec::new()));
    state.world.fold_overlay();
}
fn portal_step(state: &mut TickState) -> BTreeMap<Dimension, ChangeSet> {
    state.tick += 1;
    for child in state.other_dimensions.values_mut() {
        child.tick = state.tick;
    }
    let mut changes = BTreeMap::new();
    travel::finish(state, &mut changes);
    changes
}
fn held(state: &mut TickState, id: EntityId, name: &str, count: i32) {
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap();
    state.players.get_mut(&id).unwrap().inventory.slots[0] = mc_data::ItemStack::new(item, count);
}
fn identity_count(state: &TickState, id: EntityId) -> usize {
    std::iter::once(state)
        .chain(state.other_dimensions.values().map(|state| &**state))
        .filter(|state| state.entities.slot_of_any(id).is_some())
        .count()
}

#[test]
fn frame_sizes_axes_optional_corners_and_damage_collapse() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = local(Dimension::Overworld, registry);
    for x_axis in [true, false] {
        let shape = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 2, 3, x_axis);
        light(&mut state, shape);
        assert!(nether::frame(&state, shape.cell(1, 2), x_axis).is_some());
        let broken = shape.cell(-1, 1);
        let air = state.registry.states.air;
        state.world.set_block(broken.x, broken.y, broken.z, air);
        let mut writes = vec![(broken, air)];
        nether::after_blocks(&mut state, &mut writes);
        for y in 0..3 {
            for x in 0..2 {
                assert!(!named(&state, shape.cell(x, y), "nether_portal"));
            }
        }
        state.world.fold_overlay();
    }
    let bad = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 1, 3, true);
    assert!(!nether::ignite(&mut state, bad.bottom, &mut Vec::new()));
    load(&mut state, [4.0, 70.0, 4.0], 2);
    let max = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 21, 21, true);
    light(&mut state, max);
    assert_eq!(
        nether::frame(&state, max.cell(20, 20), true).unwrap().width,
        21
    );
}

#[test]
fn ignition_items_debit_only_once_and_fire_lights_frame() {
    let (mut state, id) = worlds();
    for name in ["minecraft:flint_and_steel", "minecraft:fire_charge"] {
        let shape = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 2, 3, true);
        held(&mut state, id, name, 2);
        assert!(crate::fire::use_on(
            &mut state,
            id,
            Hand::Main,
            shape.cell(0, -1),
            crate::placement::Face::Up,
            &mut ChangeSet::default()
        ));
        assert!(named(&state, shape.bottom, "nether_portal"));
        let stack = &state.players[&id].inventory.slots[0];
        if name.ends_with("fire_charge") {
            assert_eq!(stack.count, 1);
        } else {
            assert_eq!(stack.damage, Some(1));
        }
    }
    let shape = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 2, 3, true);
    let fire = state_id(&state, "minecraft:fire", &[]);
    state.world.set_block(4, 70, 4, fire);
    nether::after_blocks(&mut state, &mut vec![(shape.bottom, fire)]);
    assert!(named(&state, shape.bottom, "nether_portal"));
}

#[test]
fn player_timer_scaling_link_and_cooldown() {
    let (mut state, id) = worlds();
    let source = frame(
        &mut state,
        BlockPos {
            x: 80,
            y: 70,
            z: 80,
        },
        2,
        3,
        true,
    );
    load(&mut state, [80.0, 70.0, 80.0], 1);
    // Rebuild after admitting the formerly absent frame chunks.
    let source = frame(&mut state, source.bottom, 2, 3, true);
    light(&mut state, source);
    let nether = state.other_dimensions.get_mut(&Dimension::Nether).unwrap();
    load(nether, [10.0, 70.0, 10.0], 2);
    let destination = frame(
        nether,
        BlockPos {
            x: 10,
            y: 70,
            z: 10,
        },
        2,
        3,
        false,
    );
    light(nether, destination);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, [80.5, 70.0, 80.5]);
    for _ in 0..80 {
        portal_step(&mut state);
        assert_eq!(
            dimensions_travel::locate(&state, id).unwrap().0,
            Dimension::Overworld
        );
    }
    portal_step(&mut state);
    assert_eq!(
        dimensions_travel::locate(&state, id).unwrap().0,
        Dimension::Nether
    );
    assert_eq!(identity_count(&state, id), 1);
    let nether = &state.other_dimensions[&Dimension::Nether];
    let slot = nether.entities.slot_of(id).unwrap();
    assert!(nether.entities.pos(slot)[0] < 12.0);
    assert_eq!(nether.entities.row(slot).entity_life.portal.cooldown, 10);
    for _ in 0..10 {
        portal_step(&mut state);
    }
    assert_eq!(
        state.other_dimensions[&Dimension::Nether]
            .entities
            .row(slot)
            .entity_life
            .portal
            .cooldown,
        10
    );
}

#[test]
fn items_and_mobs_wait_for_destination_then_transfer_once() {
    let (mut state, _) = worlds();
    let source = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 2, 3, true);
    light(&mut state, source);
    let stack = mc_data::ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:diamond").unwrap())
            .unwrap(),
        3,
    );
    let item = crate::tick::spawn_item_entity(&mut state, [4.5, 70.0, 4.5], stack);
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    let mob = state.spawn_mob(cow, [5.5, 70.0, 4.5], 10.0);
    portal_step(&mut state);
    for id in [item, mob] {
        assert_eq!(identity_count(&state, id), 1);
        assert_eq!(
            dimensions_travel::locate(&state, id).unwrap().0,
            Dimension::Overworld
        );
    }
    let nether = state.other_dimensions.get_mut(&Dimension::Nether).unwrap();
    load(nether, [0.5, 70.0, 0.5], 2);
    portal_step(&mut state);
    for id in [item, mob] {
        assert_eq!(identity_count(&state, id), 1);
        assert_eq!(
            dimensions_travel::locate(&state, id).unwrap().0,
            Dimension::Nether
        );
    }
    let nether = &state.other_dimensions[&Dimension::Nether];
    let slot = nether.entities.slot_of(item).unwrap();
    assert_eq!(nether.entities.row(slot).item.as_ref().unwrap().count, 3);
    assert!(nether::search(nether, [0.5, 70.0, 0.5], 16).is_some());
    assert!(named(nether, BlockPos { x: 0, y: 69, z: 1 }, "obsidian"));
}

#[test]
fn physics_empty_vehicle_portal_cooldown_is_ten_ticks() {
    for name in ["minecraft:oak_boat", "minecraft:minecart"] {
        let (mut state, _) = worlds();
        let source = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 2, 3, true);
        light(&mut state, source);
        let type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name(name)
            .unwrap()
            .protocol_id;
        let id = state.spawn_mob(type_id, [4.5, 70.0, 4.5], 10.0);
        let nether = state.other_dimensions.get_mut(&Dimension::Nether).unwrap();
        load(nether, [0.5, 70.0, 0.5], 2);
        portal_step(&mut state);
        assert_eq!(
            dimensions_travel::locate(&state, id).unwrap().0,
            Dimension::Nether
        );
        let nether = state.other_dimensions.get_mut(&Dimension::Nether).unwrap();
        let slot = nether.entities.slot_of(id).unwrap();
        assert_eq!(nether.entities.row(slot).entity_life.portal.cooldown, 10);
        nether.entities.set_pos(slot, [12.0, 75.0, 12.0]);
        for _ in 0..10 {
            portal_step(&mut state);
        }
        let nether = &state.other_dimensions[&Dimension::Nether];
        assert_eq!(
            nether
                .entities
                .row(nether.entities.slot_of(id).unwrap())
                .entity_life
                .portal
                .cooldown,
            0
        );
    }
}

#[test]
fn end_portal_native_inside_shape_and_boss_exclusions() {
    let (mut state, id) = worlds();
    let portal = BlockPos { x: 8, y: 70, z: 8 };
    state.world.set_block(
        portal.x,
        portal.y,
        portal.z,
        state_id(&state, "minecraft:end_portal", &[]),
    );
    state.world.fold_overlay();
    load(
        state.other_dimensions.get_mut(&Dimension::End).unwrap(),
        [100.0, 49.0, 0.0],
        1,
    );
    let mut bosses = Vec::new();
    for name in ["minecraft:ender_dragon", "minecraft:wither"] {
        let entity_type = mc_data::entity_types::entity_type_contract_26_1_2_by_name(name)
            .unwrap()
            .protocol_id;
        bosses.push(state.spawn_mob(entity_type, [8.5, 70.0, 8.5], 100.0));
    }
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, [8.5, 70.75, 8.5]);
    portal_step(&mut state);
    assert_eq!(
        dimensions_travel::locate(&state, id).unwrap().0,
        Dimension::Overworld
    );
    state.entities.set_pos(slot, [8.5, 70.749, 8.5]);
    portal_step(&mut state);
    assert_eq!(
        dimensions_travel::locate(&state, id).unwrap().0,
        Dimension::End
    );
    for boss in bosses {
        assert_eq!(
            dimensions_travel::locate(&state, boss).unwrap().0,
            Dimension::Overworld
        );
        assert_eq!(identity_count(&state, boss), 1);
    }
}

#[test]
fn end_eyes_require_inward_facing_twelve_frame_pattern() {
    let (mut state, id) = worlds();
    let center = BlockPos { x: 8, y: 70, z: 8 };
    let mut frames = Vec::new();
    for n in -1..=1 {
        frames.extend([
            (
                BlockPos {
                    x: center.x + n,
                    z: center.z - 2,
                    ..center
                },
                "south",
            ),
            (
                BlockPos {
                    x: center.x + n,
                    z: center.z + 2,
                    ..center
                },
                "north",
            ),
            (
                BlockPos {
                    x: center.x - 2,
                    z: center.z + n,
                    ..center
                },
                "east",
            ),
            (
                BlockPos {
                    x: center.x + 2,
                    z: center.z + n,
                    ..center
                },
                "west",
            ),
        ]);
    }
    for (pos, facing) in &frames {
        let id = state_id(
            &state,
            "minecraft:end_portal_frame",
            &[("facing", facing), ("eye", "false")],
        );
        state.world.set_block(pos.x, pos.y, pos.z, id);
    }
    held(&mut state, id, "minecraft:ender_eye", 12);
    for (index, (pos, _)) in frames.iter().enumerate() {
        assert!(crate::end_portals::use_eye(
            &mut state,
            id,
            Hand::Main,
            *pos
        ));
        assert_eq!(named(&state, center, "end_portal"), index == 11);
    }
    assert!(state.players[&id].inventory.slots[0].is_empty());
    assert_eq!(state.merge_block_writes.len(), 21);
}

#[test]
fn end_platform_cleared_and_exit_uses_valid_personal_spawn() {
    let (mut state, id) = worlds();
    let portal = state_id(&state, "minecraft:end_portal", &[]);
    state.world.set_block(8, 70, 8, portal);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, [8.5, 70.0, 8.5]);
    let end = state.other_dimensions.get_mut(&Dimension::End).unwrap();
    load(end, [100.0, 50.0, 0.0], 1);
    let obsidian = state_id(end, "minecraft:obsidian", &[]);
    end.world.set_block(100, 51, 0, obsidian);
    portal_step(&mut state);
    assert_eq!(
        dimensions_travel::locate(&state, id).unwrap().0,
        Dimension::End
    );
    let end = state.other_dimensions.get_mut(&Dimension::End).unwrap();
    assert!(named(
        end,
        BlockPos {
            x: 100,
            y: 48,
            z: 0,
        },
        "obsidian"
    ));
    assert!(named(
        end,
        BlockPos {
            x: 100,
            y: 51,
            z: 0
        },
        "air"
    ));
    crate::end_portals::set_exit_portal(end, &mut ChangeSet::default(), true);
    let slot = end.entities.slot_of(id).unwrap();
    end.entities.set_pos(slot, [1.5, 64.0, 0.5]);
    end.entities.row_mut(slot).entity_life.portal.cooldown = 0;
    let player = end.players.get_mut(&id).unwrap();
    player.respawn_point = Some([12, 70, 12]);
    player.respawn_forced = true;
    player.respawn_dimension = Dimension::Overworld;
    load(&mut state, [12.0, 70.0, 12.0], 1);
    portal_step(&mut state);
    assert_eq!(
        dimensions_travel::locate(&state, id).unwrap().0,
        Dimension::Overworld
    );
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(state.entities.pos(slot), [12.5, 70.1, 12.5]);
    assert_eq!(identity_count(&state, id), 1);
}

#[test]
fn generated_gateway_pair_nbt_and_cooldown_are_retained() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = local(Dimension::End, registry);
    let mut changes = ChangeSet::default();
    crate::end_portals::create_gateway(&mut state, &mut changes, 0);
    let inner = BlockPos { x: 96, y: 75, z: 0 };
    let initial = crate::end_portals::gateway(&state, inner).unwrap();
    assert!(initial.exit.is_none());
    load(&mut state, [1024.0, 75.0, 0.0], 1);
    let image = crate::end_portals::initialize_gateway(&mut state, inner, &mut changes, initial);
    assert_eq!(image.exit.unwrap().x, 1024);
    let outer = image.exit.unwrap();
    assert!(crate::end_portals::gateway(&state, outer).is_ok());
    let bytes = state
        .world
        .chunk(mc_world::ChunkPos { x: 6, z: 0 })
        .unwrap()
        .block_entities[&inner]
        .clone();
    let mut retained = image;
    retained.cooldown_until = 123;
    crate::end_portals::save_gateway(&mut state, inner, retained);
    assert_eq!(
        crate::end_portals::gateway(&state, inner)
            .unwrap()
            .cooldown_until,
        123
    );
    state.world.set_opaque_block_entity(inner, Some(bytes));
    assert_eq!(crate::end_portals::gateway(&state, inner).unwrap(), image);
}

#[test]
fn torn_gateway_pair_wal_replays_both_links_atomically() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = local(Dimension::End, Arc::clone(&registry));
    load(&mut state, [96.0, 75.0, 0.0], 1);
    load(&mut state, [1024.0, 75.0, 0.0], 1);
    let inner = BlockPos { x: 96, y: 75, z: 0 };
    let mut initial_changes = ChangeSet::default();
    crate::end_portals::create_gateway(&mut state, &mut initial_changes, 0);
    let (mut baseline, _, _) = crate::tick::run_tick(&mut state, &[]);
    Arc::get_mut(&mut baseline.changes)
        .unwrap()
        .blocks
        .extend(initial_changes.blocks);
    let initial = crate::core::journal_entries_for_tick(&mut state, &baseline);
    let mut pair_changes = ChangeSet::default();
    let initial_image = crate::end_portals::gateway(&state, inner).unwrap();
    let mut image =
        crate::end_portals::initialize_gateway(&mut state, inner, &mut pair_changes, initial_image);
    let outer = image.exit.unwrap();
    image.cooldown_until = 123;
    crate::end_portals::save_gateway(&mut state, inner, image);
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .blocks
        .extend(pair_changes.blocks);
    let pair = crate::core::journal_entries_for_tick(&mut state, &output);
    let directory =
        std::env::temp_dir().join(format!("solaris-gateway-crash-{}", uuid::Uuid::new_v4()));
    let mut writer = crate::journal::JournalWriter::open(&directory, 1).unwrap();
    writer
        .append(
            1,
            baseline.tick,
            &[crate::journal::JournalEntry::Dimension {
                dimension: Dimension::End,
                entries: initial,
            }],
        )
        .unwrap();
    writer
        .append(
            output.tick,
            output.tick,
            &[crate::journal::JournalEntry::Dimension {
                dimension: Dimension::End,
                entries: pair,
            }],
        )
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let segment = directory
        .join("journal")
        .join(format!("batch-{:020}-{:020}.bin", output.tick, output.tick));
    let complete = std::fs::read(&segment).unwrap();
    for cut in [
        0,
        1,
        10,
        complete.len() / 2,
        complete.len() - 1,
        complete.len(),
    ] {
        let batches = crash_batches(&directory, &segment, &complete, cut);
        let batches = crate::dimensions::journal_for_dimension(&batches, Dimension::End);
        let replay = crate::journal::replay_batches(Default::default(), &batches);
        let replay = crate::journal::replay_batches(replay, &batches);
        let mut recovered = local(Dimension::End, Arc::clone(&registry));
        load(&mut recovered, [96.0, 75.0, 0.0], 1);
        load(&mut recovered, [1024.0, 75.0, 0.0], 1);
        for ((x, y, z), block) in replay.blocks {
            recovered.world.set_block(x, y, z, block);
        }
        recovered.world.fold_overlay();
        for ((x, y, z), bytes) in replay.opaque_block_entities {
            recovered
                .world
                .set_opaque_block_entity(BlockPos { x, y, z }, bytes);
        }
        let recovered_inner = crate::end_portals::gateway(&recovered, inner).unwrap();
        if cut == complete.len() {
            assert_eq!(recovered_inner.exit, Some(outer));
            assert_eq!(recovered_inner.cooldown_until, 123);
            assert_eq!(
                crate::end_portals::gateway(&recovered, outer).unwrap().exit,
                Some(inner)
            );
        } else {
            assert!(recovered_inner.exit.is_none(), "cut {cut}");
            assert!(
                crate::end_portals::gateway(&recovered, outer).is_err(),
                "cut {cut}"
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn corrupt_portal_timer_and_gateway_fail_closed() {
    for json in [
        r#"{"elapsed":2147483648,"cooldown":0}"#,
        r#"{"elapsed":0,"cooldown":301}"#,
    ] {
        assert!(
            serde_json::from_str::<PortalState>(json)
                .unwrap_err()
                .to_string()
                .contains("portal timer: invalid")
        );
    }
    assert!(from_fields(&[("PortalCooldown".into(), mc_nbt::Tag::Int(-1))]).is_err());
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = local(Dimension::End, registry);
    let pos = BlockPos { x: 8, y: 70, z: 8 };
    let block = state_id(&state, "minecraft:end_gateway", &[]);
    state.world.set_block(pos.x, pos.y, pos.z, block);
    state.world.fold_overlay();
    state.world.set_opaque_block_entity(pos, Some(vec![0]));
    assert!(
        crate::end_portals::gateway(&state, pos)
            .unwrap_err()
            .starts_with("minecraft:the_end end_gateway (8,70,8):")
    );
    assert!(
        crate::end_portals::validate_gateways(&state)
            .unwrap_err()
            .starts_with("minecraft:the_end end_gateway (8,70,8):")
    );
}

#[test]
fn corrupt_gateway_wal_refuses_core_recovery_with_exact_cell() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let state = local(Dimension::End, registry);
    let pos = BlockPos { x: 8, y: 70, z: 8 };
    let block = state_id(&state, "minecraft:end_gateway", &[]);
    let fields = mc_nbt::Tag::Compound(vec![
        (
            "id".into(),
            mc_nbt::Tag::String("minecraft:end_gateway".into()),
        ),
        ("x".into(), mc_nbt::Tag::Int(pos.x)),
        ("y".into(), mc_nbt::Tag::Int(pos.y)),
        ("z".into(), mc_nbt::Tag::Int(pos.z)),
        ("Age".into(), mc_nbt::Tag::Long(-1)),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &fields).unwrap();
    let directory =
        std::env::temp_dir().join(format!("solaris-gateway-corrupt-{}", uuid::Uuid::new_v4()));
    let mut writer = crate::journal::JournalWriter::open(&directory, 1).unwrap();
    publish_fixture_manifest(&directory);
    writer
        .append(
            2,
            2,
            &[crate::journal::JournalEntry::Dimension {
                dimension: Dimension::End,
                entries: vec![
                    crate::journal::JournalEntry::Block {
                        pos: [pos.x, pos.y, pos.z],
                        state: block,
                    },
                    crate::journal::JournalEntry::OpaqueBlockEntity {
                        pos: [pos.x, pos.y, pos.z],
                        nbt: Some(bytes),
                    },
                ],
            }],
        )
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let mut config = crate::config::CoreConfig::new(directory.clone(), 42);
    config.view_distance = 0;
    config.simulation_distance = 0;
    match crate::Core::open(config) {
        Err(error) => assert!(
            error.to_string().contains(
                "minecraft:the_end end_gateway (8,70,8): Age must be nonnegative TAG_Long"
            ),
            "{error}"
        ),
        Ok(_) => panic!("corrupt gateway recovery was admitted"),
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn portal_state_player_and_entity_checkpoint_roundtrip() {
    let (mut state, id) = worlds();
    let timer = PortalState {
        elapsed: 37,
        cooldown: 123,
    };
    state
        .entities
        .row_mut(state.entities.slot_of(id).unwrap())
        .entity_life
        .portal = timer;
    let record = crate::core::player_record_for(&state, id, state.tick).unwrap();
    let path = std::env::temp_dir().join(format!("solaris-portals-{}", uuid::Uuid::new_v4()));
    let config = crate::CoreConfig::new(path.clone(), 42);
    crate::native_checkpoint::tests::write_player(
        &config,
        &uuid::Uuid::from_u128(1).to_string(),
        &record,
        &state.registry.items,
    )
    .unwrap();
    let recovered = crate::native_checkpoint::tests::read_player(
        &config,
        &uuid::Uuid::from_u128(1).to_string(),
        &state.registry.items,
    )
    .unwrap();
    assert_eq!(recovered.portal, timer);
    let image = crate::journal::EntityStateImage::from_row(
        state.entities.body(state.entities.slot_of(id).unwrap()),
        state.entities.row(state.entities.slot_of(id).unwrap()),
    );
    let json = serde_json::to_vec(&image).unwrap();
    let recovered: crate::journal::EntityStateImage = serde_json::from_slice(&json).unwrap();
    assert_eq!(recovered.entity_life.portal, timer);
    let native = fields(timer);
    assert_eq!(from_fields(&native).unwrap(), timer);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn gateway_teleport_pair_and_shared_block_cooldown() {
    let (mut state, player) = worlds();
    let end = state.other_dimensions.get_mut(&Dimension::End).unwrap();
    load(end, [96.0, 75.0, 0.0], 1);
    load(end, [1024.0, 75.0, 0.0], 1);
    crate::end_portals::create_gateway(end, &mut ChangeSet::default(), 0);
    dimensions_travel::change_dimension(
        &mut state,
        player,
        Dimension::End,
        [96.5, 75.0, 0.5],
        [0.0, 0.0],
        &mut BTreeMap::new(),
    )
    .unwrap();
    portal_step(&mut state);
    let end = state.other_dimensions.get_mut(&Dimension::End).unwrap();
    let slot = end.entities.slot_of(player).unwrap();
    assert!(end.entities.pos(slot)[0] >= 1000.0);
    assert_eq!(identity_count(&state, player), 1);
    let end = state.other_dimensions.get_mut(&Dimension::End).unwrap();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    let mob = end.spawn_mob(cow, [96.5, 75.0, 0.5], 10.0);
    let inner = BlockPos { x: 96, y: 75, z: 0 };
    let image = crate::end_portals::gateway(end, inner).unwrap();
    let reverse = crate::end_portals::gateway(end, image.exit.unwrap()).unwrap();
    assert_eq!(reverse.exit, Some(inner));
    portal_step(&mut state);
    let end = &state.other_dimensions[&Dimension::End];
    assert_eq!(
        end.entities.pos(end.entities.slot_of(mob).unwrap())[0],
        96.5
    );
    for _ in 0..39 {
        portal_step(&mut state);
    }
    let end = &state.other_dimensions[&Dimension::End];
    assert!(end.entities.pos(end.entities.slot_of(mob).unwrap())[0] >= 1000.0);
}

#[test]
fn torn_portal_transfer_wal_recovers_exactly_one_entity_owner() {
    let (mut state, player) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    let mob = state.spawn_mob(cow, [8.5, 70.0, 8.5], 10.0);
    let (mut baseline, _, _) = crate::tick::run_tick(&mut state, &[]);
    Arc::get_mut(&mut baseline.changes)
        .unwrap()
        .entities_spawned
        .extend([player, mob]);
    let initial = crate::core::journal_entries_for_tick(&mut state, &baseline);
    let source = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 2, 3, true);
    light(&mut state, source);
    for y in -1..=3 {
        for x in -1..=2 {
            let pos = source.cell(x, y);
            state
                .merge_block_writes
                .push((pos, state.world.block_at(pos.x, pos.y, pos.z).unwrap()));
        }
    }
    let nether = state.other_dimensions.get_mut(&Dimension::Nether).unwrap();
    load(nether, [0.5, 70.0, 0.5], 2);
    nether
        .tickets
        .insert("test:search".into(), (mc_world::ChunkPos { x: 0, z: 0 }, 2));
    state
        .gamerules
        .insert("players_nether_portal_default_delay".into(), "0".into());
    for id in [player, mob] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.set_pos(slot, [4.5, 70.0, 4.5]);
    }
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    assert_eq!(
        dimensions_travel::locate(&state, mob).unwrap().0,
        Dimension::Nether
    );
    assert_eq!(
        dimensions_travel::locate(&state, player).unwrap().0,
        Dimension::Nether
    );
    let transfer = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(!transfer.iter().any(|entry| matches!(entry,crate::journal::JournalEntry::Ticket { owner,.. } if owner.starts_with("solaris:portal:"))));
    let directory =
        std::env::temp_dir().join(format!("solaris-portal-crash-{}", uuid::Uuid::new_v4()));
    let mut writer = crate::journal::JournalWriter::open(&directory, 1).unwrap();
    // The native manifest fixture below captures tick 1. Keep its empty WAL
    // interval separate from the newer baseline image.
    writer.append(1, 1, &[]).unwrap();
    writer.append(2, baseline.tick, &initial).unwrap();
    writer.flush().unwrap();
    let segment = directory
        .join("journal")
        .join(format!("batch-{:020}-{:020}.bin", output.tick, output.tick));
    writer.append(output.tick, output.tick, &transfer).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let complete = std::fs::read(&segment).unwrap();
    let cuts = [
        0,
        1,
        10,
        complete.len() / 2,
        complete.len() - 1,
        complete.len(),
    ];
    for cut in cuts {
        let batches = crash_batches(&directory, &segment, &complete, cut);
        let mut owners = Vec::new();
        for dimension in Dimension::ALL {
            let replay = crate::journal::replay_batches(
                Default::default(),
                &crate::dimensions::journal_for_dimension(&batches, dimension),
            );
            if replay
                .entities
                .get(&mob.0)
                .is_some_and(|entity| entity.alive)
            {
                owners.push(dimension);
            }
            let record = &replay.players[&uuid::Uuid::from_u128(1).to_string()];
            assert_eq!(
                record.dimension,
                if cut == complete.len() {
                    Dimension::Nether
                } else {
                    Dimension::Overworld
                }
            );
        }
        assert_eq!(
            owners,
            vec![if cut == complete.len() {
                Dimension::Nether
            } else {
                Dimension::Overworld
            }],
            "cut {cut}"
        );
    }
    std::fs::write(&segment, &complete).unwrap();
    publish_fixture_manifest(&directory);
    let mut config = crate::config::CoreConfig::new(directory.clone(), 42);
    config.view_distance = 0;
    config.simulation_distance = 0;
    let core = crate::Core::open(config).unwrap();
    let mut receiver = core.subscribe();
    assert!(core.submit(PlayerInput::Join {
        permission_level: 0,
        connection: ConnectionId(9),
        uuid: uuid::Uuid::from_u128(1),
        name: "Portal".into()
    }));
    let mut joined = false;
    for _ in 0..80 {
        let image = receiver.blocking_recv().unwrap();
        let owners: Vec<_> = Dimension::ALL
            .into_iter()
            .filter(|dimension| {
                image
                    .in_dimension(*dimension)
                    .unwrap()
                    .snapshot
                    .entities
                    .iter()
                    .any(|entity| entity.id == mob)
            })
            .collect();
        assert_eq!(owners, vec![Dimension::Nether]);
        for (_, queue) in core.try_drain_outbox() {
            joined |= queue.iter().any(|change| {
                matches!(
                    change,
                    crate::boundary::PlayerChange::Joined {
                        dimension: Dimension::Nether,
                        ..
                    }
                )
            });
        }
        if joined {
            break;
        }
    }
    assert!(joined);
    assert!(core.checkpoint());
    core.shutdown();
    let stored = crate::native_checkpoint::tests::read_player(
        &crate::CoreConfig::new(directory.clone(), 42),
        &uuid::Uuid::from_u128(1).to_string(),
        &state.registry.items,
    )
    .unwrap();
    assert_eq!(stored.dimension, Dimension::Nether);
    assert!(directory.join("native/state.redb").is_file());
    std::fs::remove_dir_all(directory).unwrap();
}

fn chest(state: &mut TickState, pos: BlockPos) -> u32 {
    load(
        state,
        [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)],
        1,
    );
    let chest = state_id(
        state,
        "minecraft:chest",
        &[
            ("facing", "north"),
            ("type", "single"),
            ("waterlogged", "false"),
        ],
    );
    state.world.set_block(pos.x, pos.y, pos.z, chest);
    state.world.fold_overlay();
    let diamond = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond").unwrap())
        .unwrap();
    let mut slots = vec![mc_data::ItemStack::EMPTY; 27];
    slots[0] = mc_data::ItemStack::new(diamond, 7);
    state.block_entities.insert(
        (pos.x, pos.y, pos.z),
        crate::tick::BlockEntityState {
            slots,
            furnace: None,
        },
    );
    let root = mc_nbt::Tag::Compound(vec![
        ("id".into(), mc_nbt::Tag::String("minecraft:chest".into())),
        ("x".into(), mc_nbt::Tag::Int(pos.x)),
        ("y".into(), mc_nbt::Tag::Int(pos.y)),
        ("z".into(), mc_nbt::Tag::Int(pos.z)),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &root).unwrap();
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
    diamond
}

#[test]
fn construction_drops_container_contents_and_replays_final_storage() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    for construction in 0..5 {
        let dimension = if construction == 0 {
            Dimension::Nether
        } else {
            Dimension::End
        };
        let mut state = local(dimension, Arc::clone(&registry));
        let (mut baseline, _, _) = crate::tick::run_tick(&mut state, &[]);
        let pos = match construction {
            0 => BlockPos { x: 8, y: 69, z: 8 },
            1 => BlockPos {
                x: 100,
                y: 49,
                z: 0,
            },
            2 => BlockPos { x: 96, y: 75, z: 0 },
            3 => BlockPos { x: 0, y: 64, z: 0 },
            _ => BlockPos {
                x: 1023,
                y: 74,
                z: 0,
            },
        };
        let diamond = chest(&mut state, pos);
        Arc::get_mut(&mut baseline.changes)
            .unwrap()
            .blocks
            .push(crate::boundary::BlockChange {
                position: pos,
                state: state.world.block_at(pos.x, pos.y, pos.z).unwrap(),
            });
        let initial = crate::core::journal_entries_for_tick(&mut state, &baseline);
        let mut changes = ChangeSet::default();
        match construction {
            0 => {
                let mut writes = Vec::new();
                super::nether::create(&mut state, [8.0, 70.0, 8.0], true, &mut writes);
                state.world.fold_overlay();
                super::commit_removals(&mut state, &mut changes);
                changes.blocks.extend(
                    writes
                        .into_iter()
                        .map(|(position, state)| crate::boundary::BlockChange { position, state }),
                );
            }
            1 => {
                let mut writes = Vec::new();
                crate::end_portals::platform(&mut state, &mut writes, &mut changes);
                state.world.fold_overlay();
                changes.blocks.extend(
                    writes
                        .into_iter()
                        .map(|(position, state)| crate::boundary::BlockChange { position, state }),
                );
            }
            2 => crate::end_portals::create_gateway(&mut state, &mut changes, 0),
            3 => crate::end_portals::set_exit_portal(&mut state, &mut changes, true),
            _ => {
                load(&mut state, [96.0, 75.0, 0.0], 1);
                crate::end_portals::create_gateway(&mut state, &mut changes, 0);
                let inner = BlockPos { x: 96, y: 75, z: 0 };
                let image = crate::end_portals::gateway(&state, inner).unwrap();
                crate::end_portals::initialize_gateway(&mut state, inner, &mut changes, image);
            }
        }
        assert!(!state.block_entities.contains_key(&(pos.x, pos.y, pos.z)));
        let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let output_changes = Arc::get_mut(&mut output.changes).unwrap();
        output_changes.blocks.extend(changes.blocks);
        output_changes
            .entities_spawned
            .extend(changes.entities_spawned);
        let replacement = crate::core::journal_entries_for_tick(&mut state, &output);
        let removal = replacement
            .iter()
            .position(|entry| {
                matches!(entry, crate::journal::JournalEntry::RemoveBlockEntity { pos: cell, .. }
                if *cell == [pos.x, pos.y, pos.z])
            })
            .expect("construction removal tombstone");
        if construction == 2 {
            let post_image = replacement.iter().position(|entry| {
                matches!(entry, crate::journal::JournalEntry::OpaqueBlockEntity { pos: cell, nbt: Some(_) }
                    if *cell == [pos.x, pos.y, pos.z])
            }).unwrap();
            assert!(removal < post_image);
        }
        let directory = tempfile::tempdir().unwrap();
        let mut writer = crate::journal::JournalWriter::open(directory.path(), 1).unwrap();
        writer.append(1, baseline.tick, &initial).unwrap();
        writer
            .append(output.tick, output.tick, &replacement)
            .unwrap();
        writer.flush().unwrap();
        drop(writer);
        let batches =
            crate::journal::read_complete_batches_after(&directory.path().join("journal"), 0)
                .unwrap();
        let replay = crate::journal::replay_batches(Default::default(), &batches);
        let replay = crate::journal::replay_batches(replay, &batches);
        assert!(!replay.block_entities.contains_key(&(pos.x, pos.y, pos.z)));
        assert_eq!(
            replay
                .entities
                .values()
                .filter(|entity| entity.alive)
                .filter_map(|entity| entity.item.as_ref())
                .filter(|stack| stack.item_id == diamond)
                .map(|stack| stack.count)
                .sum::<i32>(),
            7,
            "construction {construction} retained contents exactly once",
        );
        if construction == 2 {
            let mut recovered = local(Dimension::End, Arc::clone(&registry));
            load(&mut recovered, [96.0, 75.0, 0.0], 1);
            for ((x, y, z), block) in replay.blocks {
                recovered.world.set_block(x, y, z, block);
            }
            recovered.world.fold_overlay();
            for ((x, y, z), bytes) in replay.opaque_block_entities {
                recovered
                    .world
                    .set_opaque_block_entity(BlockPos { x, y, z }, bytes);
            }
            assert!(crate::end_portals::gateway(&recovered, pos).is_ok());
        } else {
            assert!(
                replay
                    .opaque_block_entities
                    .get(&(pos.x, pos.y, pos.z))
                    .is_none_or(Option::is_none)
            );
        }
    }
}

#[test]
fn container_placed_then_broken_in_one_tick_has_no_replayed_storage() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = local(Dimension::Overworld, registry);
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let pos = BlockPos { x: 8, y: 70, z: 8 };
    let diamond = chest(&mut state, pos);
    let previous = state.world.block_at(pos.x, pos.y, pos.z).unwrap();
    state.stage_command_break(pos, previous);
    state
        .world
        .set_block(pos.x, pos.y, pos.z, state.registry.states.air);
    state.world.fold_overlay();
    let mut changes = ChangeSet::default();
    super::commit_removals(&mut state, &mut changes);
    let output_changes = Arc::get_mut(&mut output.changes).unwrap();
    output_changes.blocks.push(crate::boundary::BlockChange {
        position: pos,
        state: state.registry.states.air,
    });
    output_changes
        .entities_spawned
        .extend(changes.entities_spawned);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let replay = crate::journal::replay_batches(Default::default(), &[(1, output.tick, entries)]);
    assert!(!replay.block_entities.contains_key(&(pos.x, pos.y, pos.z)));
    assert!(
        replay
            .opaque_block_entities
            .get(&(pos.x, pos.y, pos.z))
            .is_none_or(Option::is_none)
    );
    assert_eq!(
        replay
            .entities
            .values()
            .filter_map(|entity| entity.item.as_ref())
            .filter(|stack| stack.item_id == diamond)
            .map(|stack| stack.count)
            .sum::<i32>(),
        7,
    );
}

#[test]
fn gateway_age_is_checkpointed_without_per_tick_wal_or_revision_churn() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let directory = tempfile::tempdir().unwrap();
    let mut state = local_with_directory(
        Dimension::End,
        Arc::clone(&registry),
        Some(directory.path().to_owned()),
    );
    let pos = BlockPos { x: 8, y: 70, z: 8 };
    state.tickets.insert(
        "gateway-age-fixture".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.refresh_active_chunks(state.tick);
    let gateway = state_id(&state, "minecraft:end_gateway", &[]);
    state.world.set_block(pos.x, pos.y, pos.z, gateway);
    state.world.fold_overlay();
    crate::end_portals::after_blocks(&mut state, &[(pos, gateway)]);
    state.opaque_changes.clear();
    state.tick += 1;
    crate::end_portals::tick(&mut state);
    let revision = state
        .world
        .snapshot()
        .iter()
        .find(|chunk| chunk.pos.x == 0 && chunk.pos.z == 0)
        .unwrap()
        .revision;
    for _ in 1..30 {
        state.tick += 1;
        crate::end_portals::tick(&mut state);
    }
    assert!(state.opaque_changes.is_empty());
    assert_eq!(crate::end_portals::gateway(&state, pos).unwrap().age, 30);
    assert_eq!(
        state
            .world
            .snapshot()
            .iter()
            .find(|chunk| chunk.pos.x == 0 && chunk.pos.z == 0)
            .unwrap()
            .revision,
        revision
    );
    crate::end_portals::checkpoint(&mut state);
    assert!(state.opaque_changes.is_empty());
    let (_, _, block_entities) = crate::core::checkpoint_data_for(&state, state.tick);
    let captured = state.world.capture_dirty(&block_entities);
    state.tick += 1;
    crate::end_portals::tick(&mut state);
    let written = captured.write().unwrap();
    state.world.mark_flushed(&written);
    state.world.evict_outside(&Default::default());
    assert!(
        state.world.has_chunk(0, 0),
        "new age survives an older capture acknowledgement"
    );
    let restored = local_with_directory(
        Dimension::End,
        Arc::clone(&registry),
        Some(directory.path().to_owned()),
    );
    assert_eq!(crate::end_portals::gateway(&restored, pos).unwrap().age, 30);
    let mut image = crate::end_portals::gateway(&state, pos).unwrap();
    image.age = 199;
    crate::end_portals::save_gateway(&mut state, pos, image);
    state.opaque_changes.clear();
    state.tick += 1;
    crate::end_portals::tick(&mut state);
    assert_eq!(state.opaque_changes.len(), 1, "spawning beam boundary");
    state.opaque_changes.clear();
    state.tick += 1;
    crate::end_portals::tick(&mut state);
    assert!(state.opaque_changes.is_empty());
    image = crate::end_portals::gateway(&state, pos).unwrap();
    image.age = 2399;
    crate::end_portals::save_gateway(&mut state, pos, image);
    state.opaque_changes.clear();
    state.tick += 1;
    crate::end_portals::tick(&mut state);
    assert_eq!(state.opaque_changes.len(), 1, "2400-tick beam transition");
    assert_eq!(
        crate::end_portals::gateway(&state, pos)
            .unwrap()
            .cooldown_until,
        state.tick + 40
    );
}

#[test]
fn stationary_mob_notices_newly_placed_portal_with_zero_timer() {
    let (mut state, _) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    let mob = state.spawn_mob(cow, [4.5, 70.0, 4.5], 10.0);
    // Warm a negative section verdict before editing under the unmoved body.
    portal_step(&mut state);
    let shape = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 2, 3, true);
    let portal = state_id(&state, "minecraft:nether_portal", &[("axis", "x")]);
    // Newly placed portal directly under the stationary mob: overlay-first
    // `block_at` must expose it even though no merge/fold ran after placement.
    state.world.set_block(4, 70, 4, portal);
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(
        state.entities.row(slot).entity_life.portal.elapsed,
        1,
        "new portal under stationary mob starts the timer"
    );
    assert_eq!(
        dimensions_travel::locate(&state, mob).unwrap().0,
        Dimension::Overworld,
        "destination admission postpones travel without losing contact"
    );
    assert!(named(&state, shape.cell(0, 0), "nether_portal"));
}

#[test]
fn portal_prefilter_never_misses_overlay_or_fresh_placement() {
    let (mut state, _) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    let mob = state.spawn_mob(cow, [4.5, 70.0, 4.5], 10.0);
    let shape = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 2, 3, true);
    light(&mut state, shape);
    // Folded portal under a stationary mob: the palette gate must hit even
    // though the mob never moved (no position-only caching across ticks).
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert!(
        state.entities.row(slot).entity_life.portal.elapsed > 0
            || dimensions_travel::locate(&state, mob).unwrap().0 != Dimension::Overworld,
        "folded portal under stationary mob must register contact"
    );
    // Overlay-only portal (no fold): the gate fails open via nonempty overlay.
    let (mut state, _) = worlds();
    let mob = state.spawn_mob(cow, [4.5, 70.0, 4.5], 10.0);
    let _ = frame(&mut state, BlockPos { x: 4, y: 70, z: 4 }, 2, 3, true);
    let portal = state_id(&state, "minecraft:nether_portal", &[("axis", "x")]);
    state.world.set_block(4, 70, 4, portal);
    assert!(!state.world.overlay_is_empty());
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(
        state.entities.row(slot).entity_life.portal.elapsed,
        1,
        "overlay-only portal must not be filtered out"
    );
}

#[test]
fn portal_prefilter_skips_empty_sections_but_keeps_boundary_contact() {
    let (mut state, _) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    // Mob far from any portal in an all-air section box: timers stay idle but
    // the tick still processes cooldown/elapsed bookkeeping (no early skip).
    let mob = state.spawn_mob(cow, [200.5, 70.0, 200.5], 10.0);
    load(&mut state, [200.0, 70.0, 200.0], 1);
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(state.entities.row(slot).entity_life.portal.elapsed, 0);
    assert_eq!(state.entities.row(slot).entity_life.portal.cooldown, 0);
    // Portal at a chunk boundary with the contact box spanning two columns:
    // the palette gate must hit on the resident side.
    let (mut state, _) = worlds();
    let mob = state.spawn_mob(cow, [16.1, 70.0, 8.5], 10.0);
    load(&mut state, [16.0, 70.0, 8.0], 1);
    let portal = state_id(&state, "minecraft:nether_portal", &[("axis", "x")]);
    state.world.set_block(16, 70, 8, portal);
    state.world.fold_overlay();
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(
        state.entities.row(slot).entity_life.portal.elapsed,
        1,
        "chunk-boundary portal must survive the prefilter"
    );
    assert_eq!(
        dimensions_travel::locate(&state, mob).unwrap().0,
        Dimension::Overworld,
        "destination admission postpones travel without losing contact"
    );
    // Portal one section above the mob's feet: the contact box spans two
    // 16-high sections and the gate must check both palettes.
    let (mut state, _) = worlds();
    let mob = state.spawn_mob(cow, [30.5, 15.2, 30.5], 10.0);
    load(&mut state, [30.0, 15.0, 30.0], 1);
    let portal = state_id(&state, "minecraft:nether_portal", &[("axis", "x")]);
    state.world.set_block(30, 16, 30, portal);
    state.world.fold_overlay();
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(
        state.entities.row(slot).entity_life.portal.elapsed,
        1,
        "cross-section portal must survive the prefilter"
    );
}

#[test]
fn idle_entities_stay_idle_while_armed_timers_decay() {
    let (mut state, _) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    // Idle entity far from any portal block: the idle palette gate skips the
    // per-cell exact scan and leaves the zero timer untouched.
    let idle = state.spawn_mob(cow, [200.5, 70.0, 200.5], 10.0);
    load(&mut state, [200.0, 70.0, 200.0], 1);
    state.entities.take_touched();
    portal_step(&mut state);
    let slot = state.entities.slot_of(idle).unwrap();
    assert_eq!(state.entities.row(slot).entity_life.portal.elapsed, 0);
    assert_eq!(state.entities.row(slot).entity_life.portal.cooldown, 0);
    assert!(!state.entities.touched_ids().contains(&idle));
    // Armed timer with no contact still decays through the same path.
    let slot = state.entities.slot_of(idle).unwrap();
    state.entities.row_mut(slot).entity_life.portal.elapsed = 8;
    portal_step(&mut state);
    let slot = state.entities.slot_of(idle).unwrap();
    assert_eq!(
        state.entities.row(slot).entity_life.portal.elapsed,
        4,
        "armed timer decays by four without contact"
    );
}

#[test]
fn spectator_never_scans_contact_nor_arms_timer() {
    let (mut state, id) = worlds();
    state.players.get_mut(&id).unwrap().game_mode = crate::commands::GameMode::Spectator;
    let portal = state_id(&state, "minecraft:nether_portal", &[("axis", "x")]);
    state.world.set_block(8, 70, 8, portal);
    state.world.fold_overlay();
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, [8.5, 70.0, 8.5]);
    portal_step(&mut state);
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(
        state.entities.row(slot).entity_life.portal.elapsed,
        0,
        "spectator inside a portal never starts the timer"
    );
    assert_eq!(
        state.entities.row(slot).entity_life.portal.cooldown,
        0,
        "spectator inside a portal never takes a cooldown"
    );
    assert_eq!(
        dimensions_travel::locate(&state, id).unwrap().0,
        Dimension::Overworld,
        "spectator never transfers dimensions"
    );
}

#[test]
fn warm_negative_reuses_quiet_then_wakes_on_overlay_and_new_cells() {
    let (mut state, _) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    let mob = state.spawn_mob(cow, [4.5, 70.0, 4.5], 10.0);
    // First pass banks the geometric absence under the eligible body.
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(
        state.portal_contact_stamps[slot.0 as usize].map(|stamp| stamp.entity),
        Some(mob),
        "eligible body banks geometric absence"
    );
    // Warm pass reuses the proof: no contact, timer untouched, stamp kept.
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert!(state.portal_contact_stamps[slot.0 as usize].is_some());
    assert_eq!(state.entities.row(slot).entity_life.portal.elapsed, 0);
    // Live overlay placement under the stationary body wakes the probe even
    // though nothing folded after the write.
    let portal = state_id(&state, "minecraft:nether_portal", &[("axis", "x")]);
    state.world.set_block(4, 70, 4, portal);
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(
        state.entities.row(slot).entity_life.portal.elapsed,
        1,
        "overlay portal under a warm body must still register contact"
    );
    assert!(
        state.portal_contact_stamps[slot.0 as usize].is_none(),
        "live touch drops the negative proof"
    );
    // Movement with a warm stamp onto a PRE-EXISTING folded portal: the portal
    // predates the bank, so no revision change can rescue a broken cell-key
    // check — only exact re-probing detects the contact.
    load(&mut state, [210.0, 70.0, 210.0], 1);
    state.world.set_block(200, 70, 200, portal);
    state.world.fold_overlay();
    // Bank the warm absence away from the portal first, on the same revision.
    state.entities.set_pos(slot, [210.5, 70.0, 210.5]);
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    let warm = state.portal_contact_stamps[slot.0 as usize]
        .expect("absence away from the portal banks warm");
    assert_eq!(warm.entity, mob);
    // Step into the portal's cells: coverage changes, so the warm stamp must
    // not suppress the exact scan even though revisions are unchanged.
    state.entities.set_pos(slot, [200.5, 70.0, 200.5]);
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert!(
        state.entities.row(slot).entity_life.portal.elapsed > 0
            || dimensions_travel::locate(&state, mob).unwrap().0 != Dimension::Overworld,
        "moving a warm stamp onto a pre-existing portal must register contact"
    );
    assert!(
        state.portal_contact_stamps[slot.0 as usize].is_none_or(
            |stamp| stamp.entity == mob && (stamp.min, stamp.max) != (warm.min, warm.max)
        ),
        "contact clears the warm proof; a fresh bank must cover the new cells"
    );
}

#[test]
fn warm_negative_keeps_non_default_cooldown_ticking() {
    let (mut state, _) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    let mob = state.spawn_mob(cow, [200.5, 70.0, 200.5], 10.0);
    load(&mut state, [200.0, 70.0, 200.0], 1);
    let slot = state.entities.slot_of(mob).unwrap();
    state.entities.row_mut(slot).entity_life.portal.cooldown = 7;
    // First pass probes and banks the absence, then decays the cooldown.
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(state.entities.row(slot).entity_life.portal.cooldown, 6);
    assert_eq!(
        state.portal_contact_stamps[slot.0 as usize].map(|stamp| stamp.entity),
        Some(mob)
    );
    // Warm passes skip both scans yet still run the timer bookkeeping.
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(state.entities.row(slot).entity_life.portal.cooldown, 5);
    assert_eq!(state.entities.row(slot).entity_life.portal.elapsed, 0);
    portal_step(&mut state);
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(state.entities.row(slot).entity_life.portal.cooldown, 4);
    // An armed elapsed timer decays the same way under a warm proof.
    let restless = state.spawn_mob(cow, [210.5, 70.0, 210.5], 10.0);
    load(&mut state, [210.0, 70.0, 210.0], 1);
    let slot = state.entities.slot_of(restless).unwrap();
    state.entities.row_mut(slot).entity_life.portal.elapsed = 8;
    portal_step(&mut state);
    let slot = state.entities.slot_of(restless).unwrap();
    assert_eq!(state.entities.row(slot).entity_life.portal.elapsed, 4);
    portal_step(&mut state);
    let slot = state.entities.slot_of(restless).unwrap();
    assert_eq!(state.entities.row(slot).entity_life.portal.elapsed, 0);
}

#[test]
fn end_inside_shape_dip_never_banks_geometric_absence() {
    let (mut state, id) = worlds();
    state
        .world
        .set_block(8, 70, 8, state_id(&state, "minecraft:end_portal", &[]));
    state.world.fold_overlay();
    load(
        state.other_dimensions.get_mut(&Dimension::End).unwrap(),
        [100.0, 49.0, 0.0],
        1,
    );
    let slot = state.entities.slot_of(id).unwrap();
    // Feet exactly at y + 0.75: same AABB cells as the dip below, but the
    // 26.1 inside shape (Block.column(16, 6, 12)) does not touch the body.
    state.entities.set_pos(slot, [8.5, 70.75, 8.5]);
    portal_step(&mut state);
    assert_eq!(
        dimensions_travel::locate(&state, id).unwrap().0,
        Dimension::Overworld
    );
    let slot = state.entities.slot_of(id).unwrap();
    assert!(
        state.portal_contact_stamps[slot.0 as usize].is_none(),
        "an End block outside the inside shape must never bank absence"
    );
    // Same cells, sub-cell dip into the shape: contact must still register.
    state.entities.set_pos(slot, [8.5, 70.749, 8.5]);
    portal_step(&mut state);
    assert_eq!(
        dimensions_travel::locate(&state, id).unwrap().0,
        Dimension::End,
        "sub-cell dip into the End inside shape must travel"
    );
}

#[test]
fn slot_compaction_never_reuses_dead_proof() {
    let (mut state, _) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    // Both bodies share the exact same spot, so covered cells are identical
    // and only the EntityId guard distinguishes the proofs.
    let a = state.spawn_mob(cow, [4.5, 70.0, 4.5], 10.0);
    let b = state.spawn_mob(cow, [4.5, 70.0, 4.5], 10.0);
    portal_step(&mut state);
    // Removing the lower slot swap-removes the survivor into it while the
    // stamp still names the dead occupant over the same cells.
    let slot_a = state.entities.slot_of(a).unwrap();
    let _ = state.entities.remove(slot_a);
    portal_step(&mut state);
    let slot_b = state.entities.slot_of(b).unwrap();
    assert_eq!(
        slot_b, slot_a,
        "compaction moves the survivor into the freed slot"
    );
    assert_eq!(
        state.portal_contact_stamps[slot_b.0 as usize].map(|stamp| stamp.entity),
        Some(b),
        "the survivor re-probes instead of trusting the dead slot's proof"
    );
}

/// Dense mob field over a few Nether/End portal blocks: mobs drift in and out
/// of contact, idle bodies bank warm proofs, contacts arm timers and some
/// travel. Returns a per-step transcript of every dimension's rows.
fn portal_field(
    mobs: usize,
    steps: u64,
    step: impl Fn(&mut TickState, &mut BTreeMap<Dimension, ChangeSet>),
) -> Vec<String> {
    let (mut state, _) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    let nether = state_id(&state, "minecraft:nether_portal", &[("axis", "x")]);
    let end = state_id(&state, "minecraft:end_portal", &[]);
    for (x, z, block) in [(3, 3, nether), (10, 5, nether), (8, 8, end), (12, 12, end)] {
        state.world.set_block(x, 70, z, block);
    }
    state.world.fold_overlay();
    load(
        state.other_dimensions.get_mut(&Dimension::End).unwrap(),
        [100.0, 49.0, 0.0],
        1,
    );
    let at = |index: usize, step: u64| {
        let mix = (index as u64 + 1)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(step.wrapping_mul(0xbf58_476d_1ce4_e5b9));
        let mix = (mix ^ (mix >> 29)).wrapping_mul(0x94d0_49bb_1331_11eb);
        [
            (mix & 0xfff) as f64 / 4096.0 * 15.0 + 0.5,
            70.0,
            ((mix >> 12) & 0xfff) as f64 / 4096.0 * 15.0 + 0.5,
        ]
    };
    let ids: Vec<_> = (0..mobs)
        .map(|index| state.spawn_mob(cow, at(index, 0), 10.0))
        .collect();
    let mut transcript = Vec::new();
    for round in 0..steps {
        if round > 0 {
            for (index, id) in ids.iter().enumerate() {
                let Some((dimension, slot)) = dimensions_travel::locate(&state, *id) else {
                    continue;
                };
                let world = if state.dimension == dimension {
                    &mut state
                } else {
                    state.other_dimensions.get_mut(&dimension).unwrap()
                };
                match (index as u64 + round * 5) % 9 {
                    0 | 1 => world.entities.set_pos(slot, at(index, round)),
                    2 => {
                        let old = world.entities.pos(slot);
                        world
                            .entities
                            .set_pos(slot, [old[0] + 0.002, old[1], old[2]]);
                    }
                    _ => {}
                }
            }
        }
        state.tick += 1;
        for child in state.other_dimensions.values_mut() {
            child.tick = state.tick;
        }
        let mut changes = BTreeMap::new();
        step(&mut state, &mut changes);
        let mut rows = format!("{changes:?}\n");
        for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
            let world: &TickState = if state.dimension == dimension {
                &state
            } else {
                &state.other_dimensions[&dimension]
            };
            for slot in 0..world.entities.len() {
                let slot = crate::entities::EntitySlot(slot as u32);
                let row = world.entities.row(slot);
                let stamp = world
                    .portal_contact_stamps
                    .get(slot.0 as usize)
                    .copied()
                    .flatten();
                rows += &format!(
                    "{dimension:?} {:?} {:?} {} {} {:?}\n",
                    world.entities.body(slot).id,
                    world.entities.pos(slot),
                    row.entity_life.portal.cooldown,
                    row.entity_life.portal.elapsed,
                    stamp.map(|stamp| (stamp.entity, stamp.min, stamp.max, stamp.position))
                );
            }
        }
        transcript.push(rows);
    }
    transcript
}

#[test]
fn parallel_portal_classifier_matches_all_serial_on_any_thread_count() {
    let mobs = crate::contact_hazards::MIN_PARALLEL_ROWS / 2 * 3;
    let reference = portal_field(mobs, 5, |state, changes| {
        travel::finish_with(state, changes, None)
    });
    assert!(
        reference.iter().any(|round| round.contains("End")
            && round
                .lines()
                .filter(|line| line.starts_with("End "))
                .count()
                > 0),
        "fixture must send bodies through the End portal"
    );
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let got = pool.install(|| {
            portal_field(mobs, 5, |state, changes| {
                travel::finish_with(state, changes, Some(0))
            })
        });
        assert_eq!(got.len(), reference.len());
        for (round, (got, want)) in got.iter().zip(&reference).enumerate() {
            assert!(got == want, "round {round} diverged on {threads} thread(s)");
        }
    }
}

/// Release micro: `cargo test --release -p mc-core --lib portal_classifier_micro -- --ignored --nocapture`.
/// 100k idle mobs; B = all-serial body, A = parallel classifier + serial
/// apply, warm passes alternating B,A,B,A.
#[test]
#[ignore = "release micro: prints portal pass timings"]
fn portal_classifier_micro() {
    let (mut state, _) = worlds();
    let cow = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    let slots: Vec<_> = (0..100_000_u64)
        .map(|index| {
            let mix = (index + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let at = [
                ((mix >> 8) & 0xfff) as f64 / 4096.0 * 15.0 + 0.5,
                70.0,
                ((mix >> 24) & 0xfff) as f64 / 4096.0 * 15.0 + 0.5,
            ];
            let id = state.spawn_mob(cow, at, 10.0);
            state.entities.slot_of(id).unwrap()
        })
        .collect();
    let mut changes = BTreeMap::new();
    travel::finish_with(&mut state, &mut changes, None);
    for moving in [false, true] {
        for (label, min_parallel) in [("B", None), ("A", Some(0)), ("B", None), ("A", Some(0))] {
            let mut spent = std::time::Duration::ZERO;
            for pass in 0..20_u32 {
                if moving {
                    for slot in slots.iter().step_by(10) {
                        let old = state.entities.pos(*slot);
                        let dx = if pass % 2 == 0 { 0.003 } else { -0.003 };
                        state.entities.set_pos(*slot, [old[0] + dx, old[1], old[2]]);
                    }
                }
                let started = std::time::Instant::now();
                travel::finish_with(&mut state, &mut changes, min_parallel);
                spent += started.elapsed();
            }
            println!(
                "portal_classifier_micro moving={moving} {label}: {:.3} ms/pass",
                spent.as_secs_f64() * 1000.0 / 20.0
            );
        }
    }
}
