# ADR 0011 - Core crate topology and threading model

**Date:** 2026-09-28

**Status:** The v1 crate topology, extraction queue, threading and multi-stream
durability model are superseded by [ADR 0013](0013-core-model-v2.md) and
[ADR 0014](0014-v1-core-retirement.md), Phase 5 (2026-10-15).
The body records the retired implementation, not an active migration plan.
General constraints (one gameplay authority, protocol ownership, no production
dependency on the test harness) remain reflected in the current model; the
specific v1 crates, owners and durable streams no longer exist.
**Evidence:** historical
[`core-architecture-audit-2026-09-28.md`](https://github.com/kaiserproger/solaris/blob/1ada2ceb93688adf35d0869111743d4d231e8b62/docs/evidence/core-architecture-audit-2026-09-28.md).

## Decision

The five responsibilities of [`ARCHITECTURE.md`](../ARCHITECTURE.md) become
Cargo crates, and the dependency direction between them is enforced by Cargo,
not by source scanners. One crate is added: `mc-simulation`. No other crate is
added by this decision.

```text
mc-server        composition root, CLI, startup data, content import, console, dashboard
   |
   +-> mc-net    connection adapter: framing, encryption, login/configuration/status,
   |             Play packet decode -> typed requests, session index, chunk stream,
   |             outbound queues, semantic effect -> packet publication, Loader wire
   |      |
   |      +-> mc-simulation   the game: SimulationOwner + commands, player state and
   |      |                   inventory, containers, block edit commit, survival/combat,
   |      |                   tick orchestration, world/entity journals and persistence,
   |      |                   sleep/time/weather/rules, plugin storage actor,
   |      |                   settlement/resident ledgers and runtime, script router
   |      |        |
   |      |        +-> mc-entity (regional ECS owner + AI kernels), mc-world, mc-physics,
   |      |            mc-worldgen, mc-data, mc-nbt, mc-domain, mc-script
   |      +-> mc-protocol
   +-> mc-plugin-host (Wasmtime), mc-script
mc-test-harness  dev/tooling only; never a production dependency
```

Rules, each of which is a Cargo edge or its absence:

1. `mc-simulation` does not depend on `mc-net` or `mc-protocol`. It has no
   socket, packet, or session-connection type. It publishes owned semantic
   effects (block deltas, light updates, entity snapshots and moves, inventory
   content, damage publication, time/weather), and `mc-net` translates them to
   packets. Today's `OutboundCommand` is not that type yet: it carries packet
   DTOs (`ClientboundBlockEvent`, `PlayerInfoUpdate`, `ClientboundExplode`);
   its ingress commands (script teleport, script inventory, Loader grant) became
   owner commands in CP-ARCH-030. The migration splits it rather than moving it
   (historical plan §3 of the archived `CORE_ARCHITECTURE_PLAN.md` at commit
   `1ada2ce`).
2. `mc-net` depends on `mc-simulation` and `mc-protocol`. It owns no gameplay
   rule, no canonical player/entity/world/container state, and no journal. Its
   `SessionRegistry` is a session index: connection membership, visibility and
   recipient indexes, chunk stream, outbound queues. Canonical player records,
   combat/sleep state, world time/weather/rules move to a `mc-simulation`-owned
   player registry; admin and packet paths reach them only through
   `SimulationOwner` commands.
3. `mc-server` is the only composition root. It opens the world and journals,
   builds the simulation runtime and plugin host, then hands ready handles to
   `mc-net::bind`. `mc-net` constructs only what is intrinsic to networking.
4. `mc-server` has no `mc-test-harness` dependency. Content import from a
   vanilla jar is production code with a production owner; the harness may
   consume it, not the reverse.
5. `mc-entity` owns every spatial entity rule: kinematics, fall distance and
   fall damage, hostile/passive/villager/dragon planners. `mc-net` keeps only
   session sampling and publication adapters for entities.
6. `mc-protocol` is the only packet catalog. A packet declared elsewhere is a
   defect.
7. Player inventory has exactly one writer: `SimulationOwner` via
   `CommitPlayerInventory { provenance: Menu | Grant | Compound }`. A compound
   plugin-storage transaction keeps storage and inventory atomic without holding
   any lock across fsync: `ReservePlayerInventory` sets an owner reservation
   (token + inventory-revision generation) and returns an owned snapshot; the
   storage actor plans and appends on its blocking worker; then the owner applies
   the after-image (`Compound`), or ends the reservation with
   `ReleasePlayerInventoryReservation` (rejected / not committed) or
   `MarkInventoryRecoveryRequired` (durability unknown). While a reservation lives
   every other inventory writer of that player is refused with
   `InventoryReserved`. Disconnect clears the reservation; a late after-image is
   refused and the journal decision stays unprojected for restart recovery.
   Direct session writers are deleted, not wrapped.
8. `mc-domain` stays small. Duplicated value types (`EntityId`, `BlockStateId`,
   `BlockPosition` in projectile modules; `EntityItemStack` field copies) are
   replaced by the owning crate's type; `mc-domain` does not become a DTO dump.
9. `mc-script` is VM-independent, not async-runtime-independent. Its Tokio
   dependency is accepted and documented; it is not split.

## Threading model

```text
solaris-sim      one dedicated OS thread: SimulationOwner command batches, the 50 ms tick,
                 and every phase that waits synchronously on regional lanes
regional lanes   OS threads (ADR 0005), bounded 64, lease/epoch fenced
solaris-journal  one OS thread, bounded 64 (world-chunk + entity WAL); write-behind for
                 ordinary decisions (ADR 0005), fsync-before-reply for receipt-bearing
                 decisions, flush and checkpoint
mc-plugin-host   one OS thread for guest callbacks + epoch watchdog thread
tokio runtime    I/O only: listener, connection tasks, chunk prepare orchestration,
                 dirty flush, control plane, dashboard; CPU/disk work through
                 bounded spawn_blocking with semaphores
```

- The simulation tick and `SimulationOwner` never run on a Tokio worker. A
  Tokio task submits a command and awaits a oneshot reply; the simulation
  thread may block on regional lanes. If the simulation thread needs a blocking
  pool result it waits on a channel or `Handle::block_on` from its own thread,
  which is not a runtime worker.
- No session lock is held while waiting for a regional owner. Prepare from an
  immutable snapshot, wait, then reacquire and revalidate.
- Durable appends (`PluginStorage` journal, player `.dat`) run on the journal
  thread or a bounded blocking worker, never on a Tokio worker or a connection
  task. Player state load at Play entry also leaves the connection task.
- Every Tokio runtime thread (workers and the blocking pool) has a 16 MiB stack
  (`RUNTIME_THREAD_STACK_BYTES`, `crates/mc-server/src/main.rs`). A debug build
  gives one connection-task poll frame hundreds of kilobytes: a gdb backtrace of
  a chest open measured `handle_use_item_on_interactions` at 502,784 bytes and
  the whole open-chest chain at about 2.09 MiB, which overflowed the 2 MiB
  default. The stack is address space, not resident memory.
- Plugin storage (implemented in CP-ARCH-020, C3): the storage actor stays the
  single serialized owner of `PluginStorage`, held in `StorageSlot`. Every
  durable append (`append_frame` write + fsync, compaction) runs through
  `StorageSlot::durable`: the actor moves the owned storage into one
  `spawn_blocking` step and awaits it, accepting no further command until the
  storage is back. In-memory reads and preparation (status, scans, ledger
  lookups) stay inline on the actor. A `JoinError` is fatal: the actor marks
  itself failed. Startup inventory recovery uses the same `durable` step before
  the actor starts. The order commit → delivery → durable ack is unchanged, and
  no `Arc<Mutex<PluginStorage>>` exists.
- The durability contract itself does not change with the move: ordinary
  world/entity decisions are accepted into the bounded journal queue and the
  unflushed tail may be lost on crash (ADR 0005 "ordinary mutations");
  receipt-bearing decisions, `flush`, and `checkpoint_through` wait for fsync.
- Shutdown follows the producer chain
  `connections / guest callbacks → script command tasks → plugin-storage actor →
  SimulationOwner → journal writer`, and each stage is joined before the stage it
  submits to stops: stop accepting; drain connections; stop guest command
  admission and join the script command tasks; join the plugin-storage actor
  **while the simulation owner is live** (an in-flight storage→simulation
  operation such as `commit_warehouse_transfer`, which holds `save_coordinator`
  and awaits `SimulationHandle`, completes or is refused, never abandoned);
  save barrier and stop the simulation thread (simulation tail); final save
  (dirty flush, player `.dat`, world-journal `flush`/`checkpoint_through`)
  **while the journal writer is still live**; join the journal writer; join
  the plugin-host thread. A supervisor deadline may report a failed shutdown;
  it never cancels the future that performs the final save, and no task
  holding durability work is aborted while it can still complete.
- Every accept loop (game and dashboard) has bounded admission and joins or
  aborts its children explicitly.

## Durability contract

Four durable streams exist and stay separate; each operation names the one it
uses:

| Stream | Owner | Covers | Acknowledgement |
| --- | --- | --- | --- |
| world-chunk WAL | `mc-simulation` journal | journaled block-edit runs, receipt-bearing chest/warehouse composites with their inventory receipt | write-behind; fsync before reply only for receipt-bearing runs, `flush`, `checkpoint_through` |
| regional entity WAL | `mc-entity` decisions via the same writer thread | regional commit decisions | write-behind (ADR 0005) |
| plugin-storage journal (`solaris/plugin-storage-v1/journal-v1.bin`) | `mc-simulation::services::storage` | plugin records and the settlement/resident/order ledgers (`commit_batch`) | fsync before the projection is installed |
| player `.dat` checkpoint | `mc-simulation` persistence | player state and inventory at checkpoint/save | fsync + atomic replace at save |

A receipt-bearing composite (warehouse transfer, resident haul) is decided in
the world-chunk WAL and only then projected into the plugin-storage journal;
`PluginStorage::commit_batch` refuses a batch that carries player inventory.

Ordinary chest clicks and player inventory changes are checkpoint-durable, not
WAL-durable; that is vanilla-equivalent and intentional. A composite operation
that must survive a crash uses a receipt-bearing WAL route and fails closed
without its journal. There is no universal transaction.

## Enforcement

- Direction is enforced by Cargo manifests. `code-health` rules that only
  restate a Cargo edge are deleted when the edge exists; remaining tripwires
  stay narrow and structural.
- `mc-simulation` tests build and run without `mc-net`; `mc-net` tests exercise
  decode/publication against `mc-simulation` handles. Tests move with their owner.
- Test seams of `mc-simulation` state that `mc-net` integration tests need
  (journal writer pause/stub, storage fault points) are additive items under
  `#[cfg(any(test, feature = "test-hooks"))]`; the feature is enabled only from
  `mc-net` `[dev-dependencies]`. It never gates a public enum variant or any
  shape a production caller matches on, and no test hook is unconditional.
  Seams used only by `mc-simulation`'s own tests stay `#[cfg(test)]`.

## Rejected

- Keeping the modular monolith and adding more text tripwires: the audit shows
  the tripwires do not cover the invariants that matter (single writer, lock
  scope, import cycles), and a 277 K-line compile unit is the build cost.
- One crate per domain (`mc-inventory`, `mc-container`, `mc-settlement`,
  `mc-tick`): more manifests and re-exports without a new enforceable boundary.
- A trait/service layer between `mc-net` and `mc-simulation`: there is one
  implementation on each side; the boundary is concrete types and handles.
- A universal transaction or event bus across the three durable streams.
- Making the regional lanes async or moving the tick into the Tokio runtime:
  the lanes are correct as OS threads; the tick must be allowed to block.

## Consequences

Ownership questions get compiler answers: gameplay code that imports a session
or packet type does not build; a second inventory writer has no reachable
private field. Focused tests of the game no longer link networking. `mc-net`
becomes reviewable as a protocol adapter. The cost was one large staged
migration; its historical order and acceptance are archived with
`CORE_ARCHITECTURE_PLAN.md` at commit `1ada2ce`.
