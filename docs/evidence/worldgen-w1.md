# Earth-like W1 definition, identity and atomic preparation

Date: 2026-10-01. Implements spec v3 §26 W1, building on the historical
[W0 audit](worldgen-w0-audit.md) and [ADR 0012](../decisions/0012-earth-worldplan-and-native-overlay.md).
Raw receipts: `.analysis/codex-logs/worldgen-w1/receipt.md` and adjacent
`*-result.json` files. This is offline canonical preparation, **not** the W5
native gameplay/runtime cutover or an Earth-size solver measurement.

## Implemented contract

`mc-worldgen::earth::definition` owns validated immutable `WorldDefinition`.
Its normalized inputs are topology, finite physical bounds, horizontal m/block,
macro/climate cell metres, refinement profile ID, compact vertical profile,
seed, content/math fingerprints and independent climate datum/radius.

Validation rejects unordered/empty bounds, checked i64 area overflow, coordinates
outside i32 or the supported ±30,000,000 compact-client/physics envelope,
nonpositive/nonfinite scales, nonfinite datum, overflow/underflow in physical
area/cell/byte estimates, and invalid profile/fingerprint strings. Only plane
and compact height are supported; metric and sphere are rejected, not clamped.
The compact basin uses the existing -64..320 geometry and its explicit local Y
DEM; it does not claim a metric-Y or global tanh-mapped planet.

Physical domains can have partial cells. Estimates ceil the cell counts and
print the final partial-edge metre widths together with the actual unmodified
physical area. Playable edges must be chunk-aligned and contained in the domain.
No artificial 262144-block extent ceiling or whole-planet allocation was added.

`WorldIdentity` hashes normalized definition bytes, the fixture algorithm-source
artifact ID and canonical root hash using SHA-256 and a versioned domain
separator. Negative-zero climate datum is normalized. The algorithm ID pins
`basin_fixture.rs` and `definition.rs`; content pins the embedded required block
report, and math pins the integer-basin compact-Y profile. This is not a promise
of retaining an old executable. Native DB cache/scratch budgets, operating bounds,
logging and worker count are not definition or identity inputs.

The known W0 basin remains the W1 canonical payload: fixed-width big-endian
`SBASIN01` encoding, 520 bytes, 25 cells at 16 m, lake floor/spill Y82/Y92,
outlet cell 22 and discharge 25. Decode validates exact magic/version/length,
geometry, elevations, unique ranks, neighbour/downstream foreign keys, strict DAG
ordering and conservative per-node/outlet budgets. It is stored as canonical
base, never as a cache. No W2 tectonic/climate solver or W3 RegionPlan is claimed.

Arbitrary small/Earth-area definitions can be validated and estimated. W1
`prepare` explicitly accepts only the known 80×80 m `w0-basin` profile, with its
pinned content/math/compact semantics. Other physical grids require W2; silently
filling them with repeated fixtures or legacy noise would not implement the spec.
The seed participates in identity although this explicit fixture makes no draws.

## Publication, recovery and operating bounds

`mc-worldgen::earth::prepared` orchestrates the existing `mc-world` world lease,
canonical payload and real W0 redb store. The lease records whether it is
shareable: offline exclusivity is enforced in both acquisition orders, while
legacy startup can retain/share its own runtime lease through storage admission.

```text
world-dir/
  .solaris-world.lock       existing lease metadata, not world data
  .world-prepare/           unpublished staging, while present
  world-artifacts/
    prepared.toml          immutable prepared descriptor
    root.basin             immutable canonical root
    state.redb             native gameplay-state database, not a cache
    playable.toml          bounds + checked operating_revision
  world.manifest.toml      final publication/commit marker
```

Preparation validates configuration/budgets before allocation/world creation,
durably creates the world directory and its ancestor entries, writes a matching
staging descriptor/root/native store/bounds, checks root invariants/hash and store
identity, syncs every file and directory, then publishes the manifest **last**.
The manifest binds definition, root hash, algorithm ID and native-store identity.
Two renames are not treated as one transaction.

Recovery distinguishes unprepared, orphan staging, orphan prepared artifacts
and committed manifest. Only verified matching orphan identity/bounds resume;
an unpublished store containing records is refused. Recovered files receive
fresh fsync barriers before commit, including a fully written but previously
unflushed root. Incomplete/corrupt/unidentified artifacts are refused for manual
recovery; no existing gameplay state/root is regenerated or replaced.

Committed load refuses changed definition/artifact identity. Missing/corrupt
root, native DB or operating metadata fails closed with restore guidance rather
than creating an empty world. `world bounds set` atomically persists the next
nonzero revision without rewriting identity/root/state. Shrinking retains saves
outside the current playable subset.

`world inspect --check` uses redb's read-only open, with no repair or world-data
writes; only the required existing lease metadata can change. An unclean DB that
requires repair cannot be certified by this read-only check. An unprepared
`--check --definition` estimates support/preparation needs without creating even
the target directory. Failed reads never become empty overlays.

## Operator commands

The commands use explicit `--world-dir`/`--definition`, independently of the
server's `--config`; there is no second command alias for these operations.

```sh
mc-server world prepare --world-dir world-basin --seed 42 \
  --from 32,32 --to 48,48 --native-cache-mib 16 --scratch-mib 256
mc-server world inspect --world-dir world-basin --check
mc-server world bounds set --world-dir world-basin --from 0,0 --to 80,80
mc-server world bounds set --world-dir world-basin --from 0,0 --to 16,16
mc-server world inspect --world-dir world-basin \
  --definition expected-definition.toml --check
```

`--from` is inclusive, `--to` exclusive. Prepare defaults to the explicit W0
basin when no definition file is provided; its default seed is 42. Prepare on an
already committed matching world is idempotent and does not reset operating
bounds; use `bounds set` to change them. Native cache and scratch admission are
separate settings and cannot silently lower physical precision.

A definition file is bare `DefinitionInput` TOML: scalar keys `topology`,
`horizontal_meters_per_block`, `macro_cell_m`, `climate_cell_m`,
`refinement_profile`, `vertical_profile`, `seed`, `content_fingerprint`,
`math_fingerprint`, `equator_z_m`, `climate_radius_m`, followed by `[domain]`
with `min_x`, `max_x_exclusive`, `min_z`, `max_z_exclusive`. Inspection prints the
normalized values. The entire prepared manifest is not a definition input.

Normal server startup validates/refuses W1 native worlds as offline-only before
legacy generation. It retains a world lease across final native/legacy admission,
legacy contract creation and region/storage opening, preventing preparation from
racing that decision. `pregenerate` stays legacy Anvil-only and refuses native
worlds; it is not reinterpreted as native base persistence or cache warming.

## Exercised evidence

- P01: two actual redb cache budgets (2 and 16 MiB), same identity/canonical root,
  same complete root facts and all 25 materialized base NBT byte payloads; zero
  native base/gameplay records from those procedural reads.
- P02: concrete resident journal-stamped edit persisted through the real native
  snapshot API; expansion/shrink advances revisions 1→2→3, with unchanged old
  base bytes, retained row counts and edited block/LSN after native reopen.
  This is not a new live-journal durability claim; W0 covers that separate bridge.
- P03: only the outlet chunk is playable/materialized; all 25 ancestor runoff
  units still feed its discharge and water geometry, with no upstream saves.
- P04: changed extent, macro resolution, seed or horizontal scale refuses the
  actual committed-load path. CLI `inspect --check --definition` also rejects
  changed extent and resolution.
- Real process kill/recovery at descriptor, root, store, prepared-artifact and
  committed-manifest boundaries; lease refusal while child is alive, matching
  resume after crash, and mismatched orphan identity refusal.
- Canonical lake/root/store roundtrip; missing/corrupt root/store/bounds refusal;
  read-only inspection byte preservation; symmetric offline/runtime lease tests.
- Real `mc-server` CLI smoke: prepare, check, expand/shrink, expected-definition
  roundtrip, mismatch rejection, no-write Earth-area preflight, normal startup
  and pregenerate native refusal, and missing-root fail-closed behavior.

Touched-crate tests passed: **851 passed, 25 existing ignored, 0 failed** across
`mc-world`, `mc-worldgen` and `mc-server`. Strict touched-crate all-targets Clippy
and `cargo fmt --all` passed. Debug profile was used with compiler symbols and
incremental disabled after the first link attempt hit shared-filesystem ENOSPC;
no release build was used. Full L2, scale/harness/client gates and physical
power-loss injection were not run here; integration owns those gates.

Reference estimates: small root 4096 cells/climate 256; Earth-area root
1950×975 = 1901250 cells/climate 488×244 = 119072; actual Earth-reference area
510100197605888 m². Root partial edge widths are X8160/Z12272 m. The reported
64 bytes per root/climate cell and 4× scratch are explicitly hypothetical
planning estimates, not measured W2 peak RAM or disk requirements.

Independent read-only review found four issues; corrected asymmetric lease
sharing, the startup/admission race, missing world-directory ancestor durability,
and the recovered-root fsync barrier. Final touched-crate tests, Clippy and CLI
smoke exercise the corrected implementation. No repeat review was claimed.

## Owner-open decisions retained

No policy was silently chosen for cold/root preparation SLOs or resource target
revisions; compatible generator artifact retention/deployment; never-activated
generated inhabitants/loot export; or acceptance of discovered import
category/semantics losses. redb, offline-first export, deny-default missing
chunks and no automatic rebase remain the already-set architectural decisions.
