# Earth-like W0 delta audit and prototype evidence

Date: 2026-10-01. Audited worktree baseline:
`4865344edb3733eacd5a942cdd7b548f39a80126` plus its live unstaged baseline;
the spec's historical comparison point is
`9889036ee786dc90d2e44d7aeeb80fa62efe1a5d`. The worktree index captures the
live tree handed to this task; no parent checkout or user world was edited.

Scope: spec v3 §26 W0 and §28's first task, not W1–W8 or spheres.
Decision: [ADR 0012](../decisions/0012-earth-worldplan-and-native-overlay.md).
Raw command/smoke receipts: `.analysis/codex-logs/worldgen-w0/receipt.md`.
Line references below are worktree source at this checkpoint, not guesses from
comments or a graph indexed for a different checkout.

## Discovery and delta against §1

CodeGraph was attempted with `projectPath=/home/kaiserroman/solaris-worldgen`;
the MCP server explicitly reported this worktree has no `.codegraph` index.
The audit therefore used bounded source searches/reads and followed the real
callers and callees. The main checkout's separate graph was not substituted.
The available rust-analyzer returned no references for the private codec
helpers; direct owning-codec reads established their local calls. No codegraph
index was installed or silently initialized.

| Spec §1 row | Live fact, with evidence | Delta / staged action |
| --- | --- | --- |
| Revision | `crates/mc-worldgen/src/lib.rs:103` is still `WORLDGEN_REVISION=24`; startup checks it in `crates/mc-server/src/startup_validation.rs:340-365` | No revision bump for a disconnected fixture. New Earth identity/revision must be separately introduced before W5; vanilla-like revision/behavior stays 24 |
| Landforms | `terrain/overworld/landforms.rs:37-100,132-189` still uses warped fBm continents/erosion/ridges; scale is clamped 0.25..8 at :40 | No causal root in the live generator. W0's explicit DEM→basin fixture is separate; W1/W2 replace only the future Earth path |
| Drainage | `terrain/overworld/drainage.rs:7,218-256`: upstream depth 1, independent cubic hydraulic potential and seeded downhill branch scoring | No full DEM-constrained upstream solve. The fixture computes its entire finite DAG and discharge |
| River profile | `landforms.rs:159-189`: relief suppresses carving; river floor is sea minus channel depth | Fixture lake spill Y92/outlet Y88 is not tied to sea Y63; full reach profiles await W2/W3 |
| Column water plan | `terrain.rs:245-258,1105-1125`: biome/height/top_non_air, no WaterBody/Reach identity, water top based on sea | No live water-plan identity. W0 basin facts precede materialization, not a second live ColumnPlan |
| Ores | `terrain/geological_ores.rs:140-178,190-257`: coordinate/Y anchors and ellipsoid tests; stone/deepslate host check at :249 | Existing host substitution is not tectonic history/lithology. Future Earth placement W2/W3; vanilla-like path is unchanged |
| Geometry | `crates/mc-world/src/chunk.rs:34-43,61-75`: defaults -64..320/24/9 bits, but geometry is already configurable if aligned and height≤511, checked min+height | Spec's fixed-constructor wording needs this nuance. Native codec still has signed-byte section Y (`anvil/chunk_nbt.rs:774-775`). Router also caps rendered terrain at Y300 (`overworld/mod.rs:99-102`). None proves metric height |
| Storage generation→dirty | `storage.rs:1045-1063,1074-1108`; streaming path `mc-net/src/play/chunk_stream/prepare.rs:1001-1025,805` | Confirmed real behavior, not just the comment at storage.rs:181. W0 leaves legacy behavior unchanged; prototype clean reads have no durable gameplay records |
| Resident/view/journal | `resident.rs:36-47,410-424,465-481,484-543,622-642`; `storage.rs:150-188` | Ownership survives extraction. mc-simulation owns journal implementations now; some runtime composition/adapters are still in mc-net under staged ADR 0011 |
| 8 MiB estimate | `storage.rs:61,142-147` seeds budgets; resident admission uses actual chunk memory/profile and published dirty/resident usage (`storage/budget.rs:46`, `storage/read_view.rs:154-170`) | Do not report 8 MiB as measured payload/resident size |
| Region limits | `anvil/region.rs:34-45,359-361,912-921`: 32² slots, 4096 B sectors, 8192 B header, max 255 sectors/record, external high bit rejected | Current max compressed payload = 1044475 B (255×4096−5). Gzip/zlib/raw/LZ4 read ids exist (:57-74); no external .mcc support. Export/import must reject rather than drop |
| Startup | `mc-server/src/startup/terrain.rs:32-68`; `mc-server/src/main.rs:297-337,361-385` | Generator/village/ore composition remains unified. ExistingVanilla receives no generator. Native/root startup is not wired in W0 |
| Mosaic | `mc-worldgen/src/mosaic.rs:90-124` calls production `TerrainGenerator::diagnostic_sample` | Remains current production geography. W0 does not replace it with a pretty alternate global renderer |
| CLI / contract | `mc-server/src/main.rs:80-104,435-450`; `startup_validation.rs:304-365,471-485` | Commands are Operator, Pregenerate and Content. Content import is licensed vanilla assets, not world import. There is no native prepare/import/export CLI yet. Keep fail-closed world contract; distinguish these future operations from pregeneration |

Worldgen-relative paths in the landform/drainage/column/ore rows start at
`crates/mc-worldgen/src/`; world-relative paths at `crates/mc-world/src/`.

## Real live call graphs

### 1. Shape, materialization, install, dirty

```text
mc-server/src/main.rs:297 build_terrain_generator
 -> mc-server/src/startup/terrain.rs:32-68
    TerrainGenerator::try_with_biome_rules + geometry/mode/structures/village/ore
 -> main.rs:327 ensure_world_contract_with_spawn
 -> main.rs:364 WorldStorage::open_with_capacities (exclusive world root lease)
 -> main.rs:381-384 with_generator ONLY for SolarisGenerated

Synchronous read:
mc-world/src/storage.rs:500 get_chunk / :468 get_block
 -> :1070 ensure_chunk -> :1074 ensure_chunk_loaded
 -> :1087 ensure_region -> RegionReader::read_chunk
 -> missing slot: :1105 generator.generate -> :1106 mark_dirty
 -> :1150 insert_chunk -> :1156 admission -> :1157 resident.insert_if_absent
 -> resident.rs:410-424 insert/publish one Arc chunk

Streaming miss:
mc-net/src/play/chunk_stream/prepare.rs:766 generate_fresh_chunk
 -> :1011 resources.acquire_cpu -> :1018 spawn_blocking
 -> :1022 generator.generate -> :1024 mark_dirty
 -> :805 WorldStorage::try_insert_generated_chunk
 -> storage.rs:1054-1063 mark_dirty/admitted insert -> same resident publication

Generator itself:
mc-worldgen/src/terrain.rs:2590 generate
 -> :2603 plan_column -> :1101 cached_diagnostic_sample
 -> terrain/overworld/mod.rs:53-54 sample -> landforms.rs:37 sample
 -> landforms.rs:167 drainage::sample -> drainage.rs:218 downstream/:237 accumulation
 -> terrain.rs:2607 fill_column -> caves/ores :2625-2632
 -> assign_biomes / structures / decorations :2633-2635
 -> :2637 mark_dirty
```

Thus even a generator returning a clean chunk does not make live native mode
correct: both storage installation paths intentionally dirty it. W5 must change
only the new source-mode path, and keep entity initialization/history separate.

### 2. Ordinary simulation mutation → semantic journal

```text
mc-net/src/play.rs:7901 commit_resident_block_edits
 -> :7950-7953 spawn_blocking journal.reserve_decision_ids(1)
 -> :7971-8003 choose existing journaled resident block/fluid/scheduled-tick API
 -> mc-world/src/resident.rs:1610/1654/1694
    apply_*_conditionally_journaled -> source-fenced region mutation/publication
 -> resident.rs:2137 conditional batch, :2345-2377 COW block/LSN after-image
 -> chunk.rs:1465 set_block_and_update, :1340 dirty_generation transition
 -> resident.rs:2532-2534 pending journal fence for exact touched participants
 -> play.rs:8028-8040 capture exact touched after-images; append off-thread
 -> mc-simulation/src/persistence/world_journal.rs:456/477
    validate snapshot LSNs (:489-496), encode images (:502), append reserved run
 -> play.rs:8045 clear matching journal-pending resident fence
```

The last step follows append acceptance, **not necessarily fsync**. Ordinary
WAL is write-behind on the bounded shared `solaris-journal` writer; an actual
flush barrier precedes Anvil write. The `clear_journal_pending` doc comment is
stronger than this ordinary callsite; it is not evidence of sync-before-every-
publication. W0 does not rewrite this established ordering.

Player break/drop also enters the single simulation authority:
`mc-net/src/play/simulation.rs:5395-5411` handles `CommitBlockDrops`, reserves
an ID; `:5609` awaits `append_block_drop_decision` (defined at :5299, append
at :5307); `:5663` clears the exact touched decision. Composite cross-region
operations use `ResidentChunkTransaction::commit_durably`
(`mc-world/src/resident.rs:2625`) and receipt-bearing journal append
(`mc-net/src/play.rs:8219-8225`, `world_journal.rs:542-547`), which explicitly
flushes before returning. These stronger operations must not be downgraded.

### 3. Dirty snapshot → durable Anvil → clean acknowledgement

```text
mc-net/src/server/checkpoint.rs:326 save_all_after_simulation_barrier
 -> simulation.save_barrier :341; captures chunk plan + world/entity watermark
 -> :556-557 shared world journal writer.flush on blocking worker
 -> :585 WorldStorage::plan_dirty_flush_at_tick
 -> mc-world/src/storage/dirty_flush.rs:408-428 dirty snapshot + bounded admission
 -> :191 DirtyFlushPlan::write -> :192-193 journal barrier
 -> :230 encode_dirty_flush_chunk_payload; retain untouched raw slots :209-225
 -> :261 write_unique_region_tmp -> existing Anvil region writer + file sync
 -> checkpoint.rs:648/651 install_dirty_flush[_snapshot]
 -> storage/dirty_flush.rs:532-589 source/region/revision-fenced file installation
 -> checkpoint.rs:668 off-thread DirtyFlushInstall::sync
 -> storage/dirty_flush.rs:105-120 sync parent directory
 -> checkpoint.rs:687 finalize_dirty_flush
 -> mc-world/src/resident.rs:622-640 clear dirty ONLY if pending fence absent,
    dirty_generation unchanged and captured Arc still current
 -> checkpoint.rs:738-740 checkpoint world WAL only after world_flush_clean
```

Pressure flush takes the same plan/write/install/sync/finalize path, without
checkpointing players/entities/world metadata or truncating WAL:
`mc-net/src/server/checkpoint.rs:154-243` and
`mc-net/src/play/chunk_stream/prepare.rs:417-496`. Dirty/pinned entries are not
LRU victims (`storage.rs:1162-1175`). No independent native writer may retire
these existing authority fences merely because its DB transaction succeeded.

### 4. Reopen / recovery

```text
mc-server/src/main.rs:364 open legacy WorldStorage
 -> storage.rs:1116 ensure_region -> anvil RegionReader::open (:1125)
 -> ensure_chunk_loaded :1087-1099 read/decode slot with expected chunk position

mc-net/src/server.rs:2707 JournalWriter::open (shared entity/world writer)
 -> :2749-2755 WorldChunkJournal::open
 -> :2757-2759 install storage journal flush barrier
 -> :2761 decode_pending -> :2773-2777 replay_journal_chunk
 -> mc-world/src/storage.rs:548-565: skip existing >= LSN; replace newer dirty image
 -> ordinary save pipeline persists recovered image before checkpoint
```

`WorldChunkJournal::open` validates/repairs the append framing and holds its
lease (`mc-simulation/src/persistence/world_journal.rs:330-393`). DB transaction
+ WAL cannot be treated as one atomic medium. W4 will store applied LSN with
native state so this same recovery protocol cannot regress an after-image.

### 5. Existing import/export boundary (not a missing graph disguised as ready)

Today `WorldSource::ExistingVanilla` uses main.rs:364 to open an explicit Anvil
world, resolves slots through storage.rs:1087-1099, and has no generator attached
(main.rs:381). That is direct legacy-world reading, **not** native migration.
Existing chunk interchange is `anvil::read_region` (`anvil/region.rs:503`) →
position-checked `chunk_from_payload_with_items_at_position`
(`anvil/chunk_nbt.rs:204` → position-checked NBT decoder at :194); write is
`chunk_to_payload_with_items_at_tick_for_position` (`chunk_nbt.rs:94-113`) →
`write_region_create_new[_mixed]` (`region.rs:607,651`). Existing dirty-flush
writes this same codec. The CLI enum has no world import/export command.
W6/W6I must add complete offline save/metadata orchestration and target gates;
W0 does not claim `.mca` alone is a playable world.

## Implemented W0 proof graph

```text
mc-worldgen/src/earth/basin_fixture.rs:44 BasinFixture::build
 -> fixed DEM + unit runoff -> outlet-seeded deterministic priority flood
 -> parent/flood-rank DAG -> reverse-rank conservative whole-basin accumulation
 -> :87 materialize_chunk (lake spill and edge ports constrain compact blocks)

mc-simulation prototype test / examples/worldgen_w0.rs:35
 -> WorldStorage::commit_chunk_snapshot of clean fixture (:531 storage.rs)
 -> existing WorldMutationView journaled block/fluid API
 -> real WorldChunkJournal record_reserved_snapshot_groups
 -> persistence/native_overlay.rs:23 flush_native_snapshots
 -> journal.writer.flush -> mc-world/src/native.rs:198 apply_snapshots
 -> one immediate redb transaction for all section/metadata/applied-LSN records
 -> drop owner and DB -> NativeStore::open :101 -> compose :283

Read/export:
NativeStore::resolve_base :165 (imported presence before procedural closure)
 -> compose :283 -> existing chunk_to_payload codec -> write_region_create_new
 -> read_region + checked chunk decode; no source activation/dirty record writes

Import prototype:
existing Anvil chunk codec -> normalized compact Chunk
 -> NativeStore::import_chunk :125 (durable immutable full-chunk coverage)
 -> delete fixture source / drop every RAM copy / reopen native DB
 -> resolve_base :165 bypasses generator, including wholly empty imported chunk
```

Native schema 1 is independent of revision 24. Dimension-qualified signed
keys are sparse; no records are preallocated for potential chunks. Section
palettes carry stable names/properties; runtime registry order is not persisted.
Sparse local indices are sorted and bounded; dense all-air is explicit. The
native metadata path shares the existing side-state codec but never encodes
base block payload (`anvil/chunk_nbt.rs:720`, `native/codec.rs:75`). The prototype
is not another resident world owner, nor another live persistence mirror.

## Baseline, resources and proven boundary

The fixed profiles, assumptions, limits and formula are in ADR 0012. W0's
runnable profile is only 80×80 m, 25 cells, 16 m cell step, compact -64..320 Y,
unit fixed-point runoff, lake floor/spill 82/92, outlet discharge 25. The
small/Earth-area profiles are fixed future preparation inputs, not runnable
server presets. At hypothetical 64 B/root cell the Earth-area root layer is
121680000 B; there is no whole-planet allocation or measured speed claim.

Observed environment: Ryzen 5 7535HS, 6 physical cores/12 threads; installed
RAM 32846458880 B; rustc 1.94.1; development/debug builds. Commands use nice 19
and Cargo -j 4; no explicit cgroup CPU/RAM quota. nice is scheduling priority,
not a CPU quota, and -j 4 caps compiler jobs, not test threads. Another agent
may compile concurrently; receipt timings are not uncontended benchmarks.
Prototype DB cache =16 MiB; record cap=16 MiB; serial 32-commit writer cohort.
The executable reports root/materialization/export timing, full journal+native
ack latency/throughput, actual DB/file bytes, sparse key coverage and row counts.
DB file growth is **not** measured device write amplification or peak RSS.
Exact measured results and post-metadata-encoder verification are in the raw
receipt; don't substitute hypothetical cell arithmetic for those numbers.

The instrumented debug executable passed in 0.83 s, maximum RSS 82032 KiB;
root build 41 µs, one base chunk 4356 µs, bounded chunk export roundtrip 8095 µs.
The fixed 32 journal+native acknowledgements measured p50/p95/p99
11957/13078/14055 µs and 81.91 commits/s. DB file after the cohort was 565248 B
(no growth during this cohort), export region 12288 B. GNU time reported 4432
filesystem-output units for the entire smoke, including WAL/database/export;
this is aggregate OS accounting, not per-overlay device write amplification.
Two imported chunks, one overlay section, one metadata chunk and one applied
watermark were durable. These small/debug/concurrently-loaded measurements
establish no Earth-size throughput, no backup recovery and no client gate.

Tests prove: broken block does not resurrect after owner/DB purge; fluid mutation
and pending scheduled tick survive semantic journal recovery/native reopen;
clean procedural reads create zero base/overlay/metadata/applied records;
bounded export includes base plus state without modifying the source; durable
imported base/all-air coverage survives deleted source and RAM/DB reopen. Five
additional storage tests cover dense all-air, transaction rollback, stale LSN,
revert retaining scheduled history, reordered/unknown registry states, wrong
identity/missing database and immutable import presence. Basin tests check every
node reaches the one outlet, DAG rank, exact per-node budgets and voxel water
ports/spill elevation. They do not claim realistic planetary geology or vanilla
fluid parity.

Unimplemented by design: canonical WorldDefinition/hash/root publication;
lazy RegionPlans; shared native dictionary and compressed hysteresis tuning;
full save import/export compatibility/preflight/resume; .mcc; lifecycle migration
of every entity/plugin/tick path; backup/restore/disk-full/crash/compaction stress;
height-capable engine/client; native runtime cutover and client acceptance.
Full project L2/independent integration review remain the parent agent's gate.

## Exactly W1 next

1. Implement validated immutable WorldDefinition/WorldIdentity with explicit
   finite plane domain, physical scale/vertical mapping, precision/math/content
   fingerprints; separate persisted PlayableBounds revision/resource budgets.
2. Implement checked preallocation/resource admission for the **fixed** small
   and Earth-area reference inputs. Reject insufficient budget; never lower
   precision or infer domain from playable bounds/cache/CPU count.
3. Canonically serialize/hash/roundtrip the known basin/lake root. Publish
   staging root + new native store + final manifest atomically under the existing
   world lease, distinguishing orphan prepare from committed identity on recovery.
   Missing/corrupt committed root/store fails closed.
4. Prove spec P01–P04 and prepare/load roundtrip: two cache budgets produce
   identical base/facts (P01); playable expansion/shrink preserves old coordinates
   and saves (P02); outside-playable upstream still contributes the complete
   ancestor discharge (P03); changed extent/root precision fails identity checks
   at startup (P04). No W2 geology, W4 live persistence, planet-size solver run
   or sphere renderer is part of W1.

Owner operating choices remain only where the spec genuinely allows choices:
measured cold/root preparation SLOs and resource target revisions; compatible
pinned generator artifact retention/deployment policy; never-activated generated
inhabitants/loot export policy; explicit acceptance of any actually discovered
import category/behavior loss. Already-set decisions (redb, offline-first,
deny-default missing chunks, no auto-rebase) are not presented as blockers.
