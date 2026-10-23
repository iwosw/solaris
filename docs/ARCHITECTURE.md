# Solaris core architecture

Status: current runtime after [ADR 0014](decisions/0014-v1-core-retirement.md)
Phase 5. [ADR 0013](decisions/0013-core-model-v2.md) owns the native core model;
the old regional-owner and staged-extraction designs are superseded, not parallel
production paths. This document is a navigation summary, not a second authority.

## Composition and dependencies

```text
solaris-v2 (mc-edge binary)
  mc-edge: configuration, CLI, authentication (Mojang or a trusted Velocity
           proxy via modern forwarding), sockets, console, dashboard,
           per-viewer fanout and Loader negotiation
    mc-core: authoritative gameplay, native tick, snapshots, intents/merge,
             plugin admission, tick WAL and checkpoints
    mc-protocol: exact Minecraft packet catalog/codecs
    mc-loader: Loader manifest/artifact/ACK/sound wire contract
  mc-plugin-host + mc-script: Wasmtime worker and bounded WIT/DTO boundary
  mc-vanilla: vanilla process/registry capture and minimal protocol client
             for content import and tooling
```

Shared domain/data/world/physics/worldgen/NBT crates retain their useful native
implementations. `mc-test-harness` is tooling, never the composition root or a
production dependency. Production builds do not require sibling Loader or
first-party plugin source repositories. Process-wide tracing and signals are
composed by the binary, not imposed by reusable gameplay libraries.

## Authoritative tick

1. Drain authenticated player inputs, admitted plugin intents and ticket changes.
2. Advance entities in parallel by data: each reads snapshot `t`, writes its own
   next state and emits intents. Space supplies locality, not mutable region leases.
3. Merge intents in deterministic actor/sequence order against accumulated state.
   Validate the entire footprint, including consumed inventory and dependent
   effects; stale or conflicting work is rejected whole.
4. Settle block/random/scheduled work on the native tick owner. Independent-zone
   parallelism is an ADR model constraint, not permission to invent a new runtime
   scheduler or weaken within-chain ordering.
5. Commit one tick change set and publish immutable snapshot `t+1`.
6. Plan per-viewer packets on the edge's own workers, within each connection's
   bounded queue credit. Slow I/O never becomes gameplay authority.

There is one native authority for players, entities, inventories, containers,
blocks, world clock/rules and plugin state. Connection caches retain delivery and
protocol state only. Read-only consumers may retain snapshots; the core does not
wait for readers or permit them to mutate a published version.

Entity feature scans use exact, sorted dense-slot type lanes; player/item pickup
lanes share spawn, kind-change, compaction and world-transfer maintenance with
mob lanes. Membership includes dead rows and never relies on lazy motion flags:
consumers retain their authoritative component/liveness checks. Pickup remains
after motion writeback, including its pickup-delay decrement, with the first
eligible item chosen in dense-slot order. Row preparation combines dolphin
presence, chase-consumer counts and ordered breeding cells in one projection
reduction after serial rare-helper rebuilds (which can change enderman chasing).

The row movement/think map is the single declared-system prototype in
[ADR 0017](decisions/0017-tick-system-graph.md). Typed immutable inputs and
ordered scratch outputs retain the existing preparation/writeback barriers;
the scheduler selects serial or data-parallel execution. Other tick stages
remain owner-ordered, not a migrated general system graph.

Ordinary monster spawners are native tick work indexed by chunk. Only spawners
near live non-spectator players in active chunks advance; spawn attempts run on
due deadlines and query local entity buckets. Activation, pause and delay reset
write the existing opaque block-entity WAL, not a second persistence store or a
per-tick countdown record. Unsupported custom spawn rules/weighted potentials
fail chunk admission rather than silently changing their meaning.

Structure templates carry typed inhabitant markers, authored persistence and
rotation into native blueprint initialization. Placement claims and stable UUIDs
share the existing entity checkpoint/WAL transaction; claims survive death so
reloading a generated structure cannot resurrect its original inhabitants.
Villager data uses its specialized initializer; unsupported entity types remain
unclaimed, rather than being misrepresented as villagers.

## Threads and waiting

The core owns its tick and long-lived data-parallel worker pool. The edge owns
socket tasks and separate fanout workers. A journal worker writes tick batches
and frozen checkpoints; Wasmtime callbacks execute on a worker outside the tick.
No guest Store, socket write or blocking disk operation runs as gameplay work on
a tick worker. Admission is bounded and event-driven; wall-clock sleeps and
polling do not substitute for producer notification.

World delivery may reconcile to the newest snapshot after a skip. Per-player
results remain ordered and bounded; overflow fails the connection rather than
silently shedding an accepted result. Client rendering and successful socket
writes are not native commit evidence.

## Persistence

Serving persists only native state (`native/state.redb`) plus the tick WAL.
World, player and plugin changes share the same tick recovery boundary. An
acknowledged effect is not an fsync promise: an unflushed crash tail can roll
back consistently. Checkpoint work freezes owned records/immutable chunks and
runs off-tick. Shutdown requests a final durable checkpoint.

Chunk admission removes unused palette entries while counting non-air cells,
then packs only the live palette (or uses allocation-free single-state storage).
Native baseline encoding also drops palette history from an immutable copy;
published snapshots are unchanged. This retains the existing NBT/native schema,
zstd level and WAL/checkpoint ordering, not a new storage format or migration.

Item custody transitions close/refund window-zero cursor and crafting inputs
before opening another menu. Block-container clicks refuse and close a menu
whose loaded storage no longer matches its slot count (including an unavailable
double-chest half). Pickup commit rechecks collector liveness after damage.
Partial allay pickups publish the ground-stack remainder as a full entity
image in the same WAL batch as the allay credit. Dimension-travel wire removals
become durable tombstones only when the entity is not live in that dimension
at the journal boundary, preserving same-tick round trips.

Saved maps are global native rows (`map/<id>`) plus a monotonic `map_sequence`
allocation fence, shared across dimension ticks. Items retain the vanilla
`minecraft:map_id` component; cloning shares a row, scaling/locking creates one
new row only when the cartography output is accepted. Pixel updates sample only
resident columns around held/viewed maps, with a five-tick cadence, eight-map
round-robin limit and 1,024-column budget; the edge only encodes dirty rectangles.
The map palette is extracted from licensed vanilla block states during content
import (`reports/block_map_colors.json`); a missing/invalid palette fails startup.

First-open exploration loot and cartographer map offers reuse the bounded
`/locate` worker. Pending lookups own no inventory and allocate no map IDs.
Container completion rechecks dimension, chunk revision and exact pending loot
table/seed/metadata before publishing map rows, slots and cleared loot metadata
in the same native tick/WAL batch. Concurrent opens share one request; a replaced
or broken container discards the response. Missing destinations omit map loot,
not fabricated targets. Anvil map files and allocation counters are decoded into
native rows on import and regenerated from the recovered native state on export;
unknown map-envelope fields and retained banner metadata remain preserved.

Enchanting-table offers retain their own persisted `XpSeed`; equipment and
consumable probability draws use a separate persisted gameplay seed. Copy recipes
retain the original book/banner as a crafting remainder (returned in player
grids, ejected by crafters). Furnace recipe placement mutates the same shared
input custody as clicks, with reservation fences and viewer revisions. Furnace
recipe-count XP banks share immutable images with the tick WAL, survive native
chunk checkpoints, and settle once into orbs when the container is destroyed.

Anvil is explicit offline import/export, never a serving fallback or dual-write
backend. Conversions retain source worlds, require a new destination and emit
receipts. Offline native backup/restore requires a stopped, exclusively leased
world; the console `backup` copies a running world at a checkpoint plus its WAL
(ADR 0013, native writer). Restore verifies the inventory and checksums. Corrupt or unsupported saved state
fails closed. Earth WorldPlan remains proposed for this runtime (ADR 0012).

## Plugins and Loader

`mc-plugin-host` executes independently built Wasmtime components implementing
`solaris:plugin@0.8.0`; `mc-script` owns runtime-independent values, capabilities
and admission. `[plugins].directory` selects the exact deployment. Invalid
packages and unsupported declarations refuse startup. Legacy per-deployment
strict/expected/grants/reload settings are not current configuration.

Guests see owned values, immutable observations and opaque handles, never native
pointers, region leases, locks, ECS slots or sessions. They submit bounded
operations; the current native owner revalidates session/generation/revision and
custody at the decision. Plugin KV and effects share the native WAL/checkpoint.
`before-build` decisions run outside the tick and return single-use fenced
approvals; unsupported hooks fail startup rather than being ignored.

Optional authored client bundles use `mc-loader`; the Loader is presentation
and transport, not gameplay authority. Server-only packages accept vanilla
26.1.2 clients. See [PLUGINS.md](PLUGINS.md) and [SOLARIS_LOADER.md](SOLARIS_LOADER.md).

## Compatibility, overload and evidence

Exact compatibility targets are Minecraft Java 26.1.2 protocol and Anvil
interchange layouts. Mojang bytes stay outside Git; derive the complete content
cache from the operator's licensed artifact. Gameplay/worldgen are vanilla-like,
not bit-exact RNG or AI timing. Internal APIs/storage have no released
compatibility obligation: migrate callers and delete replaced paths.

Within declared capacity preserve gameplay semantics. Above capacity use bounded
admission, explicit refusal before mutation and delayed expensive work, never
item loss, partial transactions or discarded accepted edits. Account for induced
engine work as well as guest instructions. Do not document old autoscale config
or a historical load result as a current v2 capability.

Current routes: [runtime](architecture/runtime.md),
[state ownership](architecture/state-ownership.md),
[mutations](architecture/mutations.md), [operating](OPERATING.md).
Generated indexes are navigation, not semantics; regenerate them from the
existing `xtask architecture` command. Dated evidence and superseded ADR
bodies keep their original runtime identities and do not establish current
acceptance.
