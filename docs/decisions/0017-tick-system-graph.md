# ADR 0017 - Explicit tick system graph over native entity columns

**Date:** 2026-10-23  
**Status:** Proposed graph; bounded one-system prototype implemented. This is not
approval to migrate the rest of the tick or claim a whole-server speedup.  
**Parent contracts:** [0013](0013-core-model-v2.md),
[0016](0016-mob-capabilities.md).

## Decision and scope

Represent native tick work as systems with declared resource reads/writes and
explicit semantic ordering edges. Keep `EntityStore`, its dense columns and
exact lanes, snapshots, native intents, deterministic merge and the single tick
journal. Do not introduce an external ECS, component registry, entity identity,
plugin scheduling API or second mutable authority.

The prototype registers exactly **EntityStep**, the existing pure
`step_entity` map in `tick::run_entity_phase`. It includes passive movement/think
and the existing item/player/special cases; separating passive rows by type would
add admission/scatter work and change a carefully ordered boundary for no
benefit. Admission, target preparation, movement writeback, pickups, hostile
specials and all other tick phases are unchanged. The old path was **already
parallel**. This experiment measures the value of executing this phase on two
cores versus serial, not an invented serial baseline for the old server.

## Current order and dependencies (code, not intended architecture)

Source baseline: `0a86cfffc173d9fe13e9d0795a6c5494fc33a222`.
The anchors below are symbols; line numbers describe that baseline and may move
when the separately owned tick module extraction lands.

| Ordered owner stage | Source | Reads / writes and required predecessor |
| --- | --- | --- |
| Ticket refresh; spawn/despawn; input and player prephase | `tick.rs:2599-2665`, `run_dimension_tick_inner` | Refresh activity; mutate entities, player inventories/vitals, mining, effects and controls; stage intents. Natural spawns precede player inputs; held-main-hand snapshot precedes swap/attack. |
| Chunk ensure/eviction/hydration; block-entity sync; settlement autonomy and environmental helpers | `tick.rs:2668-2715` | Inputs determine desired chunks. Cancel simulation debt for external writes before eviction; hydrate before readers see new chunks. Can spawn entities or mutate world/rows. |
| Entity lifecycle, piglins, TNT and snowballs | `tick.rs:2717-2724` | Reads current loaded world and rows; writes state/intents before the entity snapshot. |
| Water-sensitive damage and enderman state; motion hints, active admission, rare helper rebuilds, row projection and target grids | `run_entity_phase`, `prepare_rows`, `read_row` (`tick.rs:5923-6152`, `5466-5555`) | Snapshot is **phase-local**, after earlier same-tick owner writes, not necessarily the last published snapshot. Admission may run by data; rare mutable helpers run before the projection completes. Reads entity columns, cold rows, registry, activity, player modes, current world revisions and clock. Writes prepared rows, resting/special lanes and target projections. |
| **EntityStep** | `tick.rs:6153-6184`, `step_entity:6373` | Reads frozen prepared rows, live entity columns/cold rows, target tables, registry, world and clock/rules; returns movement and raw intents per row. Does not mutate authority. |
| Ordered movement writeback | `tick.rs:6188-6291` | Requires all row results. Resting timers first; then dense-slot movement, hopper wakes, fall damage, stamina and per-actor sequence assignment. Item merging follows movement. Writes columns/indexes and staged intents. |
| Pickup and hostile/projectile postphase, raiders, vehicles, end fight and soldier attacks | `tick.rs:6292-6303`, `2728-2736` | Read post-move positions and updated pickup delays; specials can mutate state and add intents. These cannot overlap movement writeback. |
| Intent merge | `TickState::merge:1395`, `intent::sort_intents:270` | Stable deterministic sort, then footprint/kind revalidation against accumulated state; reject the entire stale/conflicting intent. Writes state and staged commit effects. |
| Block pipeline | `tick.rs:2747-2804` | Apply accepted block writes; cancel/catch up old simulation debt; unsupported blocks; farming, fluids, furnace, sculk, crafter, redstone, jukebox, brewing; shelf/shulker; late ignition/dispenser writes; falling blocks and after-block mob work; portal and fluid scheduling. Earlier world writes are visible to later stages. Shared overlays and update chains are not independent columns. |
| Commit and native tail | `tick.rs:2807-2973` | Fold blocks, apply merge effects/deaths/pickups/drops, late block owners, experience and conversions. Settlement allocator precedes villager brain; then merchant/mount/allay/equipment, drops, end fight/raids. Clock/weather, equipment and advancement state follow. |
| Seal snapshot | `tick.rs:2974-3033` | Drain late native plugin block writes, fold again, snapshot chunks and entity views, drain reliable outboxes. No reader sees half a movement or block phase. |
| Journal admission then external publication | `core.rs:1845-1876`, `1946-2023` | `run_tick` builds output; outer owner gathers journal entries and enqueues the one tick batch before broadcast. Failed journal admission halts authority. This is not an fsync-per-reply contract. |

`run_tick` also coordinates dimensions; a local system graph must not reorder
cross-dimension transfer or global tick sealing. `/tick freeze` still executes
inputs, player work and merge/commit while gating simulation stages. Scheduling
must use the existing admission/gates, not infer inactivity from an empty lane.

## Declaration and resource model

A system declaration has a stable identity, immutable read set, write set,
semantic predecessors/successors, and partition grain. Resources name **versions
and ownership**, not mob type names. For example:

```text
Prepared barrier
  -> EntityStep
       read: PreparedRows, EntityColumns, World, Registry, Targets, ClockAndRules
       write: OrderedRowOutput (one owned result per input row)
  -> MotionWriteback barrier
```

`tick/system_graph.rs` binds that declaration to `StepCtx`: shared references to
all input resources, and one exclusive `Vec<RowOutput>` destination. This is the
actual memory-safety fence. A bitset is not a license for unsafe aliasing.
Compile-time checks prohibit authoritative writes and read/write overlap for
this row-system executor. No `&mut TickState` enters a worker. Shared cold entity
access is intentionally declared broadly: panic handling reads an attacker's
position through the entity ID/slot index, not only the current row.

The minimal scheduler chooses a direct serial indexed map on one worker or
fewer than two 256-row partitions; otherwise it uses the existing long-lived
Rayon pool and indexed `collect_into_vec`. It reuses the existing output buffer,
allocates no graph or per-job resource maps, and introduces no task channels,
locks, dynamic dispatch or thread creation. There is one system node, so its
prepared/writeback edges are enforced by typed invocation between the existing
owner barriers, not a general-purpose topological queue. Undeclared phases
remain opaque, ordered owner work, never presumed safe to overlap.

For a later multi-system graph, compile the fixed declarations once at startup:

1. Require explicit versioned producer/consumer edges for derived resources
   (`Targets@prepared`, `Position@post-motion`, `World@post-blocks`).
2. Reject missing producers, duplicate IDs and cycles; unknown access is an
   exclusive owner resource, not an empty access set.
3. Read/read systems may overlap. Any write/read or write/write intersection
   requires an ordering edge, unless the typed executor proves disjoint slice
   ownership. Disjoint entity *kinds* alone are not that proof: targets, drops,
   compaction, spatial indexes and inventories cross kind boundaries.
4. Preserve semantic edges even when memory access is disjoint (sequence
   allocation, event order, RNG streams, journal sealing and plugin admission).
5. Choose ready systems in stable graph order. Pass typed borrowed views and
   private output buffers; drain those buffers in declared logical order,
   independently of completion order.

Those five graph-compilation steps are the **design**, not extra runtime added
by this prototype. General graph validation is useful only with a second
migrated system and its real conflict cases.

## Determinism and merge

No new RNG stream or reduction is introduced. `step_entity` receives the same
row, tick, registry and target ordering and executes unchanged arithmetic.
Indexed collection leaves dead-row holes in place; it does not filter into
unordered per-worker vectors. Serial execution yields the same slots. Workers
never allocate global actor sequence numbers or entity IDs.

The owner drains results in the old dense-slot order, preserves each row's raw
intent order, appends fall damage at the same point, assigns actor sequences
and performs the existing stable intent sort. Whole-footprint validation,
post-move pickup selection, item merging and journal ordering remain untouched.
Future simultaneously ready systems would need separate ordered output lanes;
letting completion order allocate sequences is forbidden even if writes do not
conflict. Floating-point sums cannot become unordered parallel reductions.

## One or two cores, and scaling up

One core runs the row kernel directly; no Rayon splitting/queueing is needed for
that node. On two cores a sufficiently large row slice can share the same frozen
input and write independent result slots. Preparation/writeback still bound the
speedup; graph notation alone cannot remove them. Small populations remain
serial. Larger worker pools may steal row partitions without changing results.
Only the existing pool's capacity is used; the prototype adds no worker count
setting and no oversubscription policy.

With multiple declared systems, prefer coarse independent ready systems before
splitting already tiny jobs; share a single core-worker capacity budget. Nested
Rayon work should reuse that pool, not create a pool per capability. Scheduling
across a semantic barrier is never an optimization. Block influence zones and
ordered within-zone chains require a separate proof, not reuse of row slice
independence.

## Migration path (only step 1 implemented)

1. **EntityStep:** declare the existing immutable row map, keep all boundaries;
   compare old-map and scheduler state hashes, then measure serial/two-worker
   execution on the same binary and fixture.
2. **Preparation:** separate serial mutation/hint rebuilds from immutable
   projections; declare derived row/target producers and retain their ordered
   reduction. Do not move sensor cadence or rare helpers while extracting it.
3. **Post-move consumers:** name pickup and special-system resources, initially
   execute in old order. Only split immutable queries from their owner commits
   once dependencies and output ordering are covered by regressions.
4. **Commit-tail capabilities/brains:** declare narrow columns and private
   output lanes; preserve allocator-before-brain and native custody boundaries.
   Start serial, then overlap only proven read-only/disjoint producers.
5. **Input/merge/block/seal:** represent existing owner barriers explicitly.
   Keep input order, deterministic merge and one journal seal. Independent
   block-zone work is a separate ADR 0013 implementation project, not part of
   this graph prototype.

Every step needs its own approval, parity fence and measured benefit. No step
implicitly enables the next. Remove the corresponding old production route at
each cutover; test-only reference kernels may remain to detect ordering drift.

## Risks and evidence boundary

- **Incomplete declarations:** a cold-row lookup or helper can hide cross-row
  reads. Broad immutable `EntityColumns` is conservative and honest. Narrow it
  only together with a narrower typed view, not by editing metadata alone.
- **Version confusion:** published snapshot `t` and the current same-tick
  phase snapshot are not interchangeable. Moving a reader before input or
  movement writeback changes gameplay even without a Rust data race.
- **Slot invalidation:** spawns/removals/compaction must remain outside worker
  borrows. The parity fixture removes a slot mid-run and edits support geometry.
- **Overhead and memory bandwidth:** 256 is inherited grain, not a universal
  optimal value. Do not add per-entity dispatch/bitset checks or allocate a DAG
  each tick. Two-core results do not establish scaling to large CPUs.
- **False speedup:** the legacy row phase already uses Rayon. Compare scheduler
  parallel to legacy parallel as well as to serial; report row and surrounding
  phase timings separately, and do not call a debug microbenchmark server TPS.
- **Compatibility:** no storage schema, network representation, authoritative
  phase order, freeze rule, AI cadence or vanilla data changes. Unit state hashes
  are not a graphical-client, save/recovery or full workload acceptance gate.

The sibling `tick/system_graph_tests.rs` contains a 48-tick, 1,024-mob replay
through real `run_tick`, comparing the legacy map with scheduled execution on
one/two/four workers on every tick. The affected-state digest covers entity
bodies/cold rows (including UUID/revisions), exact position/velocity/rotation
bits, ground flags, actor sequences, tick/day and resident block images. It
intentionally does not claim to serialize every unrelated connection/plugin
cache. An ignored measurement uses 128 and 8,192 passive mobs, eight warmup plus
48 measured entity phases, two workers, interleaved parallel/serial/legacy
samples and equal final state hashes. Use the built **release** test binary.
Quiet acceptance requires a Main-authorized two-core `mem-run.sh -x -p -m 6G`
window with explicit affinity to distinct physical cores (the runner's `-x`
overrides its usual perf affinity). `cargo test --release --no-run` is outside
the timing lane. Main may separately authorize ordinary-lane microbenchmarks;
label those **approximate, contended machine**, use medians across repetitions,
and do not treat them as quiet-window acceptance. The delivery report records
actual results and commands, rather than treating test existence as a pass.

### Prototype result (ordinary lane, approximate)

Main authorized an interim **approximate, contended machine** release experiment
instead of the paused exclusive windows. Four samples per mode on two physical
cores (0/2), two workers, 48 measured phases per sample:

| Mobs | Serial row median | Parallel row median | Serial phase+merge | Parallel phase+merge |
| --- | ---: | ---: | ---: | ---: |
| 128 | 1,683.5 us | 1,688.5 us | 2,616 us | 2,634 us |
| 8,192 | 102,778.5 us | 78,801.5 us | 175,743.5 us | 154,118 us |

Totals are over 48 phases. Large-population speedups were approximately 1.30x
for the row kernel and 1.14x including preparation/writeback/merge. Small work
did not benefit. Large parallel samples ranged 56,325–106,692 us; these are not
quiet acceptance figures. Legacy parallel medians were 88,296 us row / 159,454.5
us phase+merge; the scheduler uses the same parallel algorithm, so that noisy
difference is not evidence of a scheduler speedup. Every variant's final state
hash matched. The 48-tick full-tick parity test also passed on 1/2/4 workers.

Retain coarse data parallelism and the direct small/one-worker path; do not
infer whole-server TPS or tune a new grain from this experiment. Raw samples,
binary identity, successful instruction counter and exact commands are in
`.analysis/codex-logs/TickPipeline/report.md` and `microbench.json` (local
receipts, not required runtime files). Proper quiet-window measurement remains
deferred by Main.
