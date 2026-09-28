# State ownership index

**Status.** Actual ownership as of 2026-09-26 (CP-REFACTOR-010), verified at `9889036e`.
Rows marked **(?)** are pointer-level claims that still need the symbol-level confirmation of
the checkpoint that touches them; unmarked rows were checked against the named symbol. This
index says **who may write**, not only who reads: a caller edge is not mutation authority.

The normative target is [`../ARCHITECTURE.md`](../ARCHITECTURE.md); the actual module/queue map
is [`runtime.md`](runtime.md); end-to-end routes are [`mutations.md`](mutations.md).

| State | Semantic/native authority | Native writer path | Durable record | Readers / projection | Mutation and durability boundary |
| --- | --- | --- | --- | --- | --- |
| Player inventory, cursor, held item | `SessionRegistry` player record (`play/session.rs`) | two paths: `state.simulation.commit_player_inventory(ContainerPlayerPlan)` (`play.rs:4274` → `simulation.rs:6434` → `SessionRegistry::commit_player_inventory`) and `commit_session_owner_script_player_inventory` (`play.rs:3973`, direct under `player_persistence`) | persisted player inventory (`player_persistence` authoritative mutex) | connection committed baseline/menu, script observations, other sessions' visibility dispatches | prepare/commit under the `player_persistence` lock; a transport `Err` restores the connection's local expected candidate while an owner `Rejected` installs the authoritative state the owner returned; durability follows the specific operation contract, not the response delivery |
| Physical container/chest contents | world storage via the simulation owner | `SimulationHandle::commit_chest` submits (`simulation.rs:3432`); `SimulationOwner::commit_chest_command` applies (`simulation.rs:5564`) | world commit path (block entity/state) | player menu projection, warehouse/container reads | owner commit; **(?)** the exact journal frame and recovery assertions are CP-REFACTOR-050/060 evidence and are not asserted by this index |
| Warehouse binding/handle | core `SettlementLedger.warehouses` (`script/storage/settlement.rs:319`) | `bind_manual_warehouse` (`settlement/warehouse.rs:440`), `bind_village_warehouse` (`:346`), `bind_warehouse` (`:262`) | `DurableWarehouseBinding` in the plugin-storage journal + snapshot frames | guest holds an opaque handle/`ScriptWarehouseBinding`; warehouse queries resolve a position | native bind + one durable settlement change; a binding owns **no** container contents; catalog drift makes an authored binding unresolvable rather than blocking later binds |
| Resident identity, cargo, orders, morale | durable ledgers in `PluginStorage` (`script/storage/residents.rs`, `resident_orders.rs`, `resident_morale.rs`) for records; live entity in the regional owner | `InventoryRuntime` resident operations (`execute_resident_order_operation`, `materialize_resident`) and resident-order ledgers | resident/order/morale ledger frames in the plugin-storage journal | guest roster/policy, client entity projection | resident operation + its receipt; the live entity moves only through a regional command. Guest policy never writes the entity |
| Household/job intent, economic policy | first-party settlement component (sibling `../solaris-default-plugins`), persisted through the host storage boundary | guest command/CAS through `mc-plugin-host` → `mc-script` admission → storage actor | component-owned records inside plugin storage | component UI/policy callbacks; native owners only through admitted operations | component commands with CAS; never a direct world/entity/inventory write |
| Construction progress, building certificate | `SettlementLedger.buildings` / `DurableBuildingCertificate` (`script/storage/settlement/certificate.rs`) | `recognize_building` (`settlement/certificate.rs`); the staged construction lifecycle (`settlement/construction.rs`); the shared ledger, `commit_settlement` and the commit authority stay in `script/storage/settlement.rs` | certificate frames in the plugin-storage journal | guest/UI observations of the certificate | prepare/commit/receipt/recovery of the native path; the guest chooses the plan, not the material consumption |
| Entity motion, health, lifecycle | regional ECS owner (`mc-entity::regional`: `RegionKey`, `RegionLease`, `RegionPhase`, `RegionalCommitDecision`, `RegionalDecisionJournal`) | `RegionalOwnerLane` mutation batches / `RegionOwnerMutation` | regional decision journal (`RegionalDecisionJournal`) once the owner enables it | published snapshots and entity tracking; client projection | one spatial owner writes at a time; commit then publication; a lost publication never undoes a commit |
| Block state, block entity | `mc-world` conditional commit; cross-domain ordering by the native coordinator | world mutation views through the owner path | world chunk journal (`play/world_journal.rs`) | immutable world snapshots | state/token fence; durability is not identical to dirty publication |
| Session/socket lifecycle | connection/session adapter (`play.rs`, `play/session.rs`) | connect/disconnect/transport tasks | not persisted (session-scoped) | routing indexes, visibility dispatch | a connection may submit commands; it must not write ECS, world or block state directly. Inventory is the current exception: the legacy script path writes session-owned inventory under `player_persistence` (see CP-REFACTOR-050), which is why this index records two actual inventory writers |
| Test run, owned processes, receipt | canonical harness (`tools/harness/__main__.py`, `runtime.py`) | one profile run per artifact directory | `result.json` plus per-command logs under `.analysis/validation/<stamp>-<profile>-<id>/` | agents/CI read the artifacts; backends never write the top-level receipt | one lifecycle owner per run; readiness is event-driven (pidfd/inotify/state events), and a timeout is a failure |

## Rules that follow from the table

1. One state has one allowed authority for a given operation. Two paths may both be *actual*
   today (player inventory, see CP-REFACTOR-050) — that is a cutover in progress, recorded as
   such, never described as a clean single owner.
2. A transaction is a mutation boundary, not a data owner: the warehouse binding belongs to the
   ledger, its contents to the container, its guest handle to the guest.
3. Projection/publication may be lost without undoing the commit; readers must not treat an
   absent projection as an absent change.
4. A refusal before the durable decision changes nothing; `Option` lookups that exist today
   (for example `SettlementLedger::binding_position`, `warehouse_for_position`) keep their
   current semantics.
5. Ambiguity is a local discovery prerequisite, not a licence to guess: name the symbol, then
   confirm it with LSP/CodeGraph before editing.
