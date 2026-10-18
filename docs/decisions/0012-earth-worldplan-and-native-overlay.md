# ADR 0012 - Earth WorldPlan, immutable base and native persistent overlays

**Date:** 2026-10-01  
**Status:** Proposed for the current native runtime under ADR 0014; not ported.
W0–W5 implementation and acceptance notes below record the retired v1 prototype.
Its preparation commands, owner/journal topology and candidate-runtime paths
are historical, not a second current backend. ADR 0013 owns native persistence;
this proposal does not authorize an Earth or Anvil serving fallback.
**Scope:** Earth-like spec v3, immutable base and native overlays; future native
adoption requires its own owner decision and acceptance.
[Delta audit and real call graphs](../evidence/worldgen-w0-audit.md).
[W1 definition/publication and operator evidence](../evidence/worldgen-w1.md).
[W2 physical solvers, resource measurements and remaining scope](../evidence/worldgen-w2.md).
[W3 boundary/budget/geology fixtures and real cold-path measurements](../evidence/worldgen-w3.md).
[W4 native state, lifecycle/recovery and cache-purge evidence](../evidence/worldgen-w4.md).
[W5 startup/runtime/spawn/content cutover and acceptance evidence](../evidence/worldgen-w5.md).

## Context

The retained vanilla-like generator revision is 24 and its explicit Anvil path
is unchanged. Earth-like runtime instead consumes a prepared physical plane,
RegionPlans and native effective state. The former Earth branch based on the
legacy landform/drainage samplers is removed; Anvil is not an Earth fallback.
Height geometry remains compact, not a metric-height runtime.

A seed does not recover gameplay history, an older generator, or imported
vanilla terrain. A derived cache must not become a durability mechanism. The
existing resident store, simulation owners, semantic journals, publication and
checkpoint fences remain the authorities; adding a database does not create
an `EarthWorld` or move entities/plugin policy into worldgen.

## Decision

### Three layers, one runtime owner

1. `mc-worldgen` owns an immutable `WorldDefinition` and canonical `WorldPlan`:
   physical domain, algorithm/content/math identity, coarse geology/climate,
   complete modeled basin DAG, water budgets and refinement contracts.
2. It prepares bounded deterministic RegionPlans and base sections through the
   existing admitted CPU/I/O work pipeline. Sampling a ready plan is synchronous
   and performs no IO or global solve. Detail cannot move an ancestor outlet,
   reverse a reach, invent upstream water, or change a lake's spill level.
3. `mc-world` owns durable imported baseline/source coverage, section overlays,
   associated chunk state and composition. The existing `ResidentChunkStore`
   receives ready effective chunks. `WorldMutationView` remains the sole block
   mutation interface; COW/read publications remain views, not authorities.
   `mc-simulation::persistence` owns semantic journal ordering/recovery.
   Existing entity, inventory, clock and plugin authorities are not duplicated.
   `mc-server` owns prepare/bounds/state/startup orchestration; import/export is W6.

```text
Procedural source: immutable identity + root -> prepared region -> base
Imported source: durable chunk-presence descriptor -> native imported baseline
Absent source: immutable explicit missing-chunks policy (deny by default)
base + committed persistent state -> effective -> existing resident publication
```

Imported presence covers the entire source geometry, including omitted/air
sections and a completely empty chunk. Never fill imported air procedurally.
Imported-only + deny requires no root; procedural coverage requires a matching
canonical root and compatible pinned generator. Missing/corrupt committed root,
state DB or imported baseline fails closed. Missing derived caches rebuild.

### Identity and publication

World identity fixes domain/seed, XZ metres per block, vertical mapping,
root/climate/refinement resolution, algorithm/artifact version, content/state
semantics, math profile and canonical root hash. Imported worlds add source
profile/geometry, coverage/baseline catalog identity, provenance and missing
policy. PlayableBounds has a separate operating revision. RAM, cache, workers,
logging and visitation are not generation inputs.

W1 publishes the checked W0 basin root and initial native store through staging,
durability and a last-published manifest under the existing exclusive world lease.
Independent renames are not one transaction. Recovery classifies orphan staging,
orphan prepared artifacts and a committed world; only matching verified orphans
resume. All file/directory barriers, including newly created world ancestors and
recovered artifacts, precede manifest publication. Missing/corrupt committed
root/state fails closed; mismatched definition/artifact refuses opening.

The implemented identity is SHA-256 over a versioned domain separator, the
length-prefixed normalized definition TOML, algorithm-source artifact hash and
canonical root hash. The fixture artifact hashes its basin and definition sources;
content fingerprints pin the embedded required block report and the integer
basin/compact-Y math profile. This is not a deployment/retention policy or a
promise that a future binary can execute a past generator.

`PlayableBounds` is a separate atomic file with a checked operating revision.
Changing bounds never rewrites root/manifest/native state. Offline operations
cannot share the live world lease in either acquisition order. Source admission
retains that lease through contract creation and runtime shutdown. Explicit
native startup shares the prepared store and lease; it never opens Anvil regions.

### W2 physical root and canonical codec

`earth-root-v1` with `w2-f64-canonical-order-v1` prepares a real physical plane
root, not a tiled basin or legacy noise fallback. The ordered pipeline is
tectonics/macro-relief, two fixed climate/runoff/hydrology/erosion passes, then
final climate and hydrology. Persisted catalogs retain plate/crust separation,
boundary polarity and local regimes, inherited provinces and hotspot histories.
Fields retain physical elevations, exact ocean seed IDs, four-season climate
and water/solid/ice budgets. Root and climate grids retain actual partial-cell
areas; conservative overlap remapping does not allocate a per-cell list.

Atmospheric relief is sea surface zero over connected ocean, and actual DEM
height on dry land (including disconnected negative basins). Marine precipitation
goes directly to the ocean water-equivalent budget, not land ET/soil/groundwater
or snow stores. Present ice calves at marine receivers; seabed fines cannot
supply wind transport. Ocean heat edges connect only completely marine climate
cells, so unresolved sub-grid land barriers never become a heat shortcut.

The plane codec is `SOLROOT2`, little-endian payload length, SHA-256 payload
checksum, then canonical ordered JSON with exact f64 round trips. Manifest
schema 2 records the exact bounded payload length. The explicit W0 basin keeps
its 520-byte fixture codec. Identity pins the definition, required block content,
math profile, actual solver source artifact and canonical root bytes; a changed
artifact is not implicitly compatible with an older prepared root.

Admission uses `(root cells + climate cells) × 8192 + 1 MiB` as a conservative
encoded-root bound and four times that for builder/decode residency. CLI
prepare, inspect and bounds operations accept checked scratch/cache budgets;
refusal never changes precision. These are admission bounds, not RSS predictions
or Earth-size performance claims. Lease, manifest-last publication, fsync and
recovery semantics remain the W1 protocol.

### W3 bounded lazy regions and discardable cache

`earth-root-v1` pins 1 m maximum physical detail and uniform nominal quadtree
depth. W5 uses a 3×3 local reconstruction stencil: a live leaf has at most two
half-metre interpolation intervals. The earlier W3 artifact used 33×33. This is
an explicit approximation change, not bit-identical regeneration; the
source-byte artifact/world identity changes and older worlds are not rebased.
Partial patches retain their actual area without introducing adaptive-depth
T-junctions. `GeoNodeId` is root/level/X/Z; world/subsystem/node seeds and shared
edge/corner owners depend on immutable geometry, never first visitation.
Published root rasters hold geological-history, basin and seasonal water-body
ownership, so local queries do not traverse upstream, lake members or catalogs.

Each cold path prepares only its depth+1 ancestors, bounded at depth 30.
Each node queries at most 49 root cells and owns at most 196 reaches plus 49 body
footprints; a reach has 16 segments. Preparation makes 25 column evaluations
and at most 89 water evaluations per node, one reconstruction pass, with a
4 MiB encoded-record limit and an admitted 16 MiB region scratch floor.
These are explicit algorithm/admission bounds, not universal latency SLOs.
Ready sampling is CPU-only. Raw reconstruction is separate from final carving;
geological interfaces and ecology are reconstructed at the requested coordinate,
and the same actual-water ecology rules apply at vertices and between them.

Child local area/runoff/loss/storage and coarse/fine sediment budgets conserve
parent sums. Through-flow remains one root-owned reach, not duplicated child
runoff. Protected corridors and inherited basin/shore constraints cannot be
changed by detail. Root-owned confluence/lake/shore anchors retain compatible
profiles; sediment/depth/coastal-wind proxies infer conservative delta fans or
explicitly static estuaries. Modeled season 3 is the fixed water representative.
Compact bed-only rounding corrections are reported separately in block Y;
they never alter physical water levels, discharge or m³ budgets.

`world prepare-region --at X,Z [--level N] [--cold]` loads the admitted immutable
root and prepares a ready plan, not chunks. The separate, explicitly discardable
`derived-regions.redb` indexed container uses world/node/version keys, payload
SHA-256, FIFO metadata and a logical encoded-byte quota. Invalid derived records
are cache misses; native/state databases cannot be opened as this cache. Cache
eviction, including quota shrink, never touches canonical root or gameplay state.
Database page/MVCC overhead is not the logical quota or a write-amplification
claim. W4/W5 remain responsible for live pipeline admission and materialization.

### Native backend and codec

Use one embedded **redb database per new native world**, not a custom storage
engine or multiple interchangeable backends. W0 pins redb 3.1.0 (MSRV 1.89,
Solaris toolchain 1.94.1); transactions explicitly use `Durability::Immediate`.
There is one serialized writer, bounded off-thread snapshot batches and bounded
DB cache. Runtime source mode is exclusive: no live dual Anvil/native writes.
Anvil remains the explicit legacy backend and import/export codec.

A section key is dimension-qualified signed X/Y/Z; local index is 0..4095.
W0 keys use dimension u32, sign-bit-flipped big-endian X/Z/Y coordinates (the
physical tuple order is dimension/X/Z/Y). Persistent block identity is a
canonical name/properties world dictionary, never a runtime ordinal. Native
schema 2 stores sorted sparse index/palette pairs or palette-packed dense
replacements referring to those IDs. Both alternatives use bounded zlib frames;
the smaller compressed record wins initially. A mode switch requires both
32 bytes and 5% savings. **AllAir is a dense replacement, not absence.**
Dictionary additions and referencing sections commit in the same transaction.
Unknown states/properties, invalid dictionary indexes or corrupt records fail
closed. Schema 1 is explicitly unsupported and requires re-preparation; there
is no released-native compatibility shim or automatic rebase.

Side state uses the existing supported compact chunk metadata codec without
`block_states` or Anvil-relative tick encoding: typed/opaque block entities,
inventories, initialization markers, biomes/light, heightmaps and supported root
extras. Native metadata preserves the owner's `LastUpdate`; it never synthesizes
flush-time metadata or depends on a storage clock argument. Native scheduled work
separately preserves absolute u64 deadlines, priorities and sequence identities,
including overdue/far-future work. Ordinary Anvil serialization retains its
unchanged relative-delay payload and save-time `LastUpdate`.

Entity owners may persist serialized after-images through the same immediate
transaction as chunks. A world-wide entity identity key makes dimension/chunk
migration one replacement. Consumed generated slots and durable tombstones
prevent reactivation and resurrection. These are backing bytes from the existing
owner, not a second live entity or inventory authority. Player/plugin/global
clock authorities are unchanged; W5 wires their existing journal/save barriers.

### Durability ordering (do not strengthen the existing claim silently)

Ordinary Solaris journal records use bounded write-behind: resident changes can
be published before fsync. Receipt-bearing composite operations sync before
publication. W0 does not turn all operations into fsync-before-apply or weaken
that contract. Before any native after-image commit:

```text
existing owner mutation + exact stamped journal after-images
 -> existing shared journal writer flush acknowledgement
 -> one immediate redb transaction: section/side-state participant set + LSNs
 -> database acknowledgement
 -> exact durable-image + revision-fenced resident clean acknowledgement/checkpoint
```

The external WAL and redb are not one transaction. Replay of an older/equal
per-chunk LSN cannot regress a newer snapshot. A failed participant aborts the
whole DB transaction. The backend receipt can acknowledge resident chunks only
through the existing dirty-generation, exact captured Arc and pending-journal
fence. It never truncates external journals; all existing consumers must first
pass the owning checkpoint barrier.

### W4 backend checkpoint and offline composition

The existing `DirtyFlushPlan` and admitted save worker capture native resident
snapshots after the owning journal barrier. Pressure captures cap at 64; a full
checkpoint captures every eligible dirty snapshot and writes bounded batches
of at most 64. Base resolution, comparison, encoding and IO remain outside the
tick. There is no second native writer/runtime owner. Failed commits retain dirty
state. Entity batches cap 256 participants, 16 MiB/encoded record and 64 MiB
aggregate encoded entity state.

Resident acknowledgement retains exact captured-Arc, dirty-generation and
pending-journal fences, so a concurrent mutation survives an older receipt.
Semantic replay remains strictly LSN-monotonic. A generic live-owner checkpoint
may commit a newer captured image with the same/zero semantic LSN after its
barrier, because fire/growth/gravity/container mutations are not all new journal
decisions. A resident LSN older than committed state is rejected; a failed batch
cannot clean residents or partially advance participant state.

Metadata-only native checkpoints reuse the already durable block image only
when its semantic LSN and COW section-storage identity still match the resident
proof. They update side state in the same native transaction without resolving
the procedural base or replacing overlays. A full-image plan invalidates that
proof before commit; only the latest matching full-image acknowledgement can
restore it. Metadata acknowledgements cannot revive an invalidated proof, so a
lost or late receipt cannot hide a same-LSN block mutation or revert. The existing
owner barrier, pending-journal, captured-image and dirty-generation fences remain.
The actual 25-chunk inhabited-time flush fell from 8.49 s to 40–43 ms, with zero
base materializations; [W5 evidence](../evidence/worldgen-w5.md) distinguishes
measured IO from the serialization/transaction/acknowledgement upper bound.

Immutable ready base handles consume the checked basin or W3 leaf water/geology
facts. They sample synchronously without IO, gameplay initialization or solving,
and materialize requested signed compact sections. Plane preparation admits at
most 256 leaf preparations per chunk and retains one ready region in scratch.
The materialization source is pinned by the W4 generator artifact.
Partial physical-domain chunks retain their valid columns without sampling
outside the domain. Padding is explicit non-queryable air; operator state
commands reject coordinates outside immutable physical coverage.

`world state inspect-block` uses a read-only DB under the world lease and explains
base/sparse/dense origin, native state ID and applied LSN. `world state set-block`
composes the latest effective state and commits immediately under the offline
exclusive lease; no live external journal exists in that mode. Breaking a block
also clears its incompatible container/ticks state. Root, imported coverage,
other chunk state and discardable cache remain separate. On an activated plane,
state commands use the same borrowed `EarthBaseSource` as live streaming,
including village blocks, beard adjustments, markers and chest loot. They require
the exact activated resource cache through `--vanilla-data-dir` and refuse a
changed profile; inspection opens state read-only and does not activate entities.
The explicit W0 offline basin remains a fixture, not a live Earth source.

### W5 candidate explicit startup and runtime cutover

The small-reference seed-42 root is entirely ocean and correctly refuses a dry
spawn; this remains a permanent negative regression, not the acceptance world.
The working acceptance definition uses non-authored physical domain ±8,000 km,
250 km macro cells and 1,000 km climate cells. All eight entity-allocation,
shutdown-tail, aggregate-budget, item-registry, pre-activation-base,
spawn-expansion, finite-domain and settlement review defects are fixed with
behavioral receipts in the W5 evidence. Final graphical/check/clippy acceptance
is recorded there separately; no prepared-only receipt is a gameplay pass.

`data.storage_mode` is explicit source selection (`anvil` by default, `native`
opt-in), independent of `data.worldgen_mode`. Supported combinations are
`vanilla_like` + `anvil`, and compact `tellus_like` + `native`. Existing Tellus
Anvil worlds are refused without modification and require the W6 import/export
route; neither source falls back to the other. The owner-selected default is
`vanilla_like` + `anvil`; generic harness templates inherit those configuration
defaults. Earth examples explicitly opt into `tellus_like` + `native`.

An empty native world directory is prepared through the same manifest-last
pipeline before runtime admission. Optional `data.earth_definition` declares the
immutable TOML input. Without it, startup validates the small reference plane
below with the configured seed and records its identity. Existing native worlds
must reopen the committed definition/root/artifact/store without automatic
rebase. Nonempty unprepared worlds are not overwritten. Geometry must remain
the compact -64..320 range; metric height and spherical worlds are not supported.

Preparation identity pins Earth materialization/runtime and the used village
execution code. Unactivated physical-plane worlds permit read-only inspection
but refuse offline mutations until first live activation pins the canonical
village/loot resource profile. Activated offline mutations require that original
profile and use the same base. First activation records canonical resource-byte
SHA-256, deployment/content identities and chosen spawn in `solaris/world.json`.
Reopening refuses mismatches and retains the committed admissible spawn even
after bounds expansion; it never automatically rebases. Native startup admits
stock vanilla village resources. Authored settlement catalogs consume admitted
native natural/site facts through the existing settlement owner; alternate
legacy startup plans and the legacy ore-fill override remain explicitly refused.

`EarthRuntime` is an immutable source provider, not a world authority. Cold loads
run on admitted load workers; effective state publishes through the existing
resident store. Synchronous native misses require source admission. Derived
lighting publication remains clean, so a base-only visit does not checkpoint
procedural sections solely because lighting was computed. Live mutations and
scheduled ticks use the existing resident/COW/dirty-save path; no native load or
save performs a live `.mca` write.

Spawn uses bounded root-index candidates inside playable bounds, then checks
actual RegionPlan land/water facts and final materialized support/headroom.
Failure to find safe land is explicit, not an origin plateau. The existing
village pipeline owns pieces, buildings, beard and loot; Earth supplies read-only
natural terrain/site constraints at the boundary. A site is refused if the
affected footprint leaves physical coverage, crosses water/protected features
or violates the accepted slope bound.

Natural village suitability is memoized per existing plan key. Concurrent cold
loads share one footprint scan through a per-key pending entry; the map lock is
not held during terrain preparation, and unrelated plans remain concurrent.
Ready entries are bounded at 64; pending entries stay pinned only while existing
admitted source workers use them. Failure removes the entry rather than caching
an invented result. The dry/protected/slope predicates and full affected
footprint are unchanged.

Generated inhabitants activate only through the existing entity owner. Native
spawn-slot claims are world-identity-addressed; the slot is retained before
spawn/WAL publication, and consumed slots survive death even before the first
checkpoint. Native checkpoints retain one entity identity after movement and
preserve tombstones. Entity checkpoint metadata and participant rows commit in
one native transaction after the existing owner barrier; this does not create
a new entity authority or claim atomicity with the external WAL.

A generated villager may legitimately retain population and brain state during
its bounded `Dying` lifecycle. Native checkpoint validation accepts that state
until the existing removal deadline; type, brain, lifecycle/deadline and other
participant invariants still apply. It must not reject the whole owner checkpoint
merely because population outlives the transition from `Alive`.

### Exact invariants

- Zero base-only procedural section records from reading/materializing a chunk.
- Zero base changes from cache purge, worker count, request order or playable
  boundary expansion; generated geography is independent of resource settings.
- No cache purge deletes imported baseline, overlay, pending ticks, initialization
  history, tombstones, inventories or any other persistent authority.
- No empty/failed read is treated as a missing overlay or regenerated buildings.
- No worldgen/map/export reader mutates or initializes source gameplay history.
- Block equality alone does not authorize removal of side-state/history records.
- No dense global metre DEM, preallocated record for every potential chunk, IO,
  planet solver or durable commit on the simulation tick.
- One world authority; no duplicate entity/player/plugin state in worldgen/native.
- Existing vanilla-like worlds/revision/Anvil behavior are untouched by W0.

## Fixed reference profiles and resource model

These are reproducible **reference inputs and admission targets**, not successful
Earth-size runtime measurements. W1 validated and estimated physical profiles
without allocating their grids. W2 now prepares the small plane reference and
retains the explicit W0 basin. Keep precision fixed and reject unavailable
resources rather than reducing it. `earth-root-v1` pins the numerical root,
regional reconstruction, compact base geology/water and Earth runtime source.
Exact activated village/decor/loot bytes are additionally pinned at startup.

| Profile | Physical plane domain | Root/climate step | Geometry and purpose |
| --- | --- | --- | --- |
| W0 basin | `[0,80) × [0,80)` m; 5×5 cells | DEM/runoff 16 m; no climate solver | -64..320 compact; lake floor 82/spill 92; sole outlet cell 22; 25 unit runoff |
| Small reference | `[-8192,8192)²` m; 1 m/XZ block | root 256 m (64² cells), climate 1024 m (16²) | compact mapping; not 1:1 Y; W1 checked estimate, W2 preparation workload |
| Earth-area reference | X `[-15970288,15970288)`, Z `[-7985144,7985144)` m; 1 m/XZ block | root 16384 m (1950×975), climate 65536 m (488×244) | compact first; aspect 2, area 510100197605888 m²; not a spherical projection or metric Y |

Both reference inputs fix seed 42. The W0/W1 basin instead has explicit DEM/runoff
inputs and no random draws; seed still participates in identity. It uses the
embedded required block/item reports. W1 freezes its fixture content/math
semantics; W2 pins its physical solver artifact and canonical f64 math profile.
Neither derives generation semantics from CPU/RAM.

Plane climate datum is independent of playable bounds: equator Z=0, north=-Z,
reference radius 6371000 m, no compressed-climate implicit fallback. Compact
mapping policy follows spec §4.6 (sea Y63, amplitudes 224/111, tanh scales
3000/4000 m), not an assertion that W0's explicit local DEM executes that map.
Metric-height and spheres are outside this checkpoint.

W0 executable: 16 MiB DB cache, 16 MiB maximum encoded record, one serial native
writer, one active snapshot/chunk in the writer probe, fixed 32 commits. Current
geometry gives 24 sections. No extra worker pool or sleeper is added. Development
validation uses debug builds, `nice -n 19`, Cargo `-j 4`, worktree-local target.
The measured machine/receipt belongs in the audit, not a universal speed claim.

Small reference admission targets: canonical root pages ≤64 MiB, admitted
builder scratch ≤256 MiB, DB cache ≤16 MiB, combined runtime working set ≤512
MiB before resident client cohorts. Earth reference targets: canonical root
pages ≤256 MiB, scratch ≤2 GiB, DB cache ≤64 MiB, combined root/preparation/native
working set ≤4 GiB before resident clients. These targets are not allocated
arrays or guarantees: exact layout/solver peak must be preflighted and measured;
if they do not fit, report the real resource requirement and seek a target
revision, never silently coarsen the fixed profile.

```text
canonical logical bytes ~ area / root_step² × actual per-cell bytes + catalogs
runtime RAM = resident root pages + active region/base/effective snapshots
            + DB cache + dirty/pinned COW state + queues + admitted scratch
save = canonical root + durable imported baseline + actual gameplay history
     + DB/index/MVCC overhead; derived cache has its own bounded quota
```

At an explicitly hypothetical 64 B/cell, one small root layer is 262144 B;
one Earth reference layer is 121680000 B. These exclude other climate/graph
layers, serialization, solver buffers and indices, and are **not measured RAM**.
The historical 8 MiB/chunk constant is an initial budget estimate, not a size
measurement. Report logical/encoded bytes, DB/file bytes and actual write IO
separately. File growth is not write amplification; retained MVCC and compaction
are not proven by a 32-write fixture.

## Consequences, staged boundary and next owner decisions

W0 adds a DEM-constrained full-basin fixture, native byte-store prototype,
real-journal lifecycle tests and a runnable offline smoke. It does not attach
native to `WorldStorage`, change a style/revision, prepare a planet, or add a
server command/feature flag. Existing Tellus and vanilla-like paths remain until
the explicitly scoped W5 cutover; replaced Tellus-only code is deleted then.

**W1 implemented:** immutable validated WorldDefinition/WorldIdentity; independent
persisted PlayableBounds; checked cell/area/resource planning estimates for the
fixed reference profiles; canonical 520-byte basin root, atomic prepare/load and
manifest-last recovery under the existing lease. Real P01–P04 tests cover two DB
cache budgets, unchanged old coordinates/retained saves after expansion/shrink,
full ancestor discharge outside the playable subset, and definition mismatch
refusal. Real subprocess crashes at five durable publication boundaries recover.
The CLI provides `world prepare`, `world bounds set`, and `world inspect --check`;
inspection opens native state read-only and writes no world data.

**W2 numerical slices implemented:** real tectonics/macro-relief, seasonal
climate and conservative moisture/ocean heat, DEM drainage/depression hierarchy,
runoff/lake loss budgets and coarse/fine/glacial/hillslope/eolian material
operators. The small reference prepares and inspects through the actual CLI.
C04 production metric-mapping comparison and detailed geomorphic catalogs are
not closed by the W2 numerical fixtures; see W2 evidence. W3 adds inferred
delta geometry, lazy constrained plans and ecology, with numerical P05–P07,
geology/seam tests and measured CLI cold paths. W4 adds native persistence,
immutable compact base interfaces and offline effective-state operator commands;
W5 attaches compact native startup, streaming, spawn, villages and existing
mutation/entity owners through the same base/effective composition path.
Native `pregenerate` is explicitly unsupported, not a durable base bake; use
`world prepare-region` for discardable preparation. Metric height and spheres
remain unsupported. Vanilla-like revision 24 and explicit Anvil behavior remain.

Open operating choices, not blockers for the W0 prototype: set acceptance SLOs
for cold/root preparation after fixed-profile measurements; select the pinned
artifact retention/deployment policy for future compatible regeneration; select
never-activated generated inhabitants/loot export policy (§31.4), and explicitly
approve any discovered import category/semantics loss rather than using a blanket
force switch. redb, offline-first export, deny-default import and no automatic
rebase are already specified decisions, not questions to reopen.
