# Block-level world generation comparison

Compare **saved overworld blocks**, not just density or sampled height, with a
local stock Minecraft **26.1.2** server. This remains the gameplay oracle for the
26.3 protocol release, which retains the 26.1.2 gameplay datapack.

Prerequisites: Python 3.11+, NumPy, Pillow, Java 25+, a locally obtained Mojang
26.1.2 server bundle, an imported 26.1.2 content cache, and a release
`solaris-v2` binary. Neither this tool nor its reports redistribute the bundle,
content cache, or saved worlds. Run from the repository root:

```sh
/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 6G \
  python3 -m tools.worldgen_diff \
  --server-bin /absolute/path/to/release/solaris-v2 \
  --vanilla-jar /home/kaiserroman/solaris/.analysis/server.jar \
  --data-dir /home/kaiserroman/solaris/data/vanilla \
  --out .analysis/worldgen-diff/run
```

The single command generates seeds **0, 81, 497625**, **32×32 chunks per seed**,
and writes `report.json`, `report.md`, and six PNGs. Each rectangle is centered
on the **vanilla spawn chunk**, not the origin: seed 81, for example, spawns at
X −576, Z 432. A cached one-chunk bootstrap obtains each world's spawn before
the full rectangle is generated. Both implementations compare the same
absolute coordinates; this does not claim Solaris selects the same spawn.
Each seed's receipt records the actual spawn and chunk bounds. `--seeds` and
`--chunks` select a smaller diagnostic run; committed results state their scope.

`--out` must not already exist. Oracle worlds are cached by seed and rectangle
under `--vanilla-cache` (default `.analysis/worldgen-diff/vanilla-cache`); their
bundle SHA-256 must match, otherwise use a fresh cache directory. Bundled jars
are unpacked locally into a content-addressed runtime cache. Solaris always
pregenerates a new native world and exports it to Anvil. Keep these ignored
worlds if comparing before/after implementations; do not stage them.

## What is measured

- Exact full block-state agreement, including properties, over Y −64..319.
- All-cell and **non-air-union** denominators. Shared air can inflate all-cell
  agreement; non-air agreement excludes cells that are air on both sides.
- `no_decor_state`: full states after small plants (grass, flowers, kelp,
  vines, snow layers, etc.) become air/source water; air variants merge.
  Other properties remain significant. `no_decor` additionally drops all
  properties. The explicit plant set lives in `diff.py`.
- Block-name agreement, terrain-without-trees agreement and four-class
  air/water/lava/solid shape agreement. These are separate, looser metrics,
  **not exact parity**.
- Surface-height error: top solid terrain per column, with plants and trees
  removed. Also top non-air height (including water and trees), 3D biome-quart
  agreement, layer breakdowns, per-block confusion/count differences.
- Structure **start metadata**, by type and start chunk. This does not prove
  structure pieces, loot, entities, or block-entity contents agree. Block
  comparisons do include placed structure blocks.

Missing chunks or non-FULL vanilla chunks fail the comparison rather than
quietly increasing agreement by reducing the sample. Anvil packing is decoded
with padded longs (indices never straddle a 64-bit word), including signed NBT
longs and negative region coordinates.

Vanilla is tick-frozen by a datapack load function before world ticks, confirmed by `tick query`,
then force-loads the requested rectangle. An event-driven console/datapack
probe waits for all requested chunks to reach FULL before `save-all flush` and
`stop`. Vanilla also generates a FULL halo outside the rectangle; its requested
chunks/s therefore is **not raw generator throughput**. Solaris's CLI timings
include lighting and native storage but exclude reported setup time. Shared
host timings in the diff report are descriptive, not a controlled A/B benchmark.

## Focused tests

```sh
python3 -m unittest discover -s tools/worldgen_diff -p 'test_*.py'
```

The tests use synthetic blocks/jars only. The real three-seed command above is
the acceptance gate; unit tests do not substitute for it.

## Single-threaded generation A/B

Build `cargo test --release -p mc-worldgen --lib --no-run` at the baseline and
candidate revisions, preserving each reported test executable separately.
Builds go through the shared memory wrapper, not a measurement lane. On the
coordinator's approved measurement GO:

```sh
/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 6G \
  python3 -m tools.worldgen_diff.profile \
  --before /absolute/path/to/before-test \
  --after /absolute/path/to/after-test \
  --out .analysis/worldgen-diff/ab
```

This runs A,B,A,B,A,B, each generating 36 chunks per seed (0, 81, 497625) on one
thread, with stage timers compiled only into the test binary. Setup and lighting
are outside the generation timer. `measurements.json` contains every sample,
binary SHA-256s, per-seed medians/ranges, chunks/s, and whether the median
improvement exceeds the larger variant's run-to-run range. Do not claim a
speedup when that flag is false. These are **contended, shared-lane** numbers,
not whole-server throughput or quiet-window release claims.

The climate-column optimization removes repeated evaluation of Y-independent
noise subgraphs when filling the 3D biome palette. Independence is proven once
from the router DAG; unknown/Y-dependent nodes still evaluate normally. Raw
marker semantics and stateless first-leaf biome ties remain unchanged. Four
consecutive heights share one column cache; original palette insertion order
is retained. Removing this optimization means restoring stateless samples in
`assign_biomes` and deleting the dedicated raw-column sampler, not maintaining
a second generation mode.

Collect a sampling profile from the same frozen executable separately:

```sh
/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 6G \
  perf record -F 199 -e cycles:u -g --call-graph dwarf,32768 \
  -o .analysis/worldgen-diff/before.perf -- \
  /absolute/path/to/before-test --ignored --exact \
  terrain::profile_tests::parity_stages --nocapture --test-threads=1
perf report --stdio --no-children -i .analysis/worldgen-diff/before.perf
```

Re-run the full block diff with the candidate and compare each seed's `diff`
object against the baseline. For a stronger fence, use `diff.compare` directly
on the retained baseline/candidate exported worlds: `exact_state_mismatch_cells`
and `biome_mismatch_cells` must be zero, and `biome_cells` must cover every quart.
Rounded percentages are insufficient as an exact fence.
Timing improvements never justify relaxed parity.
