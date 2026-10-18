# W3 — bounded lazy Earth RegionPlans

Date: 2026-10-01. Architecture: [ADR 0012](../decisions/0012-earth-worldplan-and-native-overlay.md).
Predecessor: [W2 physical roots](worldgen-w2.md). Exact commands, initial failures,
review corrections and measurements: `.analysis/codex-logs/worldgen-w3/receipt.md`;
raw CLI results: `cli-results.json`, `cli-summary.json`, `cli-*.time` in that directory.

## Implemented boundary

The offline Earth track now prepares immutable `RegionPlan`s from a checked
physical `PlanePlan`. `world prepare-region --world-dir PATH --at X,Z [--level N]
[--cold]` loads the admitted root, prepares the requested ancestor path and emits
a ready natural-column sample, extensive local budgets and profiling counters.
It does **not** prepare chunks, activate inhabitants, write gameplay state, enable
native live runtime, or change the separate vanilla-like generator/revision.

The artifact-pinned `earth-root-v1` refinement is a physical-datum quadtree with
1 m maximum detail, 33×33 reconstruction vertices and maximum admitted depth 30.
Partial cells keep their actual dimensions/area and use the same nominal depth
as other root patches. Root/level/X/Z addresses, subsystem seeds and incident
edge/corner owners are independent of visitation, cache size and preparation
order. Fractional coordinate roundoff is admitted without relaxing candidate
bounds; collapsed floating-point refinement and invalid addresses fail explicitly.

These measurements describe the original W3 artifact. The later
[W5 runtime optimization](worldgen-w5.md) uses a 3×3 reconstruction stencil while
retaining the 1 m physical-detail limit and conservation/boundary contracts; it
has a new immutable artifact/world identity, not an automatic rebase of W3 worlds.

Canonical contracts retain physical reference heights/gradients, ordered
lithological interfaces, basin labels, body/reach IDs, water profiles and
crossings. Detail is constrained by inherited divides, shores and protected
valleys/corridors. Local area, seasonal runoff/loss/storage and coarse/fine
sediment budgets divide conservatively among children; a through-going reach
is one inherited flow fact, not newly generated water in each child.

Root publication adds geological-history, basin and seasonal connected-water
owner rasters, with canonical-load provenance checks. Regions query these facts
locally: no geological-catalog scan, whole lake footprint or upstream traversal.
Five geological bands represent basement, lower/upper cover, unconsolidated
sediment and soil, with ten logical material classes (seven consolidated rocks,
two loose sediment classes and soil). Fold/fault/history/deposit conditions
control interfaces, permeability, solubility and erosion response. Ecology
includes physical seasonal climate, moisture/snow/deficit, inundation, slope,
soil/rock exposure and shore/material context. Ready sampling reconstructs these
fields at the requested point, retains categorical IDs, and applies the same
actual-water ecology rules at vertices and between vertices.

### Water geometry and cache

River geometry uses shared root confluence, lake and shore anchors and
nonincreasing water profiles. Piecewise-linear meanders are checked over their
full arcs against routed corridors and additional incision cost; protected
carving runs once, not once during preparation and again during sampling.
Width/depth depend on Q, slope, material and physical confinement, not a fixed
20–40-block range. Valley/floodplain carving follows the reach rather than an
independent wet noise stripe.

W2's deferred delta inference is implemented: sediment transport, receiving
basin depth and coastal wind/reworking proxies select a conservative three-branch
fan or an explicitly static estuary. Distributaries share one split, seasonal
fractions sum to one, and arcs/bank carving remain within receiving-body and
sediment-supported bounds. Disconnected seas have distinct owners; connected
merged lake cells share one owner/level even across nested depression references.
Modeled season index 3 is the fixed water representative. Earlier-wet/final-dry
reaches/lakes do not materialize current water. Compact wet-depth corrections
lower the rendered bed only; `bed_quantization_residual_y` records the correction
separately from unchanged physical water levels, Q and m³ budgets.

`derived-regions.redb` is a separate explicitly discardable indexed container,
not gameplay/native state. Keys include world identity, schema and node;
records carry SHA-256/version checks and bounded shape. FIFO indexes and a logical
encoded-byte quota bound retained records; reducing the quota can clear only
these derived records. Corrupt derived bytes become a miss; unmarked/native
containers are refused. The current 4 MiB record limit and 16 MiB region scratch
floor are admission bounds, not RSS predictions. Logical quota does not include
redb page/MVCC overhead or imply measured write amplification.

## Numerical acceptance and review

Final affected-crate command, under `nice -n 19`, Cargo `-j 2`, debug/test symbols
disabled and incremental compilation off:

- `cargo test -p mc-worldgen -p mc-server -j 2`: **639 passed, 10 existing ignored,
  zero failed**; `tests-delivery.log`.
- `cargo clippy -p mc-worldgen -p mc-server --all-targets -j 2 -- -D warnings`:
  **passed**; `clippy-delivery.log`.
- Targeted regional gate exercised **33 passing tests**; `region-final.log`.
- `cargo fmt -p mc-worldgen -p mc-server` completed after final source edits.

| Fixture | Real numerical evidence |
| --- | --- |
| P05 | Seven-node cold fixture, physical leaf size, exact ancestor/node/point/candidate bounds; changing a remote never-visited upstream source leaves the prepared leaf and ready sample unchanged. Actual CLI paths below prepare 9 and 19 ancestors. |
| P06 | Four siblings prepared forward/reverse produce identical plans, canonical shared edges and four-way corners; cross-root contracts and ready water/ground/geology samples match. The fixture contains real flowing reaches. |
| P07 losing reach | Real root solver loses 100/50/0/100 m³ to evaporation and 200/100/0/150 m³ to groundwater export; child and grandchild extensive water/solid component sums equal the parent. External flow + losses closes against all local input. |
| P07 closed lake | 100 m³/season fills storage to 100/200/300/400 m³ without an outlet; four children inherit 25/50/75/100 m³ stocks each. Positive shallow physical water remains represented after compact rounding, with bed correction −1 Y reported separately and no physical source change. |
| Geology/ecology | Ordered ages/interfaces and root-knot seams; >5 m transverse fold response, ±800 m fault exposure, deposit-permeability/moisture response, drought/snow/lapse/partial-cell/shore effects and provenance rejection. |
| River/delta | Exact mountain/confluence/shore levels, full-arc/cost checks, Q/slope/material width response including >1000 m physical width, conservative inferred fans versus deep/reworked/sediment-poor non-deltas, dry/final-dry and nested-lake ownership. |
| Cache/sample regressions | Eviction/reopen/hash/version/native-container refusal and reconstruction equality; fractional macro seams; no double valley incision or nearest-coordinate soil mismatch; identical perennial reach ecology on/off the reconstruction lattice. |
| Published-root cohort | Six fixed seeds including both i64 extrema; real 64-root-cell physical plans prepare coastal/inland/edge patches without uphill water profiles or invalid represented depth. |

Independent read-only reviewer `WorldgenW3.W3Reviewer` found the double-incision,
nearest-coordinate ecology/geology and fractional-span errors, then a water
normalization inconsistency. Numerical regressions failed before the first two
fixes and passed afterward. The same reviewer closed every finding and separately
confirmed quantization accounting; no reviewer edits/builds/tests occurred.
`review.json` and the receipt retain failures rather than presenting only green gates.

## Actual CLI cold-path measurements

Freshly published worlds, final debug binary; `/usr/bin/time -v`, `nice -n 19`.
Default scratch 256 MiB, native/cache RAM 16 MiB, derived logical quota 32 MiB.
The root is loaded/validated before the **derived-cache-cold** ancestor path;
root-load and total-process measurements are disclosed separately.

- Small: seed 42, `[-8192,8192)²` m, root 256 m (4096 cells), climate 1024 m
  (256 cells), 1 m/XZ block, compact Y. Point `(-5,-7)`, root 2015, level 8,
  node X/Z 251/249. The sample is honestly ocean at −4459.484180112179 m.
- Wide: seed 0, `[-1000000,1000000)²` m, root 250000 m (64 cells), climate
  1000000 m (4 cells), 1 m/XZ block, compact Y. Point `(-125000,-125000)`,
  root 27, level 18, node X/Z 131072/131072. Root elevations
  −6226.213293509282..1082.7952494607393 m; ocean area 1187500000000 m².
  This sampled inland point is dry, not an invented river view.

| Operation | Exit | Wall time | Peak RSS |
| --- | ---: | ---: | ---: |
| Small fresh root prepare | 0 | 2.95 s | 65968 KiB |
| Small root-load + cold region + cache insertion | 0 | 1.20 s | 66052 KiB |
| Small cached region | 0 | 0.97 s | 60536 KiB |
| Small cold region, 1 MiB native RAM / derived quota | 0 | 1.06 s | 60868 KiB |
| Small read-only inspect | 0 | 0.82 s | 55244 KiB |
| Wide fresh root prepare | 0 | 0.34 s | 48892 KiB |
| Wide root-load + cold region + cache insertion | 0 | 0.70 s | 57828 KiB |

Small cold: **9 nodes**, 11025 column evaluations, 291 root candidates,
0 upstream traversals; loaded-root preparation **134142 µs**, root load
825719 µs. Wide cold: **19 nodes**, 23275 column evaluations, 497 candidates,
116 aggregate features, 0 upstream traversals; preparation **291046 µs**, root
load 176790 µs. Each path stayed within its declared bounds. These measurements
are fixed-profile evidence, not owner-approved universal cold/root SLOs or
Earth-size runtime/client evidence.

Small/wide roots encode 8135340/145453 bytes. Derived leaf records encode
3018896/2997241 bytes, below 4 MiB. Observed derived-container file size is
9445376 bytes in both worlds; small native DB is 1056768 bytes. The 1 MiB logical
quota retains zero oversized leaf records without changing geography. Cached,
cold and differently budgeted samples/local budgets are identical. SHA-256 of
canonical root, manifest, native DB, prepared descriptor and playable bounds is
unchanged after all region/cache/inspect operations; all four native logical
record counts remain zero. Scratch 0 and an exclusive-domain-edge coordinate
are refused with exit 1 and no durable data change. Lease bytes are excluded.

## Remaining track boundary

W3 closes the offline lazy-region row, including W2-deferred delta inference.
It does not close C04's production metric mapping, extended geomorphic catalogs,
periodic climate/hydrological equilibrium, live native durability, base-section
rasterization, scheduled Minecraft-fluid behavior, runtime cutover, import/export,
metric height or spheres. In particular, physical m³ budgets are not a claim of
exact compact rendered block-water volume. W4/W5/W7 own those later gates.

W0's owner-open cold/root SLOs, artifact retention/deployment, never-activated
inhabitants/loot export policy and accepted import losses remain open; none was
silently decided or blocks this bounded offline stage.
