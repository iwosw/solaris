//! Acceptance tests for resident work and squad order execution (C4).
//!
//! Every test drives the real order executor over a real plugin storage journal,
//! the real regional entity owner and a real world storage kernel: nothing here
//! asserts plumbing. The scenarios are the contract's A08–A12 plus the
//! demobilisation rule.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use mc_data::Identifier;
use mc_data::blocks::solaris_required_blocks_report;
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_entity::Vec3;
use mc_protocol::codec;
use mc_script::{
    ScriptBlockPosition, ScriptFormation, ScriptFormationKind, ScriptInventoryEndpoint,
    ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptOwnedInventoryOperation, ScriptResidentOrder,
    ScriptResidentOrderOperation, ScriptResidentOrderResult, ScriptResidentWorkOrder,
    ScriptWorkArea, resident_generation_id,
};
use mc_world::{BlockPos, BlockStateId, Chunk, ChunkPos, WorldStorage};
use uuid::Uuid;

use crate::play::resident_work::{LiveResidentWorld, ResidentWorld, ResidentWorldEdit};
use crate::play::{ArrowPhysicsFact, EntityPhysicsStep, SessionRegistry};
use crate::script::storage::PluginStorage;
use crate::script::storage::resident_orders::{DurableResidentOrderChange, DurableResidentStack};
use crate::script::storage::world_inventory::InventoryRuntime;
use crate::server::WorldHandle;

#[path = "resident_order_tests/inventory.rs"]
mod inventory;

#[path = "resident_order_tests/movement_work.rs"]
mod movement_work;

#[path = "resident_order_tests/order_lifecycle.rs"]
mod order_lifecycle;

#[path = "resident_order_tests/combat_recovery.rs"]
mod combat_recovery;

const OWNER: &str = "settlement";
const WORLD_IDENTITY: &str = "resident-order-world";
/// Flat terrain surface of the test world.
const SURFACE_Y: i32 = 63;

/// Test adapter that replaces the source block after loot preview but before
/// the simulation consumes the preview's precondition.
struct ReplacingResidentWorld {
    inner: Arc<LiveResidentWorld>,
    world: WorldHandle,
    replacement: BlockStateId,
    only_placements: bool,
}

impl ResidentWorld for ReplacingResidentWorld {
    fn dimension_loaded(&self, dimension: &str) -> bool {
        self.inner.dimension_loaded(dimension)
    }

    fn block(
        &self,
        dimension: &str,
        pos: [i32; 3],
    ) -> Option<crate::play::resident_work::ResidentBlock> {
        self.inner.block(dimension, pos)
    }

    fn standable(&self, dimension: &str, pos: [i32; 3]) -> Option<bool> {
        self.inner.standable(dimension, pos)
    }

    fn route_open(&self, dimension: &str, from: [i32; 3], to: [i32; 3]) -> Option<bool> {
        self.inner.route_open(dimension, from, to)
    }

    fn line_of_sight(&self, dimension: &str, from: Vec3, to: Vec3) -> Option<bool> {
        self.inner.line_of_sight(dimension, from, to)
    }

    fn foreign_zone_overlaps(
        &self,
        plugin_id: &str,
        dimension: &str,
        min: [i32; 3],
        max: [i32; 3],
    ) -> bool {
        self.inner
            .foreign_zone_overlaps(plugin_id, dimension, min, max)
    }

    fn state_for(&self, block_path: &str) -> Option<u32> {
        self.inner.state_for(block_path)
    }

    fn crop_is_mature(&self, state: u32) -> bool {
        self.inner.crop_is_mature(state)
    }

    fn preview_break(
        &self,
        dimension: &str,
        pos: [i32; 3],
        expected_state: u32,
        tool: Option<&str>,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        self.inner
            .preview_break(dimension, pos, expected_state, tool)
    }

    fn commit_world_edits<'a>(
        &'a self,
        plugin_id: &'a str,
        dimension: &'a str,
        breaks: &'a [ResidentWorldEdit],
        receipt: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + 'a>>
    {
        let inner = Arc::clone(&self.inner);
        let world = Arc::clone(&self.world);
        let replacement = self.replacement;
        let only_placements = self.only_placements;
        Box::pin(async move {
            let preview = breaks
                .first()
                .ok_or(ScriptOperationFailure::InvalidRequest)?;
            if !only_placements || preview.drops.is_empty() {
                world
                    .lock()
                    .await
                    .set_block_at(preview.precondition.pos, replacement)
                    .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)?;
            }
            inner
                .commit_world_edits(plugin_id, dimension, breaks, receipt)
                .await
        })
    }

    fn preview_place(
        &self,
        dimension: &str,
        pos: [i32; 3],
        state: u32,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        self.inner.preview_place(dimension, pos, state)
    }
}

/// Test-only protection authority whose transition models an accepted zone
/// removal while every other live-world read remains real.
struct ProtectionGatedResidentWorld {
    inner: Arc<LiveResidentWorld>,
    protected: Arc<AtomicBool>,
    standable_queries: Arc<AtomicUsize>,
    route_queries: Arc<AtomicUsize>,
}

impl ResidentWorld for ProtectionGatedResidentWorld {
    fn dimension_loaded(&self, dimension: &str) -> bool {
        self.inner.dimension_loaded(dimension)
    }

    fn block(
        &self,
        dimension: &str,
        pos: [i32; 3],
    ) -> Option<crate::play::resident_work::ResidentBlock> {
        self.inner.block(dimension, pos)
    }

    fn standable(&self, dimension: &str, pos: [i32; 3]) -> Option<bool> {
        self.standable_queries.fetch_add(1, Ordering::Relaxed);
        self.inner.standable(dimension, pos)
    }

    fn route_open(&self, dimension: &str, from: [i32; 3], to: [i32; 3]) -> Option<bool> {
        self.route_queries.fetch_add(1, Ordering::Relaxed);
        self.inner.route_open(dimension, from, to)
    }

    fn line_of_sight(&self, dimension: &str, from: Vec3, to: Vec3) -> Option<bool> {
        self.inner.line_of_sight(dimension, from, to)
    }

    fn foreign_zone_overlaps(
        &self,
        _plugin_id: &str,
        _dimension: &str,
        _min: [i32; 3],
        _max: [i32; 3],
    ) -> bool {
        self.protected.load(Ordering::Acquire)
    }

    fn state_for(&self, block_path: &str) -> Option<u32> {
        self.inner.state_for(block_path)
    }

    fn crop_is_mature(&self, state: u32) -> bool {
        self.inner.crop_is_mature(state)
    }

    fn preview_break(
        &self,
        dimension: &str,
        pos: [i32; 3],
        expected_state: u32,
        tool: Option<&str>,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        self.inner
            .preview_break(dimension, pos, expected_state, tool)
    }

    fn commit_world_edits<'a>(
        &'a self,
        plugin_id: &'a str,
        dimension: &'a str,
        edits: &'a [ResidentWorldEdit],
        receipt: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + 'a>>
    {
        self.inner
            .commit_world_edits(plugin_id, dimension, edits, receipt)
    }

    fn preview_place(
        &self,
        dimension: &str,
        pos: [i32; 3],
        state: u32,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        self.inner.preview_place(dimension, pos, state)
    }
}

struct Fixture {
    storage_root: tempfile::TempDir,
    #[allow(dead_code)]
    world_root: tempfile::TempDir,
    blocks: Arc<mc_world::BlockRegistry>,
    world: WorldHandle,
    sessions: Arc<SessionRegistry>,
    runtime: InventoryRuntime,
}

impl Fixture {
    fn new(wall: bool) -> Self {
        Self::with_options(wall, None, false, None, None)
    }

    fn with_replacement(wall: bool, replacement: Option<&str>) -> Self {
        Self::with_options(wall, replacement, false, None, None)
    }

    fn with_replant_replacement(wall: bool, replacement: &str) -> Self {
        Self::with_options(wall, Some(replacement), true, None, None)
    }

    fn with_protection_gate(protected: Arc<AtomicBool>) -> Self {
        Self::with_options(false, None, false, Some(protected), None)
    }

    fn with_query_counts(
        wall: bool,
        standable: Arc<AtomicUsize>,
        routes: Arc<AtomicUsize>,
    ) -> Self {
        Self::with_options(
            wall,
            None,
            false,
            Some(Arc::new(AtomicBool::new(false))),
            Some((standable, routes)),
        )
    }

    /// Seeds the ripe crop the crop and deposit contracts work on. Called by the
    /// tests that need it instead of seeding every fixture.
    fn seed_crop(&self) {
        let farmland = state_of(&self.blocks, "farmland");
        let wheat = state_with_props(&self.blocks, "wheat", &[("age", "7")]);
        self.set_fixture_blocks(&[
            (
                BlockPos {
                    x: 2,
                    y: SURFACE_Y,
                    z: 2,
                },
                farmland,
            ),
            (
                BlockPos {
                    x: 2,
                    y: SURFACE_Y + 1,
                    z: 2,
                },
                wheat,
            ),
        ]);
    }

    /// Seeds the crop that sits on the region edge for the boundary contract.
    fn seed_edge_crop(&self) {
        let farmland = state_of(&self.blocks, "farmland");
        let wheat = state_with_props(&self.blocks, "wheat", &[("age", "7")]);
        self.set_fixture_blocks(&[
            (
                BlockPos {
                    x: 0,
                    y: SURFACE_Y,
                    z: 2,
                },
                farmland,
            ),
            (
                BlockPos {
                    x: 0,
                    y: SURFACE_Y + 1,
                    z: 2,
                },
                wheat,
            ),
        ]);
    }

    /// Seeds the bounded natural tree the cut-tree contracts fell.
    fn seed_tree(&self) {
        let log = state_of(&self.blocks, "oak_log");
        let leaves = state_of(&self.blocks, "oak_leaves");
        let mut writes = vec![
            (
                BlockPos {
                    x: 6,
                    y: SURFACE_Y + 1,
                    z: 6,
                },
                log,
            ),
            (
                BlockPos {
                    x: 6,
                    y: SURFACE_Y + 2,
                    z: 6,
                },
                log,
            ),
        ];
        for x in 5..=7 {
            for z in 5..=7 {
                writes.push((
                    BlockPos {
                        x,
                        y: SURFACE_Y + 3,
                        z,
                    },
                    leaves,
                ));
            }
        }
        self.set_fixture_blocks(&writes);
    }

    /// Seeds the ore and the narrow reachable stair the mining contracts use.
    fn seed_ore(&self) {
        let ore = state_of(&self.blocks, "iron_ore");
        let air = state_of(&self.blocks, "air");
        self.set_fixture_blocks(&[
            (
                BlockPos {
                    x: 4,
                    y: SURFACE_Y - 2,
                    z: 4,
                },
                ore,
            ),
            (
                BlockPos {
                    x: 4,
                    y: SURFACE_Y,
                    z: 7,
                },
                air,
            ),
            (
                BlockPos {
                    x: 4,
                    y: SURFACE_Y - 1,
                    z: 6,
                },
                air,
            ),
            (
                BlockPos {
                    x: 4,
                    y: SURFACE_Y,
                    z: 6,
                },
                air,
            ),
            (
                BlockPos {
                    x: 4,
                    y: SURFACE_Y - 2,
                    z: 5,
                },
                air,
            ),
            (
                BlockPos {
                    x: 4,
                    y: SURFACE_Y - 1,
                    z: 5,
                },
                air,
            ),
            (
                BlockPos {
                    x: 5,
                    y: SURFACE_Y - 2,
                    z: 5,
                },
                air,
            ),
            (
                BlockPos {
                    x: 5,
                    y: SURFACE_Y - 1,
                    z: 5,
                },
                air,
            ),
        ]);
    }

    /// Seeds the single water column the fishing contracts use.
    fn seed_water(&self) {
        let water = state_of(&self.blocks, "water");
        self.set_fixture_blocks(&[(
            BlockPos {
                x: 8,
                y: SURFACE_Y,
                z: 8,
            },
            water,
        )]);
    }

    /// Seeds the loaded workstation the crafting contracts use.
    fn seed_crafting_table(&self) {
        let table = state_of(&self.blocks, "crafting_table");
        self.set_fixture_blocks(&[(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            table,
        )]);
    }

    /// Applies a small set of block writes to the fixture chunk.
    fn set_fixture_blocks(&self, writes: &[(BlockPos, BlockStateId)]) {
        let mut storage = self.world.try_lock().expect("test world is free");
        for (pos, state) in writes {
            storage
                .set_block_at(*pos, *state)
                .expect("fixture seed stays in the loaded chunk");
        }
    }

    fn with_options(
        wall: bool,
        replacement: Option<&str>,
        only_placements: bool,
        protected: Option<Arc<AtomicBool>>,
        query_counts: Option<(Arc<AtomicUsize>, Arc<AtomicUsize>)>,
    ) -> Self {
        let blocks = Arc::new(
            mc_world::BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap(),
        );
        let replacement = replacement.map(|path| state_of(&blocks, path));
        let world_root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            world_root
                .path()
                .join("dimensions/minecraft/overworld/region"),
        )
        .unwrap();
        let world: WorldHandle = Arc::new(tokio::sync::Mutex::new(
            WorldStorage::open(world_root.path(), Arc::clone(&blocks)).unwrap(),
        ));
        {
            let mut storage = world.try_lock().expect("test world is free");
            let air = state_of(&blocks, "air");
            let biome = Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, biome);
            let stone = state_of(&blocks, "stone");
            let grass = state_of(&blocks, "grass_block");
            for x in 0..16_u8 {
                for z in 0..16_u8 {
                    for y in SURFACE_Y - 5..SURFACE_Y {
                        chunk.set_block(x, y, z, stone);
                    }
                    chunk.set_block(x, SURFACE_Y, z, grass);
                }
            }
            if wall {
                // A closed passage: solid blocks where the squad must walk.
                let wall_block = state_of(&blocks, "stone");
                for z in 0..16_u8 {
                    chunk.set_block(10, SURFACE_Y + 1, z, wall_block);
                    chunk.set_block(10, SURFACE_Y + 2, z, wall_block);
                }
            }
            storage
                .insert_generated_chunk(ChunkPos { x: 0, z: 0 }, chunk)
                .unwrap();
        }
        let (read, mutation) = {
            let storage = world.try_lock().expect("test world is free");
            (storage.read_view(), storage.mutation_view())
        };
        let sessions = Arc::new(SessionRegistry::new());
        let items = Arc::new(solaris_required_items());
        let (journal, pending) = crate::play::world_journal::WorldChunkJournal::open_for_test(
            world_root.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
        )
        .unwrap();
        assert!(pending.is_empty());
        let first_decision = journal.reserve_decision_ids(1).unwrap()[0];
        journal
            .record_reserved_snapshot_groups(1, vec![(first_decision, Vec::new())])
            .unwrap();
        sessions.install_world_chunk_journal(journal);
        let (simulation, mut owner) = crate::play::simulation_channel();
        let _driver = {
            let sessions = Arc::clone(&sessions);
            let world = Arc::clone(&world);
            let simulation_read = read.clone();
            let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
            tokio::spawn(async move {
                while owner.wait_for_command().await {
                    owner
                        .process_commands_with_world_views(
                            &sessions,
                            Some(&world),
                            crate::play::SimulationWorldAccess {
                                read: Some(&simulation_read),
                                mutation: Some(&mutation),
                                cpu: Some(&resources),
                                light: None,
                            },
                            None,
                            1,
                        )
                        .await;
                }
            })
        };
        let item_facts = Arc::new(solaris_required_item_facts());
        let adapter = Arc::new(LiveResidentWorld::new(
            read,
            Arc::clone(&blocks),
            None,
            simulation,
            Arc::clone(&items),
            Arc::clone(&item_facts),
        ));
        let resident_world: Arc<dyn ResidentWorld> = match (replacement, protected) {
            (Some(replacement), _) => Arc::new(ReplacingResidentWorld {
                inner: Arc::clone(&adapter),
                world: Arc::clone(&world),
                replacement,
                only_placements,
            }),
            (None, Some(protected)) => {
                let (standable_queries, route_queries) = query_counts.unwrap_or_else(|| {
                    (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)))
                });
                Arc::new(ProtectionGatedResidentWorld {
                    inner: adapter,
                    protected,
                    standable_queries,
                    route_queries,
                })
            }
            (None, None) => adapter,
        };
        let runtime = InventoryRuntime::new(
            Some(world_root.path()),
            &crate::server::ShutdownHandle::default(),
            Arc::clone(&sessions),
            items,
            item_facts,
        )
        .with_resident_world(resident_world);
        Self {
            storage_root: tempfile::tempdir().unwrap(),
            world_root,
            blocks,
            world,
            sessions,
            runtime,
        }
    }
    fn enable_arrows(&self) {
        self.sessions.configure_arrow_kill_rewards(
            None,
            None,
            Some(77),
            Arc::new(solaris_required_items()),
            Arc::new(solaris_required_item_facts()),
            Arc::new(mc_data::loot::LootTables::default()),
        );
    }

    fn advance_resident_arrows(&self, tick: u64, center: Vec3) -> usize {
        let arrows = self.sessions.resident_arrows_for_test(center);
        let steps = arrows
            .iter()
            .map(|arrow| EntityPhysicsStep {
                id: arrow.id,
                position: Vec3::new(
                    arrow.position.x + arrow.velocity.x,
                    arrow.position.y + arrow.velocity.y,
                    arrow.position.z + arrow.velocity.z,
                ),
                velocity: arrow.velocity,
                on_ground: false,
                horizontal_collision: false,
            })
            .collect::<Vec<_>>();
        let facts = arrows
            .iter()
            .map(|arrow| ArrowPhysicsFact {
                arrow_id: arrow.id,
                block_hit: None,
                embedded_in_block: false,
                current_block_state: mc_world::BlockStateId(0),
                should_fall: false,
                fall_velocity_scale: Vec3::new(0.1, 0.1, 0.1),
                in_water: false,
                in_water_or_rain: false,
            })
            .collect::<Vec<_>>();
        self.sessions
            .apply_entity_physics_with_arrow_facts_and_dispatch(tick, &steps, &facts);
        arrows.len()
    }

    fn storage(&self) -> PluginStorage {
        PluginStorage::open(self.storage_root.path()).unwrap()
    }

    /// Materialise one resident through C3's real identity path and return its
    /// durable handle plus entity identity.
    async fn resident(
        &self,
        storage: &mut PluginStorage,
        slot: u32,
        position: Vec3,
    ) -> (String, Uuid) {
        let generation =
            resident_generation_id(WORLD_IDENTITY, "site-7", slot).expect("generation id");
        let snapshot = self
            .runtime
            .materialize_resident(storage, OWNER, &generation, position)
            .await
            .expect("resident materialises");
        let handle = mc_script::resident_handle_for_generation(OWNER, &generation).expect("handle");
        assert_eq!(handle, snapshot.handle);
        (handle, Uuid::parse_str(&snapshot.entity_uuid).unwrap())
    }

    /// Materialise a resident at the fixture's only physical mine entrance.
    async fn mining_resident(&self, storage: &mut PluginStorage, slot: u32) -> (String, Uuid) {
        self.resident(storage, slot, Vec3::new(4.5, 64.0, 8.5))
            .await
    }

    async fn execute(
        &self,
        storage: &mut PluginStorage,
        request: &ScriptOperationRequest,
    ) -> ScriptOperationOutcome {
        self.runtime
            .execute_resident_order_operation(storage, OWNER, request)
            .await
            .expect("resident order reaches the durable boundary")
    }

    async fn block(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        let mut storage = self.world.lock().await;
        storage.get_block(BlockPos { x, y, z }).unwrap()
    }

    /// Seed enough real farmland cells to force a receipt-bearing replant
    /// operation to stop at the simulation's conditional-edit boundary.
    async fn prepare_dense_replant_field(&self) {
        let farmland = state_of(&self.blocks, "farmland");
        let air = state_of(&self.blocks, "air");
        let mut storage = self.world.lock().await;
        for y in [40, 42, 44] {
            for x in 0..16 {
                for z in 0..16 {
                    storage
                        .set_block_at(BlockPos { x, y, z }, farmland)
                        .expect("dense farmland stays in the loaded fixture chunk");
                    storage
                        .set_block_at(BlockPos { x, y: y + 1, z }, air)
                        .expect("dense crop target stays in the loaded fixture chunk");
                }
            }
        }
    }
    async fn goal(&self, uuid: Uuid) -> mc_entity::GoalState {
        self.sessions
            .resident_entity_snapshots(&[uuid])
            .await
            .into_iter()
            .next()
            .flatten()
            .expect("resident stays live")
            .goal
    }

    async fn position(&self, uuid: Uuid) -> Vec3 {
        self.sessions
            .resident_entity_snapshots(&[uuid])
            .await
            .into_iter()
            .next()
            .flatten()
            .expect("resident stays live")
            .position
    }

    async fn health(&self, uuid: Uuid) -> f32 {
        self.sessions
            .resident_entity_snapshots(&[uuid])
            .await
            .into_iter()
            .next()
            .flatten()
            .expect("target stays live")
            .health
    }
}

fn state_of(blocks: &mc_world::BlockRegistry, path: &str) -> BlockStateId {
    let name = codec::Identifier::parse(format!("minecraft:{path}")).unwrap();
    blocks.block(&name).expect("required block").default
}

fn state_with_props(
    blocks: &mc_world::BlockRegistry,
    path: &str,
    props: &[(&str, &str)],
) -> BlockStateId {
    let name = codec::Identifier::parse(format!("minecraft:{path}")).unwrap();
    let props = props
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(&name, &props)
        .unwrap_or_else(|| state_of(blocks, path))
}

/// Seed canonical gear through the durable order ledger, the same store the
/// executor reads and writes.
fn seed_gear(
    storage: &mut PluginStorage,
    handle: &str,
    entity_uuid: Uuid,
    carry: &[(&str, u32)],
) -> u64 {
    seed_gear_in(storage, handle, entity_uuid, carry, false)
}

fn seed_gear_in(
    storage: &mut PluginStorage,
    handle: &str,
    entity_uuid: Uuid,
    carry: &[(&str, u32)],
    equipment: bool,
) -> u64 {
    // Gear is one field of the resident's durable order record, so seeding it
    // merges into the record the resident already has: acquiring a bow must not
    // silently drop the standing order it is serving.
    let mut record = storage
        .resident_orders()
        .record(handle)
        .cloned()
        .unwrap_or_else(|| {
            crate::script::storage::resident_orders::DurableResidentOrderRecord::empty(
                handle.to_owned(),
                OWNER.to_owned(),
                entity_uuid.to_string(),
                crate::script::storage::resident_orders::DurableAssignment::Civilian,
            )
        });
    for (index, (item, count)) in carry.iter().enumerate() {
        let stack = Some(DurableResidentStack::new((*item).to_owned(), *count));
        if equipment {
            record.equipment[index] = stack;
        } else {
            record.carry[index] = stack;
        }
    }
    storage
        .append_resident_order_change(DurableResidentOrderChange::Record {
            record: Box::new(record),
        })
        .expect("seeded gear is durable")
}

fn gear(storage: &PluginStorage, handle: &str) -> Vec<(String, u32)> {
    storage
        .resident_orders()
        .record(handle)
        .map(|record| {
            record
                .equipment
                .iter()
                .chain(&record.carry)
                .flatten()
                .map(|stack| (stack.item_id.clone(), stack.count))
                .collect()
        })
        .unwrap_or_default()
}

fn work_request(
    operation_id: &str,
    handle: &str,
    work: ScriptResidentWorkOrder,
    work_units: u64,
    expected_revision: u64,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::AssignWork {
                operation_id: operation_id.to_owned(),
                handle: handle.to_owned(),
                work,
                work_units,
                expected_revision,
            },
        },
    )
    .expect("valid work request")
}

fn order_request(
    operation_id: &str,
    handles: &[String],
    revisions: &[u64],
    order: ScriptResidentOrder,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::IssueOrder {
                operation_id: operation_id.to_owned(),
                handles: handles.to_vec(),
                expected_order_revisions: revisions.to_vec(),
                order,
            },
        },
    )
    .expect("valid order request")
}

fn demobilize_request(
    operation_id: &str,
    handle: &str,
    expected_revision: u64,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::Demobilize {
                operation_id: operation_id.to_owned(),
                handle: handle.to_owned(),
                expected_revision,
            },
        },
    )
    .expect("valid demobilise request")
}

fn area(min: [i32; 3], max: [i32; 3]) -> ScriptWorkArea {
    ScriptWorkArea::new(
        "minecraft:overworld".to_owned(),
        ScriptBlockPosition::new(min[0], min[1], min[2]),
        ScriptBlockPosition::new(max[0], max[1], max[2]),
    )
}

fn crafting_station() -> ScriptWorkArea {
    area([9, SURFACE_Y + 1, 9], [9, SURFACE_Y + 1, 9])
}

fn work_of(outcome: &ScriptOperationOutcome) -> mc_script::ScriptWorkAssignment {
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!(
            "expected a resident order payload, got {:?}",
            outcome.payload()
        );
    };
    match &**result {
        ScriptResidentOrderResult::Work { assignment } => (**assignment).clone(),
        other => panic!("expected a work result, got {other:?}"),
    }
}

fn order_of(
    outcome: &ScriptOperationOutcome,
) -> (
    u64,
    Vec<mc_script::ScriptOrderMemberOutcome>,
    Vec<mc_script::ScriptCombatEvent>,
) {
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!(
            "expected a resident order payload, got {:?}",
            outcome.payload()
        );
    };
    match &**result {
        ScriptResidentOrderResult::Order {
            order_revision,
            members,
            combat,
        } => (*order_revision, members.clone(), combat.clone()),
        other => panic!("expected an order result, got {other:?}"),
    }
}

fn formation() -> ScriptFormation {
    ScriptFormation::new(ScriptFormationKind::Line, 4)
}

fn carry_request(handle: &str, expected_revision: Option<u64>) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Query {
                endpoint: ScriptInventoryEndpoint::ResidentCarry {
                    handle: handle.to_owned(),
                },
                expected_revision,
            },
        },
    )
    .expect("a carry query is a valid request")
}

/// Every non-empty stack one carry snapshot reports.
fn carried_from(outcome: &ScriptOperationOutcome) -> Vec<(String, u32)> {
    let ScriptOperationPayload::OwnedInventory { result } = outcome.payload() else {
        panic!("expected an inventory payload, got {:?}", outcome.payload());
    };
    let mc_script::ScriptOwnedInventoryResult::Snapshot { inventory } = &**result else {
        panic!("expected an inventory snapshot, got {result:?}");
    };
    inventory
        .slots
        .iter()
        .filter_map(|slot| {
            slot.item
                .as_ref()
                .map(|item| (item.resource_id.clone(), item.count))
        })
        .collect()
}

/// The stacks in the worker's own carry slots, equipment excluded.
fn carry(storage: &PluginStorage, handle: &str) -> Vec<(String, u32)> {
    storage
        .resident_orders()
        .record(handle)
        .map(|record| {
            record
                .carry
                .iter()
                .flatten()
                .map(|stack| (stack.item_id.clone(), stack.count))
                .collect()
        })
        .unwrap_or_default()
}

fn members_revision(outcome: &ScriptOperationOutcome) -> u64 {
    match outcome.payload() {
        ScriptOperationPayload::ResidentOrder { result } => match &**result {
            ScriptResidentOrderResult::Order { order_revision, .. } => *order_revision,
            other => panic!("expected an order result, got {other:?}"),
        },
        other => panic!("expected a resident order payload, got {other:?}"),
    }
}
