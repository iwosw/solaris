# W4 — native persistent state backend

Date: 2026-10-02. Architecture: [ADR 0012](../decisions/0012-earth-worldplan-and-native-overlay.md).
Predecessor: [W3 RegionPlans](worldgen-w3.md). Exact commands, failures, independent
review, CLI output and measurements: `.analysis/codex-logs/worldgen-w4/receipt.md`.

## Implemented boundary

W4 supplies the native backend, bounded storage-worker checkpoint path, immutable
compact base interfaces and offline effective-state inspection/editing. It does
**not** switch live startup, streaming, spawn, entities or generated-content
activation to native storage. Those remain W5. Existing resident, inventory,
entity, journal and plugin owners remain authoritative; Anvil/vanilla-like runtime
remains unchanged.

Native schema **2** uses one immediate-durability redb database per world. A
world-wide dictionary stores canonical block names and sorted properties; runtime
numeric IDs are never persistent identities. Dictionary additions, hybrid section
records, chunk side state, entity after-images, generated-slot consumption and
participant LSNs commit atomically. Compressed sparse and palette-packed dense
records preserve effective cells; switching requires both 32 bytes and 5% savings.
A fully cleared generated section is an explicit dense AllAir replacement.
Schema 1 is refused explicitly with re-preparation guidance, not silently migrated.

Chunk side state retains inventories, initialization/history, supported opaque
metadata and absolute scheduled block/fluid deadlines, priorities and sequence
identities. Overdue and far-future u64 deadlines survive restart exactly. Native
metadata preserves owner `LastUpdate`, not the time of a flush. Removing block
overrides does not remove non-base inventory/history/ticks. Full Anvil encoding
retains its existing relative tick payload and save-time `LastUpdate` behavior.

Persistent entity backing bytes use a world-wide identity, so dimension/chunk
migration is one replacement. Generated-slot records are consumed once; movement
and durable death tombstones do not permit respawn or duplicate identity. This is
an after-image API for the existing entity owner, not another live entity store.

`WorldStorage::plan_native_dirty_flush` captures at most 64 snapshots and excludes
pending journal images. The existing fair dirty cursor advances on capture.
`NativeStateWriter` runs base resolution, encoding and immediate commit on one
storage worker with one queued batch. Entity input is capped at 256 participants,
16 MiB per encoded record and 64 MiB aggregate. The configured journal barrier
must complete before DB commit. A DB receipt cleans only the captured Arc/dirty
generation with no pending journal fence; concurrent newer edits remain dirty.
Equal-LSN replay can authorize cleaning only if the durable image is identical.
Older resident LSNs and unstamped newer images fail without clearing dirty.
External journal checkpoint/truncation remains the owning persistence barrier.

Ready base handles sample W0 basin or W3 leaf water/geology without IO, activation,
or a solver. Preparation admits at most 256 leaf preparations per chunk and
retains one ready region. Partial physical-domain chunks preserve valid columns;
padding is non-queryable air. The W4 materialization artifact is identity-pinned.

## Behavioral acceptance

All tests below ran against the real native database, not a mocked storage backend.

| Criterion | Exercised proof |
| --- | --- |
| S01 | Basin/plane pure materialization and real-journal clean procedural read/comparison leave imported, overlay, metadata and applied-LSN counts zero. |
| S02, S07 | Break/revert removes equal block overrides but retains emptied chest inventory, consumed initialization and `InhabitedTime`; stale replay cannot restore older blocks. |
| S03 | Dense AllAir replacement survives close/reopen and reordered runtime IDs; generated terrain does not return. |
| S04 | Existing simulation fluid mutation and scheduled work survive real WAL replay, owner/cache purge and native reopen. Generic journaled resident edits and side-state snapshots use the same native path; live fire/growth/gravity/plugin cutover is W5, not claimed here. |
| S05, S06 | Generated entity moves between chunks/dimensions, survives replay/restart as one identity, and dies without losing its slot-consumption record or tombstone. |
| S08 | Metadata/ticks-only dirty snapshots commit without block overlays; overdue/far-future deadlines and sequence identities round-trip exactly. |
| S09 | Sparse/dense/sparse transitions compare every effective cell after reopen and preserve block entities/ticks; hysteresis retains the previous mode for small compressed savings. |
| S10 | Failed participants atomically roll back blocks, metadata, dictionary and watermarks; a real child process exits inside an uncommitted redb transaction; a real kernel EFBIG file-growth failure aborts a 32-entity batch. Failed checkpoint work retains dirty state, pending journal images are not captured, and lost acknowledgements replay safely. **Exact ENOSPC is not exercised**: a private mount namespace is unavailable (uid_map EPERM), and shared volumes were not filled. |
| S11 | Event-synchronized mutation during worker flush leaves the newer revision dirty; the next flush persists it. Equal-LSN nonidentical images cannot receive a clean acknowledgement. |
| S12 | World dictionary and mixed overlays keep canonical names/properties when runtime IDs are reordered; removed/invalid states and disagreeing dictionary indexes fail closed. |
| S13 | Fresh create differs from open of a lost committed DB; schema-1 refusal precedes changed-generator identity validation. Actual CLI missing-store inspection fails without recreating a store. |
| P08 | Real W3 derived-cache deletion, fresh process and immutable-root regeneration retain the block edit and stable native ID/LSN. Read-only inspection leaves native file SHA-256 unchanged and does not recreate the derived cache. |
| P09 | Corrupt overlay/frame/dictionary/tick records and missing committed DB fail explicitly, never return clean base; read-only paths never repair state. |

The first-task linkage required by spec §28 remains exercised through
`mc-simulation::persistence::native_overlay`: real mutation → WAL → native apply →
owner/cache purge → reopen; base+state Anvil codec export; imported baseline and
imported air surviving source removal. This is a bounded codec/journal receipt,
not production client/export/import acceptance.

## Actual CLI surface

Commands ran in distinct actual `mc-server` processes under `nice -n 19`:

```text
world prepare --world-dir DIR --definition small-plane.toml
world prepare-region --world-dir DIR --at=-13,-11 --cold
world state inspect-block --world-dir DIR --at=-13,-60,-11
world state set-block --world-dir DIR --at=-13,-60,-11 --block minecraft:air
# Remove only DIR/derived-regions.redb after process exit.
world state inspect-block --world-dir DIR --at=-13,-60,-11
```

The plane is `[-32,32)²` m, seed 42, root 16 m, climate 32 m, 1 m/XZ block,
compact Y. At chunk `(-1,-1)`, immutable base is `minecraft:basalt[axis=y]`.
The edit commits air as sparse override, native state ID 1, applied/journal LSN 1.
After cache deletion/restart the same base, air override, ID and LSN remain.
Repeated read-only inspection preserves native file hash
`ba9e189a45e4fa2d11a8a21e62806b1e399603d0f9d0b8600ad4a43d8622edc1`.
Temporarily withholding that committed DB produces an explicit restoration error
and no replacement DB. The private fixture DB was restored afterward.

A second plane `[-33,33)²` with playable `[-32,32)²` exercises the actual partial
edge chunk: `(32,-60,0)` can be inspected, edited to gold block and read after
restart; `(33,-60,0)` is explicitly outside physical coverage and refused.

`set-block` is offline under the exclusive world lease, not an external-journal
live mutation. It composes the latest effective chunk and prunes incompatible
container/ticks when changing block type. Inspection reports immutable base,
effective state, base/sparse/dense origin, native state ID and applied LSN.

## Gates, review and measured scale

Build/test commands used `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`,
`CARGO_INCREMENTAL=0`, `nice -n 19` and Cargo `-j 2`:

- `cargo test -p mc-world -p mc-worldgen -p mc-server -j 2`: **972 passed,
  25 existing ignored, zero failed**; complete `test-final.log`.
- `cargo clippy -p mc-world -p mc-worldgen -p mc-server --all-targets -j 2 -- -D warnings`:
  **passed** after removing three unnecessary non-Drop lifetime calls in tests.
- `cargo test -p mc-simulation native_overlay -j 2`: **5 passed**.
- `cargo test -p mc-worldgen materialization -j 2`: **6 passed** after that lint cleanup.
- `cargo run -p mc-simulation --example worldgen_w0 -j 2 -- OUTPUT_DIR`:
  **passed actual WAL/base/edit/replay/export/import smoke**.
- `cargo build -p mc-server --bin mc-server -j 2` and actual CLI scenarios above:
  **passed**. `cargo fmt -p mc-world -p mc-worldgen -p mc-server -p mc-simulation`
  completed before export.

Independent read-only reviewer `WorldgenW4.W4Review` returned **PASS, no remaining
findings**, confidence 0.97. Their equal-LSN false-clean and partial-domain findings
have failing-before/passing-after regressions; the owner-LastUpdate follow-up was
also fixed. They ran no checks; the gates and CLI proof above are the worker's.
The initial native deadline regression (99 becoming 79 at a later flush) and the
initial compilation/lint failures are retained in the receipt, not hidden.

A throwaway compiled Rust probe performed 96 real immediate commits over 32
chunks (single edit, 8×8 cluster, full section), then copied a closed database and
verified every compact-section cell after reopen. Keys include i32 extrema and
±100,000,000 chunks. Final debug/no-symbols measurement on AMD Ryzen 5 7535HS,
Linux 7.0.0-34-generic, approximately 31 GiB RAM:

| Measure | Observed |
| --- | ---: |
| Commit throughput | 103.44 commits/s |
| Commit median / p95 / maximum | 9849 / 12801 / 14072 µs |
| Total timed commit work | 928080 µs |
| Closed DB file bytes | 155648 |
| Retained logical table key/value bytes, including dictionary/index/header | 48416 |
| Linux per-process kernel `write_bytes` during commits | 3198976 |

This tiny debug fixture is not an Earth-scale SLO, a long-run MVCC/compaction
measurement, a physical-device write-amplification ratio or a production backup
protocol. File size, retained logical bytes and kernel writes are deliberately
separate. The independent W0 example also measured 32 commits: 62.41 commits/s,
p50/p95/p99 14913/17916/26899 µs, final DB 159744 bytes; workload differs.
Throwaway probe source/binary were removed after proof; receipts remain private.

## Remaining boundary

Exact ENOSPC, production backup/compaction behavior and Earth-size runtime
performance remain unverified. Runtime/startup/spawn/native-source selection,
all live generated-content activation, metric height, spheres and actual client
acceptance remain outside W4. No production import/export/target-client acceptance
is claimed. W0 operating choices (cold SLOs, pinned artifact retention,
never-activated export policy and explicit import-loss approval) remain open and
do not prevent this backend implementation.
