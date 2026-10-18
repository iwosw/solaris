# ADR 0004 - Staged single-writer simulation boundary

**Date:** 2026-07-11  
**Status:** Superseded by [ADR 0013](0013-core-model-v2.md) and
[ADR 0014](0014-v1-core-retirement.md), Phase 5 (2026-10-15).
The decision and implementation details below are historical v1 records; the
named authorities and source paths were deleted, not retained as current APIs.

## Decision

`SimulationOwner` remains the ordered authority for player/session aggregates,
world mutations, containers, block entities, and transactions that span those
domains. Moving entities are no longer owned by one global simulation writer;
ADR 0005 assigns each entity to one regional ECS owner.

Every simulation command has a monotonic enqueue sequence and a typed result.
The bounded command channel wakes the owner directly. Queue closure or capacity
rejection happens before mutation. Once mutation begins, requester or socket
progress cannot decide whether the mutation committed.

A command that cannot determine its commit outcome is fatal. Solaris stops the
owning runtime rather than retrying, guessing, or publishing a result that might
not match authority.

## Current authority

`SimulationOwner` owns ordering and commit policy for:

- player inventory, cursor, experience, survival, death, and connection-fenced
  mutations used by migrated commands;
- packet-authored world edits and their block-state or mutation-token
  preconditions;
- shared container, furnace, campfire, and sign transactions already routed
  through typed commands;
- cross-domain transactions whose state does not share one regional entity
  owner;
- save barriers and persistence snapshots for its remaining domains.

Conditional block-state and opaque block-entity commits are headless:
`WorldStorage::apply_block_edits_conditionally` prepares all required positions
and delegates to the resident commit kernel. Missing/out-of-bounds positions or
stale state/token preconditions reject the batch before any edit is published.
Single-region execution retains its fast path; multi-region storage edits reuse
the existing staged source fence and atomic publication boundary.

Receipt-bearing server-owned block-edit batches cover settlement structure
portions and resident harvesting, mining, cutting, and planting. They reuse
`ApplyBlockEdits`' regional lane rather than a domain-specific command or saga:
the submitter captures exact mutation-token preconditions and resolves the
bounded `before-build` question before enqueuing the finite block portion. It
prepares the storage batch (including material consumption or resident cargo and
progress) and encodes its receipt before submission; that preparation has no
durable effect until the regional decision is acknowledged. A cancellation is
delivered with the submitted approval and the regional worker refuses it before
block mutation or world-journal reservation. The worker checks the supplied
preconditions and rechecks the captured zone fence before it conditionally
applies/stamps the blocks, appends the chunk after-images and encoded receipt as
one world-journal decision, then publishes. Only the acknowledged decision
permits storage projection and `mark_inventory_projected`; reopening projects
that same receipt before new settlement or resident work. Ordinary server-owned
edits without a receipt retain the canonical staged path, including campfire
eviction and reactivity.

Structure footprint change detection is content-scoped, not journal-scoped: a stage
fence compares a digest of the block states strictly inside the reserved footprint and
fails closed while a covering chunk is unloaded. Keying it on the covering chunks'
durable journal position was wrong: the world's own scheduled-block-tick transaction
advances that position for work no player did, so a build paused as `site_changed` on
its own surroundings. The deliberate narrowing is that an edit elsewhere in the same
16x16 chunk - digging beside the reserved box - no longer pauses the operation, while
any block change inside the footprint still does.

`WorldStorage::commit_opaque_block_entity_conditionally` loads the chunk, then
checks state/token and publishes NBT under the existing resident region lock.
The net-side check/write split and unconditional opaque storage setter are gone.
Campfire use publishes inventory/cooking state only after that commit accepts;
hopper persistence captures its expected state/token from one chunk snapshot.

The storage facade publishes dirty world state, not a disk durability receipt.
Durability-backed scheduled-block transactions still persist before publication;
normal saves retain their existing dirty-flush and journal fences. Inventory,
entity/drop effects, command ordering and socket publication remain outside this
world-only operation.

Player inventory publication uses the local, session-free
`PlayerInventory::try_update` operation: plan against an immutable inventory,
then replace it only on success. Player actions retain their inventory, cursor
and menu-input preconditions under the existing player-state lock. Script
inventory deltas and Loader item grants plan from canonical
`PlayerPersistedState.inventory`, not the potentially stale connection
projection; that projection is refreshed only after a successful commit.

`RegionalOwnerRuntime` and its lane-owned `EntityStore` instances own entity
identity, lifecycle, retained state, AI, motion, combat mutations, vehicles,
regional transfers, and entity persistence snapshots. `SessionRegistry` and
published snapshot maps are indexes or projections, not a second entity
authority.

The session, connection task, plugin host, persistence worker, and outbound
transport never receive mutable ECS references.

## Current execution path

A normal tick has these boundaries:

1. Accepted `SimulationCommand` values run in enqueue order. Session-authored
   commands revalidate the current `SessionId` immediately before mutation.
2. Entity commands are routed to the current regional lease. Same-owner work
   mutates that owner directly; multi-owner work uses ADR 0005's ordered
   transaction protocol.
3. Regional owner lanes apply local goals. Central domains that intentionally
   interact between goals and physics then commit their work.
4. Regional owner lanes reread current ECS state, run exact local physics, and
   commit kinematics.
5. Semantic results and compact movement facts are published only after commit.
   Encoding and socket writes run after every authority lock or lease is gone.
6. A save barrier captures one completed authority boundary. Disk IO follows
   from immutable snapshots.

Bounded dirty-only saves advance a world-local region/chunk cursor when selecting
a batch, wrapping after the last position. Repeatedly dirtied early chunks cannot
starve later positions. Selection advances even when mutations invalidate a
pending batch; region-version and dirty-generation fences remain unchanged.

Startup requests the existing full checkpoint, including the simulation save
barrier. Unlike pressure-only flushing, it must persist the warmed spawn snapshot
even after players begin mutating chunks. It uses the normal checkpoint's
serialized capture, snapshot installation and journal watermarks; it does not
relax pressure-flush generation checks or add another save path.

The whole-world `entities.dat` checkpoint uses an explicit bounded named-NBT
document policy at its file boundary, not the generic per-root string budget.
Ordinary entity records repeat keys and retained-state JSON; the aggregate
string ceiling is the unchanged 64 MiB encoded-file ceiling, while individual
strings retain their u16 Modified UTF-8 limit. Decoder allocation admission is
64 MiB plus eight times the actual bounded encoded length: four times for the
existing cumulative string-decoding charges, four times as workload-informed
structural headroom. This is a finite resource policy, not an NBT allocation
invariant or a peak-RSS guarantee. Generic NBT limits remain unchanged.

Entity-file writer preflight accounts for the same string, array, list,
rounded compound-slot and reader-frame allocations before creating a temporary
file. Over-budget trees cannot replace the previous checkpoint. The reader and
writer retain the existing depth, node, entry, collection, wire and gzip limits;
readback requires one gzip member and exact root consumption. Completed
compressed output is checked before the existing fsync/atomic-replace sequence.

Entity recovery replays the durable checkpoint and WAL, then initializes an
empty regional owner without journaling those after-images again. All region
stores are validated before installation. Recovery keeps lifecycle, sequence,
and allocation high-water boundaries monotonic, consumes no new mutation
sequence, and leaves checkpoint/WAL bytes untouched. Partial in-memory
installation fail-stops instead of serving a partial population; a restart
replays the same unchanged durable inputs. Ordinary gameplay mutations retain
the existing journaled path and its unchanged admission limits.

The command queue is admission and ordering, not an entity tick scheduler.
Ordinary entity AI and physics do not traverse it.

Chunk generation, preparation and simulation planning share bounded CPU
admission. Chunk-queue pressure includes requests still awaiting execution, not
only completed results. It reduces producer rates and view limits without
reducing the CPU capacity needed to drain those requests. Tick-time and memory
pressure reduce admission gradually; shutdown draining still reduces capacity
to one immediately. The application policy lives in `server/runtime_control.rs`;
there is no second executor, inline planning bypass or operator thread setting.

The shared CPU pool starts at one worker and its default ceiling is the
physical core count of the allowed CPU set. Sustained chunk-queue and
first-chunk pressure grow the CPU and IO pools by one worker per `ScaleDown`
step. Measured tick-time pressure fills the CPU pool to its existing ceiling
even when the operator-facing producer-limit cooldown still returns `Hold`;
it must not serialize regional simulation for that additional 60-second
window. Memory pressure grows nothing. The pool never decays while pressure
is observed. A completed tick with selected entities also resets the shared
fully-idle streak: regional owner work does not acquire chunk-worker permits,
so an idle chunk queue is not an idle simulation. Clearing active selection
also clears the selected/population observations, so the last observer's
departure cannot keep historical work marked busy. After a sustained fully
idle window the pool still shrinks one step. Regional entity owner lanes follow
the live CPU pool size. Producer rate/view reductions, their pressure
duration, the physical-core ceiling and immediate shutdown draining remain
unchanged; this is not a startup thread-count override or a second executor.

The `tick-tail` per-tick trace distinguished one-lane convergence from scheduler
delay: its initial measured window had 71 ticks with all 100k bodies on one
owner, followed by 329 ticks with six owners. Slow-owner CPU time nearly
matched wall time. Alternating same-window owner cuts kept six lanes throughout
both 400-tick candidate windows and reduced their tails, with shared-host
contention reported separately. Raw profiles and the final receipt live under
`.analysis/codex-logs/tick-tail/`; only Main's exclusive unchanged-budget
living-world gate establishes p95 <= 50 ms / p99 <= 60 ms acceptance.

Standalone and regional goal transactions still borrow their prepared requests
and retain exact transform/goal/path stale-state checks and planned speeds.
The owner lane instead preflights its selected ECS handles while it holds the
store, then resolves and commits each entity directly in that same uninterrupted
operation. It does not materialize population-sized request/result maps, active
ID sets or ordinary goal checkpoints. Only override, follow-target and terrain
facts need maps. Missing job-site snapshots, nonlocal current-goal references,
snapshot overrides and cross-region villager candidates reject the region
before mutation; pending input commands must already be drained.

Owner publication captures identity, motion and gameplay facts directly rather
than constructing detached full simulation results. Standalone/regional
transaction fences are unchanged, and goals remain separate from physics so
central operations can still run between them. Owner diagnostics report
scan/selection/apply/post; apply now includes bounded resolution and commit.
There is no old owner transaction switch or diagnostic compatibility path.
The earlier borrowed-request measurement remains in
`.analysis/codex-logs/tick-budget/receipt.md`; the direct-owner A/B is in the
`tick-goals` receipt below.

Grounded navigation that made progress along an unchanged retained path now
follows that path without another bounded candidate probe. The body must still
have horizontal velocity and no vertical velocity; stopped, airborne, stalled,
new-target and exhausted-node states return to bounded probing. The pathing
probe must explicitly admit the next foot position in its loaded simulation
domain without a terrain-collision marker; unknown probes conservatively veto
reuse. This prevents a progressing retained path from steering out of the
active chunk window. Collision physics still runs every tick, and a failed
move invalidates reuse on the next goal tick. This follows the retained-node
boundary in 26.1.2
`PathNavigation.tick`, not a blanket every-other-tick navigation skip. A settled
Wander pause returns before allocating a request only while its original
`resume_tick`, position and zero horizontal motion are unchanged. Selection
rolls, target epochs, pause duration, panic timing and Brain due timing remain
unchanged.

The ECS also indexes entities with villager type **and retained villager data**
in `VillagerBrainEntities`, alongside its existing breeding/sheep indexes.
Insertion, snapshot restore/type conversion and removal maintain membership;
regional transfer uses those same operations. Brain preparation intersects
this index with the captured ordered selection, retaining the existing
lifecycle/chunk checks and all non-due gossip sources. An empty index skips
preparation entirely. This is a membership index over authoritative ECS facts,
not another Brain or spatial authority. Any future direct metadata mutation
must maintain membership as part of the same owner operation.

Dense physics passes also reuse a pass-local lookup of immutable loaded/missing
chunk snapshots: at most 256 rectangular slots, built only with at least four
body queries per slot. Sparse/wide passes use the snapshot lookup directly.
This avoids repeated chunk hashing, not block/material/shape decoding, and dies
with the physics snapshot. Original chunk-identity fences and every body's
fluid, powder-snow, collision and unloaded behavior remain unchanged. Direct
candidate sorting moves references and collision flags instead of duplicate
full fences; crossing/rejected fallback candidates keep their original order.

Owner physics prepares each selected body's exact velocity-swept sample bounds
and current-position chunk once, then carries them through snapshot selection,
sample completeness and sampler setup. The selected query remains unchanged;
every body still runs the exact fluid, powder-snow and collision kernel. After
physics publication is ID-sorted, the already ID-ordered goal publications use
one monotonic replacement walk instead of a binary search for every body.
Neither cut changes the immutable chunk-identity or entity/lease commit fences.
Debug assertions check the carried query/bounds pair and both publication
vectors' strict ID order before using these optimizations.

A same-window 400-tick diagnostic bracket observed physics medians of
21.087 ms (baseline) and 20.665 ms (prepared bounds plus replacement walk).
The flat owner-physics symbol plus its closure fell from 4.98% to 3.32% of
whole-run CPU samples; these include setup, and the shared host prevents a
canonical latency claim. A 160-body, 256-tick fixture produced bit-identical
position, velocity, grounding and collision results. Commands, rolling-profile
sample caveats and the rejected unmeasured block-cache experiment are recorded
in `.analysis/codex-logs/tick-phys/receipt.md`. If sampling/query semantics
change, regenerate the carried bounds from that exact query; if goal publication
ordering changes, restore an explicit ordering boundary before the replacement
walk. There is no old execution switch or retained collision approximation.

The unchanged living-world benchmark forces all 100k seeded types to slow
Wander goals; its 40k villager types have no retained villager data. Measurements
found roughly 100k goal/pathing requests and physics candidates per tick, no
whole-region fallback in the sampled window, and fewer than 1% settled bodies.
An instrumented navigation-only run resolved roughly 6.2k candidate probes per
tick; the membership index removes the empty-Brain population scan rather than
changing real villager behavior. The final direct-owner/physics run observed
tick p50/p95/p99 of 85.682/169.264/189.772 ms, versus the clean baseline
99.467/163.862/217.541 ms. Goal median moved from 29.668 to 19.284 ms, but the
unchanged 50/60 ms budget is still missed on the contended host. Exact A/B
stages, vanilla bytecode identity, correctness gates and the next measured cut
are in `.analysis/codex-logs/tick-goals/receipt.md`. Replacing retained path,
membership or transient lookup representations must preserve these
collision/progress, metadata-lifecycle and immutable-world fences; there is no
old-path switch.

Entity-ID-keyed goal, physics, routing, tracking and visibility collections now
use `EntityIdMap`/`EntityIdSet` rather than SipHash-backed collections. These
keys are server-assigned runtime identities; UUID, session, chunk, string and
network-authored keys retain their existing hashers. The change is confined to
lookup/membership representations: sorted decision and mutation inputs, RNG
inputs, due ticks, stale-state/lease fences and visibility epochs are unchanged.
Adjacent flat profiles of the same living workload observed sampled hash-symbol
CPU share of 6.27% before and 4.36% after. Goal/physics/dispatch medians were
32.764/40.473/6.470 ms before and 21.026/30.121/7.795 ms after; shared-host
contention prevents treating those latency deltas as isolated causal savings.
The 50/60 ms tail budget remains unmet. Exact windows and correctness/population
gates are in `.analysis/codex-logs/tick-hash/receipt.md`. Restore standard
hash-backed collections if key provenance becomes attacker-controlled; replace
the aliases, not ownership or gameplay scheduling, if a measured better
representation supersedes them. There is no runtime fallback or tuning switch.

Regional movement publication can outlive a coarse lane version because central
fallback work committed unrelated entities. Refresh now compares the already
committed ID-ordered movement facts on their current native owner lanes, rather
than fetching and sorting a second population-sized physics/kinematics batch.
It retains exact current lease/epoch routing, alive/unclaimed/unattached checks
and equality of position, rotation, velocity and grounded state. Every matching
lane supplies a captured version; after reacquiring the session lock, publication
rechecks all captured versions. Changed facts fail closed; replaced topology or
a later mutation invalidates the captured fence. The original movement facts,
tracking budget, packet cadence and post-commit ordering are unchanged.
Same-environment 1200-tick A/B traces
reduced refresh p95/p99 from 89.796/111.362 ms to 20.214/21.331 ms; dispatch
p95/p99 fell from 110.053/135.497 ms to 32.562/33.771 ms. The contended host still
missed the overall 50/60 ms tick budget. There is no duplicate DTO refresh path or
operator switch; if tracking needs additional authoritative facts, extend the
native comparison and its version fence before publishing them. Receipt and
remaining stage costs share the `tick-budget` evidence above.

Post-physics fall damage was another coarse-version invalidator: its commit and
finalization advanced the lane twice even when only health/damage metadata
changed. Owner lanes now retain the last movement-relevant revision as well as
the unchanged coarse revision. Only damage-only commits whose exact native
kinematics fences are present and equal before/after can leave that marker
unchanged; lethal damage, every other mutation and rollback invalidate it
conservatively. The single writer publishes a relevant marker before its coarse
revision. An authority-matching opaque movement proof can be re-fenced only
when the marker shows no intervening relevant revision and the coarse revision
is stable across the read. Ordinary snapshot fences remain coarse-stale, and
publication still checks the final coarse fence under the session lock. Fast
repair merges the already ordered lane runs by ID, preserving full repair's
tracking ordinals and packet order without copying/sorting all movement facts.
Alternating 400-tick living-workload A/B runs reduced repair-prefix medians from
20.919/19.053 ms to 0.028/0.027 ms and slowest-5% dispatch medians from
26.849/24.762 ms to 6.089/6.106 ms. Tick p95 fell from 79.559/78.425 ms to
64.722/65.806 ms and p99 from 91.554/86.456 ms to 67.970/68.913 ms; the unchanged
50/60 ms overall budget remains unmet on this shared host. Owner physics is now
the largest remaining slow-tick stage. Commands, noise and native/packet
equivalence evidence are in `.analysis/codex-logs/tick-repair/receipt.json`.

Adaptive view/rate limits and work budgets use monotonic, continuous pressure
and recovery windows, independently reset after each one-unit step. Both policy
durations are seconds, normalized to at least 60; a step requires strictly more
than that duration. Recovery also requires 20% tick-time headroom. Source recovery
rechecks the latest tick and memory observation rather than manufacturing a
healthy sample. Coalesced producer notifications retain an aggregate-zero
recovery barrier before pending pressure states, including recovery followed by
reactivation before consumption. Isolated slow-client shedding cannot establish
continuous overload. Queue and memory admission fences and explicit draining
remain immediate; delaying adaptive tuning does not delay those safety bounds.

Resident and dirty byte usage are updated with the existing chunk-publication
counters. Publication measures the replaced chunk before releasing its snapshot
and the resulting chunk after mutation, applying byte deltas for insert,
replacement, growth/shrinkage, clean/dirty transitions and removal. Storage
admission and statistics consume these totals instead of rescanning every
resident chunk. The heap estimator, budgets, save-health checks and cross-region
publication fence are unchanged; there is no parallel resident cache or new lock.

Scheduled-block plans run on the shared admitted CPU workers. The serial commit
path reuses its first snapshot-based plan because no earlier group in that batch
has committed; it does not reacquire CPU merely to produce the same plan again.
Every later group replans against current snapshots so repeated regions observe
earlier changes. Existing state/token and due-prefix checks still reject stale
plans without consuming their work. The plan type and synchronous planning live
in the scheduled-block domain; CPU admission and blocking-worker dispatch remain
in play orchestration. The domain does not own an asynchronous execution path.

Full chunk-light computation shares those CPU permits. Direct-sky boundary
seeding records each column's open-sky bottom, then queues only its bottom edge
and the intervals below adjacent columns' bottoms. This replaces a measured
full-volume six-neighbour scan; propagation, opacity, missing-neighbour handling
and publication fences do not change. Bounds are local to one computation, not
a retained cache. Full-source propagation comparison guards the optimization.
If direct sky no longer forms a top-open interval, replace this seeding rule;
there is no legacy scan fallback or runtime switch. Measurement and client
evidence live in the `streaming-light` checkpoint linked from `docs/MEMORY.md`.

## Invariants

- One mutable authority exists for each field at any instant.
- Queue-full, queue-closed, stale-session, stale-lease, and failed-precondition
  outcomes do not mutate state.
- Cancellation is checked before mutation. Cancellation after an accepted
  commit cannot turn the commit into rejection.
- Composite transactions validate every expected player, world, container, and
  entity value before changing any of them.
- Cross-domain locks use their documented order and are never held across an
  owner wait, packet encoding, or socket progress.
- Publication is a post-commit projection. A stale projection fence causes
  revalidation or fail-closed omission, never speculative publication.
- Save and shutdown wait for exact completion notifications. Wall-clock sleeps
  and timeout-based success are forbidden.
- Durable replay uses recorded decisions and watermarks. It does not infer a
  commit from partial messages.

## Warehouse inventory transfer (landed 2026-09-14)

Settled during implementation, recorded here because it constrains the receipt:
the run allocates the decision id and the response carries it, so the receipt is
encoded before that id exists. A player endpoint's receipt fence revision IS the
post-commit `inventory_operation_revision`, which `PlayerInventoryRecovery` and
`load_player_state` force to be the journal decision id (its monotonic guard
makes any other numbering skip a replay write), and a submitter pre-reservation
would break `WorldChunkJournal::record_reserved_decisions` contiguity: a run
reserves one block per run and cannot wait for a foreign pending id without
deadlocking the owner task that must process the deposit. A warehouse transfer's
`Transfer` result therefore names the container's resulting fence (binding
revision plus the hash of its planned slots) and not the actor's, whose fence is
re-read by a query; naming a pre-commit revision would hand the plugin a fence
that can never match.

## Warehouse inventory transfer (design, 2026-09-14)

A warehouse is a bound authored container of a placed settlement structure. The
plugin never names a coordinate: core mints the handle and `bind_warehouse`
(now mc-net `script/storage/settlement_runtime/warehouse.rs`) binds it to a durable
`DurableWarehouseBinding`; reading it is already a canonical owned-inventory
snapshot. Writing it must be one server-owned simulation command,
because a deposit spans the three domains this ADR keeps on one ordered
authority: the world container, the player inventory, and the plugin operation
receipt.

The first write attempt was built on a bare `WorldMutationView` container
mutation and was reverted: it neither advanced the container's `chest_state_ids`
fence nor published `ChestSlots`, so a concurrent menu commit could plan from
the pre-transfer contents. That is a second authority, and it is why the write
path is a designed cutover rather than a container-local fix.

Required shape:

- The composite rides the existing regional chest command as
  `CommitChest` with a server-owned flag, not as a new standalone command: that
  command already owns the mutation view, the container state-id fence, the
  `ChestSlots` publication and the journal, and a standalone variant would
  duplicate enum, `kind`, `command_requires_world`, region, result, response,
  metrics and handler arms for the same composite.
- The command carries the container's position, its expected block entity (27
  slots) and expected `state_id`, the player's expected/updated inventory plan,
  and the **encoded** plugin operation receipt
  (`PreparedStorageBatch::encode_world_inventory`).
- The receipt rides the run's ONE group append: the chest job result must carry
  the stamped after-images plus the encoded batch, so the journal needs a group
  append that takes images plus an optional encoded batch, with the existing
  `record_reserved_snapshot_groups` and `record_reserved_inventory_decision`
  delegating to it (this append **does not exist yet**), because one decision id
  takes exactly one append
  (`crates/mc-net/src/play/world_inventory_journal.rs:48`).
- Decision id ownership is the first implementation decision, not a detail: the
  run allocates its own consecutive ids
  (`crates/mc-net/src/play/simulation/regional_mutation.rs:226`), so either the
  reservation is filtered to envelopes that need a decision and the submitter
  reserves this one, or the response returns the allocated id. The submitter
  cannot write the player recovery after-image or mark the decision projected
  without it.
- Inside the owner turn, in this order: validate the player fence
  (`inventory_recovery_required`, expected inventory, expected carried item) and
  the container fence (the `chest_state_ids` value plus
  `commit_chests_conditionally(expected, updated)`, which rejects a stale
  after-image); stamp the container's chunk for the reserved id and take its
  after-image (`WorldMutationView::stamp_chunks_for_world_journal`,
  `crates/mc-world/src/resident.rs:715`); append the ONE world-journal decision -
  chunk after-image plus the encoded receipt; advance `chest_state_ids` and
  publish `ChestSlots` to the container's viewers; then respond with the decision
  id and outcome.
- The server-owned commit is a second entry point beside
  `ChestTransaction::commit` (**not yet written**): the same composite as the menu
  path without the `actor_has_open_view` fence, with the actor receiving the
  published slots instead of a menu ack. Every other fence - container state id,
  expected inventory, expected carried item - is unchanged.
- The submitter then projects only what the response acknowledges: install the
  decoded plugin storage batch, write the player recovery after-image
  (`PlayerInventoryRecovery::recover`), publish the player's
  `AuthoritativeInventory`, and only then `mark_inventory_projected(id)`.
- A stale fence, an absent container or a rejected chest commit never mutates:
  the command answers a typed refusal and appends the decision with no
  participant, mirroring the stale path of `BlockEdits` (`crates/mc-net/src/play.rs:8540`).

Rejected alternatives:

- Reusing the *menu* semantics of `CommitChest`
  (`crates/mc-net/src/play/session/transactions.rs:87`) unchanged: the menu path
  requires `actor_has_open_view` and fences the player against a menu plan, and a
  plugin deposit has no open menu. The command shape is reused, its
  session-authored semantics are not.
- A new standalone command variant: the same composite as the chest run, but it
  duplicates that run's mutation view, fence, publication, journal and outcome
  plumbing.
- Two decisions - container first, receipt second: a crash between them leaves
  items inside a durable container with an unspent receipt, or a spent receipt
  without the transfer. That split is what the one-decision rule exists to
  prevent.
- Taking a `WorldStorage::mutation_view()` on the script thread: the reverted
  attempt, and a second writer.

The cutover removes the `Warehouse` arm of the endpoint gate in
`crates/mc-net/src/play/session/owned_inventory_endpoint.rs` and routes it to
this command. The reading endpoint stays as it is; there is no second item
ledger and no plugin-supplied coordinate.

As landed, one further rule joins this section: a receipt-bearing `CommitChest`
is admitted only to a journaled regional run
(`command_needs_world_journal`), and the menu path refuses one rather than
committing a container whose receipt has nowhere to go. A refused composite
still appends its reserved decision with no participant, so the run's single
append stays contiguous, and an unstampable container chunk fails the run's
append rather than publishing a receipt recoverable without its container.

For an open player menu, the chest transaction also carries each block's
`BlockMutationToken` captured at opening. The resident CAS compares those tokens
under the region write lock along with the expected slots; the coordinator
fallback compares them under the storage lock. An ABA replacement with identical
slots and state id therefore rejects instead of applying a click to the new
container. The connection closes an invalidated menu, unregisters its lid
viewers, and resynchronizes the player's inventory and cursor; a warehouse
receipt has no player menu token and retains its existing
binding/slot/state-id fences. This is one additional opening fence on the same
container authority, not a writable menu cache.

The append-before-publication order is enforced, not merely structural. The
regional run arms a test-only probe
(`crates/mc-net/src/play/simulation/regional_mutation.rs`) before it appends,
and the observation is taken inside the publication path itself, where the
container's `ChestSlots` command leaves the run
(`crates/mc-net/src/play/session/outbound.rs`): the journal records the append
state at that moment. `server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both`
fails with `[(decision_id, false)]` when the run publishes before appending, and
with no observation at all when the publication does not pass through that path,
so the rule cannot silently regress into publishing a container whose
after-image is not journaled.

## Warehouse inventory transfer, worker principal (landed 2026-09-15)

The same composite serves a deposit whose second participant is not a player
session. A worker's `Haul` work order names a bound warehouse as its
destination; its other half is the worker's own canonical record, which lives in
the plugin ledger and not in the world. Two rules make that reuse safe rather
than a second authority:

- **One container half, one ordering.** `commit_container_half`
  (`crates/mc-net/src/play/session/transactions.rs`) holds the state-id fence,
  the conditional `commit_chests_conditionally`, the state-id bump and the
  `ChestSlots` publication for *both* the menu path and the server-owned path;
  only which actor the publication excludes and how each names a refusal stay
  caller-specific, so the publication order cannot drift between them.
- **The actor is optional, the receipt is not.** `CommitChest` carries
  `actor_session: Option<SessionId>` and `player: Option<Box<ContainerPlayerPlan>>`.
  A deposit with no player participant is enqueued through
  `enqueue_with_fence(None, …)` exactly like every other server-owned command;
  the session-authored menu shape is refused by the validator when it is missing
  its session or its player plan. The session and the player plan travel
  together in every shape the validator admits
  (`actor_session.is_some() == player.is_some()`), so a plan can never move a
  player's items past the state that fences and after-images them; the
  transaction asserts the same invariant against the state it actually holds.
  The second participant is the record change inside the encoded receipt, so the
  container's after-image and the worker's cargo are durable together or not at
  all - the batch's `order` change is applied by the same projection
  (`append_inventory_projection`) the player's recovery after-image uses, and a
  receipt-bearing batch is still admitted only to a journaled regional run.
- **One deposit tail, one ordering.** `InventoryRuntime::commit_prepared_deposit`
  (`crates/mc-net/src/script/storage/world_inventory.rs`) owns everything after a
  caller has prepared its batch: encode the receipt, fence and commit the
  container, project the ledger frame, acknowledge the decision. The player's
  inventory and a worker's record differ only in how their batch was prepared and
  in whether a player participant travels with it, so both callers keep their own
  fencing and planning and share this tail; the alternative - a resident entry
  point that re-implements the sequence - is exactly the drift this section
  exists to prevent.

A worker haul also carries a planning rule worth naming, because it decides
whether a settled storage state can progress at all: the deposit moves what fits
and leaves what does not, slot by slot. A container that cannot take the worker's
first stack but can take a later one deposits the later one rather than refusing
the whole cargo and stalling the job on the same stack every cycle; only a cargo
that fits nowhere answers `capacity`.

One consequence is recorded deliberately: a haul is directed. `Haul { source,
destination }` is no longer reordered by `ScriptResidentWorkOrder::canonicalize`
(which is gone); a directional pair is not a set, and the previous swap turned a
deposit into a withdrawal.

## Remaining migration

The staged boundary is not a claim that all world state has one owner. Current
remaining work is limited to real caller cutovers:

- remove legacy session/world mirrors when their final readers have moved;
- move remaining server-origin block and aggregate mutations behind their
  owning transaction boundary;
- shrink `SimulationCommand`, `SimulationHandle`, and `SimulationOwner` whenever
  a command category has no production callers;
- keep active save entry points ordered while global multi-domain disk commits
  remain non-atomic.

Each cutover removes the old helper, mirror, alias, and test path in the same
change. Solaris has no compatibility obligation to superseded internal APIs.

## Rejected alternatives

- A second mutable entity store or session-owned entity mirror.
- Treating outbound delivery as an authority acknowledgement.
- Retrying an outcome-unknown transaction.
- Polling commands on tick cadence or sleeping for lock availability.
- Holding the global session lock while waiting for a regional owner.
- A generic event bus, service layer, or compatibility shim without two current
  implementations.
- Moving ordinary regional AI or physics back through `SimulationOwner`.

## Consequences

Ordering remains explicit where gameplay state spans domains, while ordinary
entity work scales through regional ownership. The cost is a visible boundary
between authoritative commit and publication plus deterministic protocols for
multi-owner work. That cost is accepted because it prevents duplicated
ownership and ambiguous recovery.
