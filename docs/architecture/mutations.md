# Mutation cards

**Status.** Actual routes as of 2026-09-26 (CP-REFACTOR-010), verified at `9889036e`. Each card
states entry, admission, owner validation, commit point, projection, restart/replay and the
tests that actually prove it. Two operations with different durability deliberately look
different here — do not copy one card as a template for another.

The state index is [`state-ownership.md`](state-ownership.md); module/queue facts are in
[`runtime.md`](runtime.md).

## Card 1 — manual warehouse bind (plugin → WIT → storage → ledger)

| Step | Actual |
| --- | --- |
| Entry / provenance | guest issues the WIT bind operation; `mc-script` validates and admits it (`ScriptOperationRequest` with a survey token and a position), then `script/router.rs` routes it to the storage actor |
| DTO / admission | `ScriptOperationRequest` bounds are checked in `mc-script`; the token is an opaque string whose owning plugin is recorded in `DurableSurveyToken.plugin_id` |
| Observations | the storage actor reads the live world through `SettlementWorld` (`container_reading`, `claims_overlap`, `footprint_changed_since`) without mutating it |
| Owner validation | `bind_manual_warehouse` (`script/storage/settlement/warehouse.rs:440`): no world → `RuntimeUnavailable`; unknown token → `NotFound`; token owned by another plugin → `Forbidden`; non-overworld token → `RuntimeUnavailable`; position outside token bounds → `InvalidRequest`; claimed footprint → `Forbidden`; container unloaded → `Unloaded`; container missing → `NotFound`; expired token or changed footprint → `StaleRevision`. An existing binding for the position is returned to its owner and refused (`Forbidden`) to anyone else |
| Commit point | one `DurableSettlementChange::Warehouse` inside `commit_settlement` → `PluginStorage` journal append (`script/storage.rs:590 commit_batch` → `:1519 append_frame`); the ledger `apply` (`settlement.rs`, `Warehouse` arm) rejects a changed rebind of the same container or a revision regression |
| Projection | the guest receives `ScriptOperationOutcome`/`ScriptWarehouseBinding`; nothing about the container contents changes |
| Restart / replay | `PluginStorage::open` (`script/storage.rs:299`) replays journal frames into `SettlementLedger`; `Option` lookups (`binding_position` `settlement/warehouse.rs:82`, `warehouse_for_position` `settlement/warehouse.rs:101`) keep their current semantics across reopen |
| Tests (native) | `script::storage::settlement_tests::warehouse_tests::manual_warehouse_binds_surveyed_container_and_reuses_handle`, `…::manual_warehouse_refuses_stale_token_outside_claimed_missing_and_cross_kind`, `…::manual_bind_refuses_position_held_by_authored_binding`; run `python3 -m tools.harness run scenario warehouse-bind-manual` and `run scenario warehouse-bind-refusals` (3 tests, ~0.4 s once built) |
| Not proven here | graphical/manual acceptance of manual building (CP-049 debt); the component round-trip belongs to the sibling `settlement-cargo` gate |

## Card 2 — warehouse deposit / owner transfer (two endpoints, composite receipt)

| Step | Actual |
| --- | --- |
| Entry / provenance | `LiveSettlementWorld::commit_warehouse_transfer` (`crates/mc-net/src/settlement.rs:768`) forwards an admitted request to `SimulationHandle::commit_warehouse_transfer` (`play/simulation.rs:3475`), which the executing `SimulationOwner` applies |
| Owner validation | the simulation owner re-reads the live fences attached to the request (state id, player, warehouse/owner identity) before mutating anything |
| Commit point | the owner appends its durable record and then publishes the transfer; `world_inventory.rs:1549 commit_warehouse_transfer` is the storage-side counterpart that carries the warehouse/player participants |
| Projection | the committed container/inventory projections and the receipt are published after the durable append, so a lost publication does not undo the transfer |
| Restart / replay | recovery reads the durable append; the owner refuses a stale fence instead of replaying blindly |
| Tests (native, owner path) | `play::simulation::tests::server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both` (`play/simulation/tests.rs:11527`) and `play::simulation::tests::server_owned_warehouse_deposit_refuses_stale_fences_without_mutating` (`:11793`); run `python3 -m tools.harness run test -- -p mc-net --lib server_owned_warehouse_deposit` |
| Not proven here | graphical conservation of the client-visible passenger item; that is the `inventory` real-client gate, deferred by the owner |

## Card 3 — owned / compound inventory transaction across a real process crash

| Step | Actual |
| --- | --- |
| Entry / provenance | an admitted owned-inventory (or script compound) operation reaches `InventoryRuntime` (`script/storage/world_inventory.rs`; `OwnedInventoryPrepare`/`OwnedInventoryCommit` in `play/owned_inventory.rs`) |
| Owner validation | expected revisions and fences are re-read (`ScriptInventoryFence`, per-endpoint expected revisions) |
| Commit point | the **world journal** decision comes first: `WorldInventoryCommit::commit_prepared` records the reserved inventory decision durably (`record_reserved_inventory_decision`, `world_inventory.rs:629`) and marks it decided (`:636/647`) before `project` (`world_inventory.rs:659`) appends the plugin-ledger half through the `append_inventory_projection` helper (`:690`, called from the commit paths such as `:902`) and installs the player file. The plugin-storage append is the projection of an already-durable decision, not the decision itself; `PluginStorage::commit_batch` refuses a batch that carries player `inventory` |
| Restart / replay | `InventoryRuntime::recover` (`world_inventory.rs:212`) replays world-journal inventory decisions into `PluginStorage` and then the player projection, exactly once |
| Tests (native, real child-process crash) | `play::session::owned_inventory_endpoint_tests::owned_transfer_recovers_exactly_once_across_a_process_crash` (`play/session/owned_inventory_endpoint_tests.rs:704`) and `play::script_inventory_transaction_tests::compound_transaction_recovers_inventory_after_process_crash` (`play/script_inventory_transaction_tests.rs:151`); run `python3 -m tools.harness run test -- -p mc-net --lib process_crash` |
| Not proven here | client-visible conservation after a crash; deferred with the `inventory` gate |

Cards 2 and 3 share the journal/actor boundary but differ in endpoints, receipt shape and
recovery source; neither proves the other, and a single generic "transaction" description
would be wrong.

## Card 4 — window click / crafting (connection-owned preparation, owner commit)

| Step | Actual |
| --- | --- |
| Entry | `ServerboundContainerClick` decoded in `play.rs`; per-window handlers such as `handle_crafting_container_click` (`play.rs:2685`) |
| Preparation | the connection computes the candidate inventory/cursor/window state and a `ContainerPlayerPlan` (`play.rs:4264`) |
| Commit/refusal | `state.simulation.commit_player_inventory(plan)` (`play.rs:4274`) → `SimulationOwner::player_inventory_response` (`simulation.rs:6434`) → `SessionRegistry::commit_player_inventory` (`play/session/player_state.rs:116`). Two branches: a transport/runtime `Err` restores the connection's local expected inventory/cursor/window candidate and refreshes the crafting result; an explicit `Rejected` installs the authoritative inventory/cursor/window state the owner returned (`play.rs:4316-4338`) |
| Legacy exception | the script-admitted player-inventory transaction path writes `InteractionState.inventory` directly (`play.rs:3973`), bypassing the simulation owner — this mixed authority is the CP-REFACTOR-050 cutover, not a target |
| Projection | the committed inventory content is written back to the client (`write_inventory_content`) |
| Evidence | focused native tests for the crafting/quick-craft path; the client-visible assertion is `m94-03a-inventory-oak-log-to-planks` (`../solaris-loader/java-agent/.../M94InventoryCraftingScenario.java:13`, assertions at lines 112-148) driven by `python3 -m tools.harness run inventory` — **not run** here (graphical, owner-deferred) |

## Card 5 — one harness run (process and receipt lifecycle)

| Step | Actual |
| --- | --- |
| Entry | `python3 -m tools.harness run PROFILE` (`tools/harness/__main__.py`) |
| Admission | profile registry `tools/harness/profiles.py`; unknown profile → fail-closed `failed` receipt listing known profiles |
| Owner validation | prerequisites per profile (siblings, credentials, Java 25, Xvfb, ports); missing ones fail the run instead of degrading |
| Commit point | the profile's own commands; the run owns one systemd scope and one artifact directory |
| Projection / receipt | `.analysis/validation/<stamp>-<profile>-<id>/result.json` with `status` `passed`/`failed`/`prepared`/`interrupted`, plus per-command logs (`__main__.py:363,374`) |
| Restart / cleanup | owned server/client/Xvfb processes are stopped by the shared runtime; an interruption keeps recorded evidence and exits 130 |
| Evidence | `python3 -m tools.harness run harness-check`; this checkpoint ran only focused `test` profiles (receipts in `.analysis/codex-logs/cprefactor-000/receipt.json`) |

## What a new card must state

1. entry and provenance (packet / admitted plugin operation / native scheduled action);
2. DTO and admission, including permissions and generation/session identity;
3. what may be read without touching authority;
4. which revision/token/stock fences the owner re-reads;
5. the commit point and where the outcome can be unknown;
6. what may fail to reach the caller without undoing the commit;
7. who restores state after restart and from which record;
8. the exact test id or selector plus what it does **not** cover.
