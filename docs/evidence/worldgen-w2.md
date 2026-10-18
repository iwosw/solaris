# W2 — physical Earth root numerical slices

Date: 2026-10-01. Architecture: [ADR 0012](../decisions/0012-earth-worldplan-and-native-overlay.md).
Predecessor: [W1 publication evidence](worldgen-w1.md).
Local command, failure/correction and measurement receipt:
`.analysis/codex-logs/worldgen-w2/receipt.md`.

## Implemented boundary

`world prepare --definition <TOML>` now builds a real `earth-root-v1` physical
plane root. It does not tile the W0 basin, call Tellus/vanilla noise, prepare
chunks, or enable native live runtime. Without a definition the CLI still
prepares the explicit W0 basin fixture. Existing runtime generation/revision,
Anvil, resident owners and semantic journals are unchanged.

The root pipeline is tectonics/macro-relief → two fixed seasonal
climate/runoff/hydrology/erosion passes → final climate/hydrology validation.
The pass count is artifact-pinned, not determined by time or memory budgets.
Persisted fields include both grids, tectonic catalogs, physical DEM, ocean mask
and exact selected seed IDs, seasonal climate/physical forcing, runoff stocks,
complete drainage/depression hierarchy and both material/ice passes.

- **Tectonics:** connected nonuniform plate partition separate from crust;
  local normal/tangential contact regimes, coherent polarity, canonical boundary
  polylines, continental/oceanic interaction kernels, age/province/lithology
  provenance, inherited orogens/cold-phase footprints and moving hotspot chains.
  Physical fragments have fixed kilometre-scale support; small windows are
  samples of that geography, not continent sizes rescaled to fit the window.
- **Climate:** physical latitude and lapse rate, four seasons, east/north winds,
  conservative finite-volume moisture and mask-only ocean heat transfer with
  explicit open-boundary budgets and 128 CFL-admitted substeps per season.
  Ocean seeds use canonical maximum area×depth oceanic cells, one per submerged
  component; disconnected continental depressions below zero stay dry land.
- **Hydrology:** seasonal snow/soil/groundwater stocks and actual ET; downhill
  DAG with strict rank descent; marginal-cell depression hierarchy, storage,
  fill/spill/merge and shared lake levels; explicit reach/lake evaporation and
  groundwater-export losses. Hierarchy storage is linear in cell count; nested
  level computations still have worst-case quadratic-logarithmic CPU work.
- **Erosion:** material-specific stream-power incision, coarse/fine transport,
  deposition/export/storage balance, conservative talus, existing-loose-fine
  wind transport, inherited glacial episode and separately snow-fed present ice
  with temperature-limited ablation, flux/storage and marine calving. The model
  year is 365 days. Inherited ice is not a claim of current snow cover.

Atmospheric height is zero over ocean, not bathymetry; negative dry-land heights
remain physical heights. Root/climate overlap preserves actual partial areas
and precipitation integrals. Marine precipitation enters the ocean as full
water-equivalent volume, bypassing all land ET/soil/groundwater/snow stores;
marine evaporation belongs to the atmosphere's external ocean reservoir.
Present grounded ice cannot originate from marine snowfall; seabed fines cannot
be a wind source, although land-sourced fines can deposit in ocean cells.
Only completely marine climate cells receive ocean heat edges, preventing an
unresolved sub-grid land barrier from becoming a shortcut. Mixed-cell local
marine refinement remains outside this numerical root stage.

## Identity, codec and resource admission

The supported plane profile is `earth-root-v1`, compact vertical profile,
`w2-f64-canonical-order-v1`, and pinned required-block content. Source artifact
hashing covers the fixture/definition/grid and all physical solver/root sources.
World identity retains the W1 immutable definition + artifact + canonical-root
contract; playable bounds remain a separate operating revision.

The plane root is `SOLROOT2` (8 bytes), little-endian JSON payload length
(8 bytes), SHA-256 payload checksum (32 bytes), then ordered canonical JSON.
`serde_json` float round-trip decoding preserves f64 values exactly. Manifest
schema 2 pins actual root file length; loading checks bounds/hash/definition,
all solver validators and erosion/ocean provenance. The W0 basin retains its
520-byte fixture encoding. W1 lease, fsync, staging and manifest-last recovery
remain in place; older artifact identities are not silently accepted.

Admission reports `(root cells + climate cells) × 8192 + 1 MiB` as encoded-root
bound, and four times that for builder/decode residency. This is a conservative
admission bound, not a measured RSS prediction. Checked `--scratch-mib` and
`--native-cache-mib` work for prepare, inspect and bounds operations. Refusal
never coarsens resolution; MiB arithmetic overflow fails before world creation.
The Earth-area reference was **not prepared**: its estimated requirement exceeds
the original W0 target. No Earth-size timing/memory or runtime SLO is claimed.

## Fixture and gate evidence

Final source passed `cargo test -p mc-worldgen -p mc-world -p mc-server -j 2`
under `nice -n 19`, debug/test symbols disabled and incremental compilation off:
905 tests passed, 25 existing tests ignored, no failures. The final strict gate
`cargo clippy -p mc-worldgen -p mc-world -p mc-server --all-targets -j 2 -- -D warnings`
passed. A preceding scoped Earth run exercised 68 tests, all passing. Logs retain
initial failures rather than presenting only a final green result.

| Fixtures | Real exercised behavior / evidence |
| --- | --- |
| T01–T10 | Collision vs subduction, correct trench/arc sides and polarity, ridge age, rift/graben, transform/bend/mirror, inherited orogen, moving hotspot age chain, exact DEM/material ID-permutation invariance. CSV/profile artifacts. |
| C01–C03, C05 | Ridge rain shadow and wind reversal; 6°C/km physical lapse including dry subsea land; sea-bathymetry invariance; mask-only warm transport and strait closure; moisture/ocean-heat budgets. |
| C04 | Physical/API isolation and compact-vs-metric-reference CSV only. Production metric vertical mapping is unsupported, so the full production comparison is **not closed**. No identical-input echo test substitutes for it. |
| H01–H10 | Actual DEM trace and additive confluences; wet/spilling, dry subsea and nested lakes; explained reach losses; seasonal snow/runoff; flat DAG/flat water surface; elevated lake outlet; mountain-to-sea profile. |
| H11 | Explicit caller-selected distributary fractions conserve one source Q. `H11_delta.csv`; no inferred production delta geometry/catalog is claimed. |
| E01–E03 | Hard/soft response, coarse/fine balances, broad inherited U vs ordinary V valley, present snow-fed ice/ablation, wind and talus budgets; marine runoff/ice/wind regressions. CSV/balance/profile SVGs. |
| P01–P04 | Existing W1 identity/cache/bounds/history/ancestor/mismatch tests still pass; actual plane publication also compares multiple budgets and changed operating bounds. |

The root integration cohort uses 16 fixed seeds (including both i64 extrema),
64 root/16 climate cells per seed; it checks every downstream trace and exact
plan/byte round trips. Partial cells and noncommensurate root/climate steps have
an area/precipitation conservation fixture. Generated evidence lives under
`.analysis/codex-logs/worldgen-w2/fixtures/` and includes cohort root maps,
T/C/H/E CSVs, budgets and physical profiles. These small numerical fixtures are
not varied-seed real-client terrain exploration.

## Actual CLI measurements

Final debug binary, seed 42, physical `[-8192,8192)²` m, root 256 m (4096 cells),
climate 1024 m (256 cells), equator Z=0, radius 6371000 m, 1 m/XZ block,
compact profile, 16 MiB native cache and default 256 MiB scratch:

| Operation | Exit | Elapsed | Maximum RSS |
| --- | ---: | ---: | ---: |
| Fresh `world prepare` | 0 | 3.002 s | 62000 KiB |
| `world inspect --check` | 0 | 0.809 s | 51420 KiB |

Root file: **7022272 bytes**; native DB file: **1056768 bytes**. All four native
record counts are zero. Seed 42's small window is entirely ocean, physical DEM
−4558.057..−4359.829 m: the emitted map honestly shows that, rather than an
invented shoreline. `cli-root-map.svg/.csv` and `cli-water-balance.json` derive
from the actually prepared root. The four combined seasonal water residuals are
−2.8312e−7, −2.7195e−7, −2.8312e−7 and +2.2352e−7 m³. The probe also checked the
canonical header/checksum, every DAG trace, both erosion-pass solid balances and
atmospheric budgets. Inspection preserved hashes of root, manifest, native DB,
prepared descriptor and playable bounds (lease excluded).

A 128²-root/32²-climate plane requires 574619648 admitted builder/decode bytes:
default prepare and inspect refused; `--scratch-mib 1024` prepared in 16.611 s
(119864 KiB peak RSS), then inspected in 3.978 s (91616 KiB). Bounds shrink with
the same admitted budget advanced operating revision to 2 while preserving root,
manifest, prepared descriptor, world identity and all zero native logical counts.
The existing writable bounds opener changed redb's physical file hash; this is
not evidence of unchanged DB bytes. Read-only inspection did preserve those
bytes. A maximal-u64 cache-MiB request failed overflow without creating a world.
File bytes/RSS are not write amplification or gameplay-save efficiency measures.

Independent read-only review found and verified fixes for whole-contact regime
cancellation, missing reader/bounds budget flags and north/Z wind direction.
Subsequent marine air/rain/runoff/ice/wind corrections have real regression
fixtures and were independently source-reviewed without additional blockers.

## Exact remaining scope and owner-open choices

This closes the implemented numerical slices, **not every interpretation of the
full W2 row**. Remaining: full C04 production mapping comparison; automatic
sediment/coast-regime delta discovery and persisted branch geometry; inferred
river-corridor/fan/terrace/dune, cirque/moraine/overdeepening/fjord catalogs and
fine constrained region geometry. H11's explicit conservative split is not a
production delta generator. Reservoirs begin dry and run one modeled year;
periodic hydrological/climate equilibrium and reconstructed paleo climate are
not claimed. Two fixed passes are measured here, not a universal convergence
or geography-quality guarantee. W3 refinement/ecology, W4 live native mutation
persistence, W5 startup/runtime/spawn cutover, metric height and spheres remain
outside this delivery.

Owner-open W0 choices stay open: cold/root acceptance SLOs after fixed-profile
measurements; pinned artifact retention/deployment policy; never-activated
inhabitants/loot export policy (§31.4); explicit approval of any discovered import
category/semantics loss. No blanket force switch or silent choice is introduced.
