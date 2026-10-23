//! Double chests through the tick, on a stone platform in open sky: forming
//! by placement, one 54-slot menu over both storages, breaking one half.

use std::sync::Arc;

use crate::boundary::{
    BlockPos, ClickMode, ConnectionId, DigAction, EntityId, Hand, PlayerChange, PlayerInput,
};
use crate::inventory::MenuKind;
use crate::registry::CoreRegistry;
use crate::tick::{BlockEntityState, TickState, run_tick};
use crate::world::ChunkWorld;

/// Platform height: far above the generated terrain.
const Y: i32 = 200;
/// Protocol id of `minecraft:generic_9x6`.
const GENERIC_9X6: u32 = 5;

fn test_state() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry loads"));
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().expect("test terrain spawn");
    let world_spawn = [spawn.block_x, spawn.surface_y + 1, spawn.block_z];
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        world_spawn,
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(name).expect("id");
    let default = state.registry.blocks.block(&id).expect("block").default;
    let mut full = state
        .registry
        .blocks
        .by_id(default)
        .expect("default")
        .properties
        .clone();
    for (key, value) in props {
        if let Some(entry) = full.iter_mut().find(|(name, _)| name == key) {
            entry.1 = (*value).to_owned();
        }
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &full)
        .expect("state")
        .0
}

fn item(state: &TickState, name: &str, count: i32) -> mc_data::ItemStack {
    let id = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(name).expect("id"))
        .expect("item");
    mc_data::ItemStack::new(id, count)
}

fn chest(state: &TickState, kind: &str) -> u32 {
    block(
        state,
        "minecraft:chest",
        &[("facing", "north"), ("type", kind)],
    )
}

type Drains = Vec<(ConnectionId, Vec<PlayerChange>)>;

struct Rig {
    state: TickState,
    player: EntityId,
    /// Left and right halves of the chest row (left west of right).
    left: BlockPos,
    right: BlockPos,
}

impl Rig {
    fn new() -> Self {
        let mut state = test_state();
        let _ = run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(1),
                uuid: uuid::Uuid::new_v4(),
                name: "chests".to_owned(),
                permission_level: 0,
            }],
        );
        let want = state.desired_chunks();
        state.world.ensure_blocking(&want);
        let player = state
            .connections
            .entity_of(ConnectionId(1))
            .expect("joined");
        let slot = state.entities.slot_of(player).expect("slot");
        let at = state.entities.pos(slot);
        let left = BlockPos {
            x: at[0].floor() as i32,
            y: Y + 1,
            z: at[2].floor() as i32,
        };
        let right = BlockPos {
            x: left.x + 1,
            ..left
        };
        let stone = state.registry.states.stone;
        for dx in -4..=6 {
            for dz in -4..=4 {
                state.world.set_block(left.x + dx, Y, left.z + dz, stone);
            }
        }
        state.world.fold_overlay();
        state
            .entities
            .set_pos(slot, [at[0], f64::from(Y + 1), at[2] + 2.0]);
        Self {
            state,
            player,
            left,
            right,
        }
    }

    /// A finished double chest holding `left_items` / `right_items` from
    /// slot 0 of each half.
    fn double_chest(
        &mut self,
        left_items: &[mc_data::ItemStack],
        right_items: &[mc_data::ItemStack],
    ) {
        let (left_state, right_state) = (chest(&self.state, "left"), chest(&self.state, "right"));
        for (pos, state) in [(self.left, left_state), (self.right, right_state)] {
            self.state.world.set_block(pos.x, pos.y, pos.z, state);
        }
        self.state.world.fold_overlay();
        for (pos, items) in [(self.left, left_items), (self.right, right_items)] {
            let mut slots = vec![mc_data::ItemStack::EMPTY; 27];
            slots[..items.len()].clone_from_slice(items);
            self.state.block_entities.insert(
                (pos.x, pos.y, pos.z),
                BlockEntityState {
                    slots,
                    furnace: None,
                },
            );
        }
    }

    fn input(&mut self, input: PlayerInput) -> Drains {
        run_tick(&mut self.state, &[input]).1
    }

    fn open(&mut self, pos: BlockPos) -> Drains {
        self.input(PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: pos,
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        })
    }

    fn click(&mut self, slot: i16) {
        let menu = self.state.players[&self.player]
            .open
            .as_ref()
            .expect("open menu");
        let (window, state_id) = (menu.window, menu.state_id);
        self.input(PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window,
            state_id,
            slot,
            button: 0,
            mode: ClickMode::Pickup,
        });
    }

    fn dig(&mut self, pos: BlockPos) -> crate::boundary::TickOutput {
        run_tick(
            &mut self.state,
            &[PlayerInput::Dig {
                connection: ConnectionId(1),
                position: pos,
                action: DigAction::Finish,
                sequence: 2,
            }],
        )
        .0
    }

    fn storage(&self, pos: BlockPos) -> Option<&[mc_data::ItemStack]> {
        self.state
            .block_entities
            .get(&(pos.x, pos.y, pos.z))
            .map(|record| record.slots.as_slice())
    }

    fn block_at(&self, pos: BlockPos) -> Option<u32> {
        self.state.world.block_at(pos.x, pos.y, pos.z)
    }
}

fn opened(drains: &Drains) -> Option<(u32, String)> {
    drains
        .iter()
        .flat_map(|(_, queue)| queue.iter())
        .find_map(|change| match change {
            PlayerChange::MenuOpened {
                menu_type, title, ..
            } => Some((*menu_type, title.clone())),
            _ => None,
        })
}

fn contents(drains: &Drains) -> Vec<Option<mc_data::ItemStack>> {
    drains
        .iter()
        .flat_map(|(_, queue)| queue.iter())
        .find_map(|change| match change {
            PlayerChange::MenuContents { slots, .. } => Some(slots.clone()),
            _ => None,
        })
        .expect("menu contents")
}

#[test]
fn integrity_unloaded_double_chest_refuses_pickup_shift_and_drag() {
    for mode in [
        ClickMode::Pickup,
        ClickMode::QuickMove,
        ClickMode::QuickCraft,
    ] {
        let mut rig = Rig::new();
        // Put the partner across a chunk boundary so only that half unloads.
        rig.left.x = rig.left.x.div_euclid(16) * 16 + 15;
        rig.right.x = rig.left.x + 1;
        let slot = rig.state.entities.slot_of(rig.player).unwrap();
        rig.state.entities.set_pos(
            slot,
            [
                f64::from(rig.left.x),
                f64::from(Y + 1),
                f64::from(rig.left.z) + 2.0,
            ],
        );
        let diamond = item(&rig.state, "minecraft:diamond", 3);
        rig.double_chest(std::slice::from_ref(&diamond), &[]);
        rig.open(rig.left);
        let cursor = item(&rig.state, "minecraft:stone", 2);
        rig.state
            .players
            .get_mut(&rig.player)
            .unwrap()
            .open
            .as_mut()
            .unwrap()
            .carried = cursor;
        let menu = rig.state.players[&rig.player].open.as_ref().unwrap();
        let window = menu.window;
        if mode == ClickMode::QuickCraft {
            for (slot, button) in [(-999, 0), (53, 1)] {
                let state_id = rig.state.players[&rig.player]
                    .open
                    .as_ref()
                    .unwrap()
                    .state_id;
                rig.input(PlayerInput::ContainerClick {
                    connection: ConnectionId(1),
                    window,
                    state_id,
                    slot,
                    button,
                    mode,
                });
            }
        }
        let chunk = crate::boundary::ChunkPos {
            x: rig.right.x.div_euclid(16),
            z: rig.right.z.div_euclid(16),
        };
        rig.state.world.finish_inhabited_tick(rig.state.tick + 1);
        rig.state
            .world
            .set_active_chunks(Default::default(), rig.state.tick + 1);
        // Freeze the inactive inhabited interval, then acknowledge its revision.
        rig.state.world.capture_dirty(&Default::default());
        rig.state
            .world
            .mark_flushed(&[(chunk, rig.state.world.storage_revision(chunk))]);
        let mut want = rig.state.desired_chunks();
        want.remove(&chunk);
        assert!(rig.state.world.evict_outside(&want).contains(&chunk));
        assert_eq!(rig.block_at(rig.right), None);
        let state_id = rig.state.players[&rig.player]
            .open
            .as_ref()
            .unwrap()
            .state_id;
        let drains = rig.input(PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window,
            state_id,
            slot: if mode == ClickMode::QuickCraft {
                -999
            } else {
                53
            },
            button: if mode == ClickMode::QuickCraft { 2 } else { 0 },
            mode,
        });
        assert!(rig.state.players[&rig.player].open.is_none());
        assert!(!rig.state.open_containers.contains_key(&rig.player));
        assert!(drains.iter().flat_map(|(_, changes)| changes).any(|change|
            matches!(change, PlayerChange::MenuClosed { window: closed } if *closed == window)));
        assert_eq!(rig.storage(rig.left).unwrap()[0], diamond);
        assert!(
            rig.storage(rig.right)
                .unwrap()
                .iter()
                .all(mc_data::ItemStack::is_empty)
        );
        assert_eq!(
            rig.state.players[&rig.player]
                .inventory
                .slots
                .iter()
                .map(|stack| stack.count)
                .sum::<i32>(),
            2
        );
    }
}

#[test]
fn integrity_opening_chest_preserves_window_zero_custody_in_wal_and_checkpoint() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    for cursor in [false, true] {
        for full in [false, true] {
            let mut rig = Rig::new();
            rig.double_chest(&[], &[]);
            let diamond = item(&rig.state, "minecraft:diamond", 3);
            let stone = item(&rig.state, "minecraft:stone", 64);
            let player = rig.state.players.get_mut(&rig.player).unwrap();
            if full {
                player.inventory.slots.fill(stone);
            }
            if cursor {
                player.inventory_menu.carried = diamond.clone();
            } else {
                player.inventory_menu.grid[0] = diamond.clone();
            }
            let (output, _, _) = run_tick(
                &mut rig.state,
                &[PlayerInput::UseItemOn {
                    connection: ConnectionId(1),
                    hand: Hand::Main,
                    position: rig.left,
                    face: 1,
                    cursor: [0.5; 3],
                    sequence: 1,
                }],
            );
            assert!(rig.state.players[&rig.player].open.is_some());
            let uuid = rig
                .state
                .entities
                .row(rig.state.entities.slot_of(rig.player).unwrap())
                .uuid
                .to_string();
            let record =
                crate::core::player_record_for(&rig.state, rig.player, output.tick).unwrap();
            let entries = crate::core::journal_entries_for_tick(&mut rig.state, &output);
            let dir = tempfile::tempdir().unwrap();
            let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
            writer.append(1, output.tick, &entries).unwrap();
            writer.flush().unwrap();
            drop(writer);
            let replay = replay_batches(
                Replay::default(),
                &read_complete_batches_after(&dir.path().join("journal"), 0).unwrap(),
            );
            let count = |player: &crate::journal::ReplayPlayer| {
                player
                    .inventory
                    .iter()
                    .chain(std::iter::once(&player.carried))
                    .chain(player.grid.iter())
                    .filter(|stack| stack.item_id == diamond.item_id)
                    .map(|stack| stack.count)
                    .sum::<i32>()
            };
            let dropped: i32 = replay
                .entities
                .values()
                .filter_map(|entity| entity.item.as_ref())
                .filter(|stack| stack.item_id == diamond.item_id)
                .map(|stack| stack.count)
                .sum();
            assert_eq!(count(&replay.players[&uuid]) + dropped, 3);
            assert_eq!(dropped, if full { 3 } else { 0 });
            let config = crate::config::CoreConfig::new(dir.path().join("checkpoint"), 42);
            let restored = crate::native_checkpoint::tests::roundtrip_player(
                &config,
                &uuid,
                &record,
                &rig.state.registry.items,
            )
            .unwrap();
            assert_eq!(count(&restored), if full { 0 } else { 3 });
        }
    }
}

#[test]
fn placing_a_chest_beside_a_chest_forms_a_double_chest() {
    let mut rig = Rig::new();
    let chests = item(&rig.state, "minecraft:chest", 2);
    rig.state
        .players
        .get_mut(&rig.player)
        .expect("player")
        .inventory
        .slots[0] = chests;
    let floor = |pos: BlockPos| BlockPos { y: Y, ..pos };
    for pos in [rig.left, rig.right] {
        rig.input(PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: floor(pos),
            face: 1,
            cursor: [0.5, 1.0, 0.5],
            sequence: 1,
        });
    }
    assert_eq!(rig.block_at(rig.left), Some(chest(&rig.state, "left")));
    assert_eq!(rig.block_at(rig.right), Some(chest(&rig.state, "right")));
    // The placer's two chests are spent.
    assert!(rig.state.players[&rig.player].inventory.slots[0].is_empty());
}

#[test]
fn a_double_chest_opens_54_slots_over_both_storages() {
    let mut rig = Rig::new();
    let (dirt, stone, planks) = (
        item(&rig.state, "minecraft:dirt", 3),
        item(&rig.state, "minecraft:stone", 1),
        item(&rig.state, "minecraft:oak_planks", 2),
    );
    // Left half: stone, planks; right half: dirt.
    rig.double_chest(
        &[stone.clone(), planks.clone()],
        std::slice::from_ref(&dirt),
    );
    let (left, right) = (rig.left, rig.right);

    // Either half opens the same menu: the right half's 27 slots first.
    let drains = rig.open(left);
    assert_eq!(
        opened(&drains),
        Some((GENERIC_9X6, "Large Chest".to_owned()))
    );
    assert_eq!(
        rig.state.players[&rig.player]
            .open
            .as_ref()
            .map(|menu| menu.kind),
        Some(MenuKind::Chest { rows: 6 })
    );
    let slots = contents(&drains);
    assert_eq!(slots.len(), 54 + 36);
    assert_eq!(slots[0], Some(dirt.clone()));
    assert_eq!(slots[27], Some(stone.clone()));
    assert_eq!(slots[28], Some(planks.clone()));
    assert_eq!(slots[1], None);

    // Move the left half's stone (menu slot 27) into the right half (slot 5).
    rig.click(27);
    rig.click(5);
    assert_eq!(rig.storage(right).expect("right")[5], stone);
    assert_eq!(rig.storage(right).expect("right")[0], dirt);
    assert!(rig.storage(left).expect("left")[0].is_empty());
    assert_eq!(rig.storage(left).expect("left")[1], planks);

    // Closing and opening from the other half shows the new arrangement.
    rig.input(PlayerInput::ContainerClose {
        connection: ConnectionId(1),
        window: 1,
    });
    let slots = contents(&rig.open(right));
    assert_eq!(slots.len(), 54 + 36);
    assert_eq!(slots[5], Some(stone));
    assert_eq!(slots[28], Some(planks));
    assert_eq!(slots[27], None);
}

#[test]
fn breaking_one_half_keeps_the_other_halfs_items() {
    for break_right in [true, false] {
        let mut rig = Rig::new();
        let (dirt, planks) = (
            item(&rig.state, "minecraft:dirt", 3),
            item(&rig.state, "minecraft:oak_planks", 2),
        );
        rig.double_chest(std::slice::from_ref(&planks), std::slice::from_ref(&dirt));
        let (left, right) = (rig.left, rig.right);
        rig.open(left);
        assert!(rig.state.open_containers.contains_key(&rig.player));

        let (broken, kept, lost, kept_items) = if break_right {
            (right, left, &dirt, &planks)
        } else {
            (left, right, &planks, &dirt)
        };
        let output = rig.dig(broken);
        let air = rig.state.registry.states.air;
        assert_eq!(rig.block_at(broken), Some(air));
        // The survivor is a single chest again, with its own items.
        assert_eq!(rig.block_at(kept), Some(chest(&rig.state, "single")));
        assert_eq!(rig.storage(kept).expect("kept")[0], *kept_items);
        assert!(rig.storage(broken).is_none());
        assert!(
            rig.state
                .block_entity_removed
                .contains(&(broken.x, broken.y, broken.z))
        );
        // Only the broken half's contents dropped, and the viewer lost the menu.
        let dropped: Vec<u32> = output
            .changes
            .entities_spawned
            .iter()
            .filter_map(|id| rig.state.entities.slot_of_any(*id))
            .filter_map(|slot| {
                rig.state
                    .entities
                    .row(slot)
                    .item
                    .as_ref()
                    .map(|row| row.item_id)
            })
            .collect();
        assert!(dropped.contains(&lost.item_id), "{dropped:?}");
        assert!(!dropped.contains(&kept_items.item_id), "{dropped:?}");
        assert!(rig.state.open_containers.is_empty());
        assert!(rig.state.players[&rig.player].open.is_none());

        // The survivor now opens as a plain 27-slot chest.
        let drains = rig.open(kept);
        assert_eq!(opened(&drains), Some((2, "Chest".to_owned())));
        assert_eq!(contents(&drains).len(), 27 + 36);
    }
}
