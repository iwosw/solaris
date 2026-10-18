# W5 compact native Earth cutover — incomplete checkpoint

**Disposition:** W5 is not closed. The user requested wrapping the current step and returning the checkpoint; this document distinguishes implemented wiring and exercised proof from unresolved acceptance. Do not merge this as a completed runtime cutover.

Worktree: `/home/kaiserroman/solaris-w5`. Receipts: `.analysis/codex-logs/worldgen-w5/`. Architecture: [ADR 0012](../decisions/0012-earth-worldplan-and-native-overlay.md). The spec's W5 stage is startup/runtime/spawn/village cutover and cleanup, with new compact worlds working, old worlds untouched, and one base/effective path.

## Implemented candidate

- Explicit `data.storage_mode`: default `anvil`, native opt-in. Owner-selected `worldgen_mode` default is `vanilla_like`. `tellus_like` requires native and compact geometry; old Tellus Anvil worlds are refused unchanged with a W6 import/export route. Generic harness templates inherit vanilla-like/Anvil defaults; Earth configs explicitly state both selections.
- Empty native startup uses the existing preparation/publication pipeline. Optional `data.earth_definition` supplies immutable input; absent input uses the validated seed-derived small reference plane. Existing native publication/artifact/content mismatches fail closed without rebase.
- `EarthRuntime` is an immutable source provider sharing prepared store/lease, not a second world authority. Native chunk plans route through the existing load workers and resident publication; saves use the existing `DirtyFlushPlan`/save worker, in batches at most 64. Native misses never fall back to Anvil.
- Existing resident mutation, semantic WAL, pending-image and captured-Arc/revision acknowledgement fences remain. Generic owner checkpoints allow captured same/zero-LSN live changes after the owner barrier, while semantic replay remains monotonic. Computed native lighting is discardable and does not dirty a base-only visit.
- A borrowed `EarthBaseSource` is used by live runtime and activated offline state operations, including village/beard/marker/loot materialization. Unactivated physical planes permit inspection but refuse offline mutations until live activation pins the content/base profile; activated mutations require the original resource cache.
- Indexed spawn candidates are checked against natural water facts and final support/headroom; no origin plateau or noise fallback is created. Existing village piece/decor/loot ownership is preserved, with natural dry/protected/slope constraints at the Earth boundary.
- Generated villagers retain world-identity-addressed slots before spawn/WAL; the existing owner persists migration and death/tombstones, including death before the first checkpoint. The same owner allocator high-water is now checkpointed/restored for ordinary entities and retained through WAL spawn/remove histories.
- Removed old Tellus settings/payload, generator mode builders/field, Earth biome/water branches and obsolete Tellus pattern fixtures. Standalone vanilla `RealisticDeposits` support was retained because its production plugin override remains a caller; Earth does not call it. Vanilla-specific modules, Nether/End and vanilla-like revision 24 were not intentionally changed.

## Exercised validation

| Proof | Observed result | Receipt |
| --- | --- | --- |
| Recovered `cargo check -p mc-server -j 3` | Passed; no half-applied source edit remained | `recovery-check.log` |
| Optimized server and coastline example build | Passed after throwaway summary serialization repair | `candidate-recovered-build-after.log` |
| `cargo check --workspace --all-targets -j 3` | Passed before subsequent behavior-preserving Earth lint fixes | `workspace-check.log` |
| Native focused baseline | 57 passed across mc-world/worldgen/net/simulation | `native-focused-tests-post-integration.log` |
| Native startup regressions | 6 passed including optional-loot inventory, bounds expansion and all-ocean refusal | `startup-native-after.json`, `startup-native-after.log` |
| Earth runtime consumer regressions | 7 passed including mapper, exact base/sites and finite-domain halos | `earth-runtime-consumers-final-after.json`, `earth-runtime-consumers-final-after.log` |
| Review follow-on regressions | All eight findings closed; exact passing selectors/receipts mapped | `review-closure.json` |
| Strict touched-crate lint, all targets | Passed: mc-entity/net/server/simulation/test-harness/world/worldgen, `-D warnings` | `strict-clippy-final.log` |
| Historical Anvil canonical graphical reopen | Passed: actual Play, 28.986 m ordinary walk, pre-W5 gold/air/support retained; frozen source hashes unchanged | `anvil-reopen-final.json`, `anvil-reopen-region-semantic-diff.json` |
| Actual new native server launch, explicit Tellus/native, seed 42 | Automatic preparation committed; startup refused before Play because no safe dry land exists | `runtime-smoke.json`, `native.toml`, `native-world/` |
| Actual `world inspect --world-dir …/native-world --check` | Passed read-only inspection of committed identity | `runtime-smoke.json` |
| Final production-content coastline probe | Passed: 128 pages, 59 raw-site pages/72 raw sites, 43 accepted-site pages/52 accepted sites, 72.22% acceptance, zero persistent records | `final-v4/site-probe-summary.json` |
| Actual final native graphical Play and ordinary walk | Passed dry spawn and two real-input walking stages; 12 screenshots. Phase later failed on unloaded village destination | `graphical-9ghlyovr/w5-earth-before/20261002T051347Z-graphical-manifest-Waz40L/observations.json`, `screenshots/` |

All recovery heavy commands use `/home/kaiserroman/solaris/.analysis/tools/mem-run.sh`,
Cargo `-j 3`, `CARGO_INCREMENTAL=0`, and disabled dev/test debug info. Runtime
acceptance builds use dev optimization level 2 to avoid unoptimized cold solver
latency. Main owns final integrated project-wide L2; this worktree's requested
workspace/check and touched-crate lint gates are recorded separately. Formatting
has not been run by this worker.

The actual seed-42 small reference is entirely marine: elevation -4558.056605775788 to -4359.828763354943 m, ocean and physical area both 268435456 m², root payload 8135340 bytes. The committed identity was `ed1a2a26a77c20e95cf34e9319330c4d388044f49f946df24e3907675e863f67`. All imported/overlay/metadata/entity/slot/state counts were zero. Refusal is correct and explicit, but this is **not** proof of a working safe-land world. A declared supported land/shore plane or another seed is required for the client scenario; no terrain adjustment or fallback was applied.

### Rocky-peak biome threshold provenance

The coastal probe exposed a mapper defect: thin soil alone classified flat low
coast as `stony_peaks`, excluding otherwise eligible village biomes. The mapper
now requires rock exposure > 0.65 **and** physical ground elevation ≥ 1,500 m
**and** physical slope ≥ 0.1. These last two values are **provisional biome
presentation thresholds**, not a derived alpine/treeline belt. Spec §10.2 permits
an approximately 6 °C/km lapse rate; `earth/climate.rs` uses 0.006 °C/m and
`earth/region_geology.rs` applies the local physical-height correction. Neither
the spec nor that ecology model defines a treeline temperature, alpine belt or
mountain slope cutoff from which 1,500 m and 0.1 could be derived. The values
therefore must not be described as a universal climatic boundary. They prevent
rock exposure caused by cold/dry/thin soils from alone inventing mountain relief.
The permanent regression covers flat low coast, high steep exposed rock and low
steep exposed rock. The repeated production-content probe records all 128
coastline pages, raw/accepted sites and acceptance rate in
`final-v4/site-probe-summary.json` on a newly prepared identity; no authored island,
river relocation or village predicate weakening is used.

The completed final v4 probe examined **128 pages** with **59 pages containing
raw sites** (72 raw sites), **43 pages containing accepted villages** (52 accepted
sites), and a **72.22% site acceptance rate**. Its dry spawn was
(-7,625,000, 85, -1,875,000); the selected village was 227.976 m from the protected
ocean sample. All persistent state record counts remained zero. The final identity
is `0aa17daf73607162632ae6ab9e38ff2b008d1c926ac84bc90a230f654af78a55`.
Behavior-preserving strict-lint edits changed the source-byte artifact fingerprint,
so final v4 was prepared anew rather than automatically rebasing the recovery v3
world. The independent completed v3 probe measured the same accounting.
The earlier negative/diagnostic attempts each contain only 127 completed pages;
they are not falsely reported as a completed 128-page before comparison.

## W5 acceptance disposition

| Acceptance item | Status | Evidence / limitation |
| --- | --- | --- |
| New compact native world prepares automatically | Pass | Actual server published the checked seed-42 reference; three startup tests include declared small-plane prepare/reopen/mismatch refusal |
| New compact world reaches Play with dry safe spawn | Pass | Final non-authored supported procedural reference reached real graphical Play with dry stone support and full health; 12 actual spawn/walk images. Small all-ocean reference remains a correct negative regression |
| One native base/effective load/save path, no live `.mca` dual-write | Pass, focused/runtime path | Native source/dirty-save, clean-lighting, fallback refusal, edit/reopen and acknowledgement tests pass; finite-domain halo and pre-activation offline base fixes verified in `review-closure.json` |
| Old worlds untouched, existing Anvil opens unchanged | Pass | Genuine pre-W5 create/edit/close/hash → byte-identical same-path copy → W5 Play/walk. Frozen source all bytes unchanged; all 78 retained chunks preserve blocks/block entities. Normal copy saves update lighting/times and add 23 adjacent chunks; post-gameplay `.mca` byte identity is not claimed. Actual historical Tellus/Anvil refusal also leaves input bytes unchanged |
| Village natural-site adaptation and activation through existing owner | Pass, focused path | Real prepared natural/site consumer and native authored-catalog startup regressions pass; graphical coastal route recorded separately |
| Generated identity, migration and consumed-slot tombstones survive recovery | Pass, focused path | Generated move/death/WAL/reopen, killed-before-first-checkpoint, ordinary UUID high-water and aggregate budget proofs pass |
| Fire/growth/gravity/plugin mutations use native effective history | Partial proof | Shared generic checkpoint regression passes; evicted inhabited-time tail proof now passes. Individual actual gameplay mutation paths are not yet separately exercised |
| Obsolete Earth/Tellus settings/samplers/callers removed | Pass | Removed APIs have no callers; generic harness templates migrated to vanilla-like/Anvil defaults, diagnostics/native settlement consumer cut over |
| Canonical graphical spawn/village-water/walk/edit/restart acceptance | Fail, exact runtime blocker | Spawn and two ordinary-input walks passed. Immediately after real teleport to the selected village, `wait_loaded_block` refused unloaded destination (-7124789,54,5000158); subsequent village/water, edits, melee and restart/rejoin were not exercised. Server also reported checkpoint flushes of 13.5–15.0 s and shutdown drain exceeding 6 s; final save completed late |
| Focused tests, workspace check and strict touched-crate clippy | Pass | Focused behavioral receipts, workspace all-targets check and final strict seven-crate all-targets lint pass |

## Closed code-review findings

All eight findings in `review-blockers.json` are fixed. `review-closure.json`
maps each implementation and exact behavioral receipt, reusing successful
unchanged gates rather than rerunning them:

| Finding | Fix and exercised proof |
| --- | --- |
| Ordinary UUID reuse after tombstone/restart | Existing owner allocator high-water persists/restores; `entity-allocation-after.json` and `review-wal-after.json` |
| Evicted inhabited-time shutdown tail lost | Admitted off-thread load, existing resident mutation, failed-load retention; `review-inhabited-after.json` |
| Writer/reader aggregate budget mismatch | Historical tombstones and metadata count before publication; `entity-history-budget-after.log` |
| Optional loot leaves item decoder empty | Real item registry installed unconditionally; `startup-native-after.json` |
| Pre-activation offline edit uses a different base | Physical-plane edits refused before live activation; activated base/profile shared; `offline-state-after.json` |
| Bounds expansion invalidates spawn | Committed admissible spawn retained; excluding shrink refuses; `startup-native-after.json` |
| Finite-domain halo aborts startup | Imported coverage first, uncovered procedural chunks absent, warm skips absent; `earth-runtime-consumers-final-after.json` |
| Native authored-catalog startup requires generator | Existing owner consumes admitted native geography with no generator; native/Anvil compatibility selectors in `settlement-native-after.json` |

## Harness source assumptions supplied to Main

Generic live templates now inherit `vanilla_like` + `anvil`: `example.toml`,
`playable.toml`, `examples/loader-live-gate/playable.toml`,
`tools/harness/compatibility.py` and `tools/harness/precommit.py`.
Earth acceptance uses an explicit seed-42 procedural definition and
`tellus_like` + `native`, rather than converting generic fixed vanilla scenarios.

Loader's `playable-46-generated-ruin-cache` still addresses the explicitly
vanilla-like seed-zero ruin, not Earth geography. Core replay seed 81 uses
relative motion; M94 inventory and water routes use explicit operator fixtures.
No loader terrain coordinate or loot expectation was silently repinned.

## Shared-file hunks / integration boundaries

- `mc-worldgen/src/lib.rs`: remove Tellus settings export; export Earth-aware mosaic inputs.
- `mc-worldgen/src/terrain.rs`: vanilla-only constructors/routing; remove mode payload/field/builders; preserve standalone vanilla ore override; remove obsolete test-only helper.
- `mc-worldgen/src/village/plan_source.rs`: add crate-private `VillagePlanSet::from_plans` to combine the already-owned accepted plans and expose the existing `plan_for_start_chunk` as crate-private for the Earth boundary. Two import-order hunks also appear in the patch. No solver/piece/loot implementation was moved or duplicated. Used sources are included in the Earth artifact fingerprint.
- `mc-server/src/main.rs`: early native source admission; optional vanilla generator/native provider construction; source-specific rules/spawn/contract/lease/open/warm branches; remove old Tellus compatibility branch; reject native Anvil pregeneration.
- `mc-server/src/startup.rs`: native module/reexports; `startup/native.rs` and sibling tests are new.
- `mc-server/src/startup/terrain.rs`: remove obsolete mode constructor argument; retain production vanilla builders/resource owners.
- `mc-server/src/startup_validation.rs`: add explicit native source and expose the existing Anvil-presence predicate; old schema-4 Anvil contract remains.
- `mc-server/src/startup_rules.rs`: factor existing spawning rules; native uses spawning only and refuses unpinned tree/clay overrides.

Exact old-side shared hunk anchors from the binary patch: main `192,304,381,399,459`; startup `2`; startup/terrain `31,47`; startup_rules `1,12,42`; startup_validation `2,46,416`; lib `40,69`; terrain `160,195,237,634,717,758,791,932,990,1033,1052,1102,2643`; village/plan_source `88,100,297,627`. These anchors include formatting-only hunks and are provided for Main's concurrent settlement integration.

Graphical artifacts (`graphical_acceptance.py`, `graphical_driver.py`, `graphical-manifest.json`) are ignored receipts, not permanent harness changes. They reuse `python3 -m tools.harness run regression`. Actual final native spawn/walk and historical Anvil reopen were exercised; final native village/water/edit/restart acceptance did not complete because the teleported destination was unloaded. Generated-villager movement lacks a bridge operation and is covered only by the owner regression. No final native restart, death or water screenshot is claimed. The user requested wrapping the current step after this run; the remaining runtime failure is explicitly handed back, not silently marked complete.
