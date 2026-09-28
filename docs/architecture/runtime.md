# Actual runtime map

**Status.** Actual runtime as of 2026-09-26 (CP-REFACTOR-010), verified against the tree at
`9889036e`. This file describes **what exists now**, including unfinished migrations. The
normative *target* is [`../ARCHITECTURE.md`](../ARCHITECTURE.md); live state and the single
`next` are in [`../MEMORY.md`](../MEMORY.md). Symbols are named as `file:line` at the verified
revision; re-check them before editing.

## Entry points

```text
mc-server (composition root)
  server.rs:bind_internal:2666
    binds the listener, opens the world/journal, builds the runtime, then serves
    - SessionRegistry (play/session.rs)                    connections + players + entity owner lanes
    - SimulationOwner channel (play/simulation.rs:3663)    spatial/domain command owner
    - PluginStorageHandle (script/storage.rs:1876)         tokio task for plugin storage
    - InventoryRuntime (script/storage/world_inventory.rs:109)  resident/settlement/inventory operations
    - SettlementRuntime + LiveSettlementWorld (settlement.rs, server.rs:2786-2847)

Minecraft client
  <-> mc-protocol codecs + mc-net connection/session adapters (play.rs, play/session.rs)
        typed, authenticated requests
            |
            +-> SimulationOwner::process_commands_with_world_views (simulation.rs:4563;
            |   production tick consumer in server/entity_ticker.rs)
            +-> SessionRegistry player/container owners
            +-> PluginStorageHandle::enqueue -> run_storage_actor (script/storage.rs:1976)
                     |
                     +-> PluginStorage (journal-backed)   records, ledgers, receipts
                     +-> InventoryRuntime                 inventory/resident/settlement operations
                            |
                            +-> world storage (mc-world) / regional entity owner (mc-entity::regional)
```

WASM side: `mc-plugin-host` owns the component stores and bounded callbacks; `mc-script`
owns the runtime-independent DTO/admission contract (`ScriptBoundary`, `ScriptOperation*`
DTOs and their validation); the admitted-command dispatch (`script/router.rs`) lives in
`mc-net`, which routes each command to its native owner. The deployed packages are Wasmtime
components built against `solaris:plugin@0.7.0` (`crates/mc-script/wit`). First-party guest
policy lives in the sibling `../solaris-default-plugins` and is not part of a production core
build.

## Queues, tasks and ownership boundaries (actual)

| Mechanism | Where | Owns | Notes |
| --- | --- | --- | --- |
| Tokio task + `mpsc` (`STORAGE_QUEUE_CAPACITY`) | `PluginStorageHandle::start` (`script/storage.rs:1876`) | plugin storage, ledger mutation, resident order admission | async task, not an OS thread; a stop guard marks failure/stop and the handle can `wait_stopped()` |
| Simulation channel + `SimulationHandle` / `SimulationOwner` | `SimulationHandle` (`play/simulation.rs:2295`, the connection-side handle) and `SimulationOwner` (`:3663`, the executing owner); channel built by `simulation_channel_with_explosion_seed` | spatial/domain command execution, container commits, player pose/inventory commits | one executing owner object; handles submit and await responses |
| Regional entity owner lanes | `SessionRegistry::try_new_with_entity_owner_lanes` (`play/session.rs:1440`), `try_new_with_entity_owner_journal` (`:1446`), `mc-entity::regional` (`RegionalOwnerLane`, `RegionalCommitDecision`, `RegionalDecisionJournal`) | entity state per region key/epoch | lane count derives from detected CPU capacity; region leases, epochs and phase watermarks are native-only concepts |
| Connection tasks | `play.rs` | socket I/O, packet decode, session state, deferred delivery | a connection never mutates ECS directly; it submits commands and publishes results |
| Harness process scope | `tools/harness/__main__.py`, `tools/harness/runtime.py` | one bounded systemd user scope per run when available (otherwise a warning and an unbounded run), plus owned server/client/Xvfb processes, readiness waits and `result.json` | the only canonical receipt writer; backends never write the top-level `result.json`; process ownership and the receipt stay mandatory in both cases |

## Verified mutation authority (what the code actually enforces)

- Player inventory has **two kinds of actual writer**; CP-REFACTOR-050 left the direct session
  paths in place (no mutable mirror was proven), so they remain legacy exceptions:
  1. *Owner path (migrated in part):* a connection builds a `ContainerPlayerPlan` and calls
     `state.simulation.commit_player_inventory(plan)` (`play.rs:4274`, `SimulationHandle`
     method), which reaches `SessionRegistry::commit_player_inventory`
     (`play/session/player_state.rs:116`) via `SimulationOwner::player_inventory_response`
     (`play/simulation.rs:6434`). Two distinct outcomes follow: a transport/runtime `Err`
     restores the connection's local **expected** candidate, while an explicit
     `PlayerInventoryCommitOutcome::Rejected` installs the **authoritative** inventory/cursor/
     window state the owner returned (`play.rs:4316-4338`).
  2. *Direct session paths (legacy exception):* `commit_session_owner_script_player_inventory`
     (`play.rs:3973`) applies an admitted script inventory transaction, and
     `commit_session_owner_loader_item_grant` (`play.rs:3993`) applies a Loader item grant,
     straight to `InteractionState.inventory` plus the persisted inventory under the
     `player_persistence` authoritative lock. They do not go through the simulation owner.
  Both kinds serialize on the same `player_persistence` mutex, and the owner path rejects a
  plan whose expected slots no longer match, so a direct write cannot be silently overwritten
  by a stale owner commit. The duplication is policy, not a lost-update race. The connection
  keeps the committed client baseline for stale-transaction checks and does not overwrite it
  speculatively.
- Container/chest contents are committed through the simulation owner:
  `SimulationHandle::commit_chest` (`play/simulation.rs:3432`) submits; the owner applies it
  in `SimulationOwner::commit_chest_command` (`play/simulation.rs:5564`) and returns
  `ServerOwnedChestCommit`/`ChestCommitOutcome`.
- Settlements: `SettlementLedger` (`script/storage/settlement.rs:319`) is the durable
  site/structure/survey/warehouse/building authority, replayed through `PluginStorage`.
  `bind_manual_warehouse` (`script/storage/settlement/warehouse.rs:440`) is the manual-bind writer.
- Residents: durable identity/cargo/orders live in `PluginStorage` ledgers
  (`script/storage/residents.rs`, `script/storage/resident_orders.rs`,
  `script/storage/resident_morale.rs`); live entity state lives in the regional owner; the
  guest holds opaque handles, never region keys or entity handles.
- Harness receipts: `tools/harness/__main__.py` writes
  `.analysis/validation/<stamp>-<profile>-<id>/result.json` (status set at
  `tools/harness/__main__.py:363`, written at `:374`).

## Legacy exceptions (actual, not target)

- The container/menu surface still has per-window handlers in `play.rs`
  (for example `handle_crafting_container_click`, `play.rs:2685`) alongside the migrated
  window-0/cursor path; `../ARCHITECTURE.md` states the cutover is partial.
- `mc-net` remains the modular monolith (ADR 0006). There is **no** `mc-simulation` or
  `mc-inventory` crate; extraction is not a precondition of this program
  (`AGENT_READINESS_REFACTOR_PLAN.md` §5.4).
- The plugin storage actor is one tokio task that takes `PluginStorage` by value; after that
  move the storage is reachable only through the actor handle (`enqueue`, `wake_resident_work`,
  `wait_stopped`), never by another owner.

## How to use this file

Answer "where does this live?" here, then read the named symbol or the domain README next to
the code. For the state-by-state owner/projection/persistence index use
[`state-ownership.md`](state-ownership.md); for end-to-end routes use
[`mutations.md`](mutations.md).
