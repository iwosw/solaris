# Climate-column generation optimization

## Scope and result

On an AMD Ryzen 5 7535HS (6 cores / 12 logical CPUs), release generation on one thread was measured with frozen executables in A,B,A,B,A,B order. Each invocation generated 36 chunks for each of seeds 0, 81 and 497625, at chunk coordinates −3..2 in each axis. Setup and lighting are outside the generation timer. These are **contended shared-lane laptop measurements**, not an exclusive-window benchmark, a low-end VPS result, or whole-server throughput.

| Seed | Baseline median ms/chunk | Optimized median ms/chunk | Baseline chunks/s | Optimized chunks/s | Median time reduction | Larger run range ms | Conclusion |
|---:|---:|---:|---:|---:|---:|---:|---|
| 0 | 22.923 | 19.634 | 43.62 | 50.93 | 14.35% | 3.199 | median improvement exceeds run range |
| 81 | 19.901 | 16.593 | 50.25 | 60.27 | 16.62% | 2.342 | median improvement exceeds run range |
| 497625 | 22.837 | 21.186 | 43.79 | 47.20 | 7.23% | 5.434 | **inconclusive**; not claimed as a speedup |

Every sample, command and binary fingerprint is in [measurements.json](measurements.json). There are three repetitions per variant; the small sample and shared host limit generalization. The native CLI's separately recorded pregeneration times include lighting/storage and may use parallel generation, so they are not substituted into this single-threaded table.

## Bottleneck and change

A separate `perf record -F 199 -e cycles:u -g --call-graph dwarf,32768` run collected 1,216 samples with zero lost samples. The top self-time symbol was `ImprovedNoise::sample_with_y_scale` (11.90%); climate-tree search was 4.28%. That sampling invocation includes setup and lighting, unlike the generation timer.

`assign_biomes` formerly sampled six raw climate fields independently for every biome quart. Most noise subgraphs are Y-independent, but the scratch memo was discarded at every sample. The optimization proves independence once using the existing conservative DAG proof, then reuses node values at the same X/Z across four consecutive quart heights. It preserves raw marker semantics, full-coordinate keys for Y-dependent nodes, stateless first-leaf biome ties, and original palette insertion order. The normal stateful `ClimateSampler` is unchanged.

The baseline is gameplay source `1aaa08dd` plus the profiling helper committed in `364315ec`; the optimization source is `db0ba531`. The measurements were captured before the separate deepslate/revision-30 parity correction. Report JSON records the checkout HEAD at execution time, which was still `364315ec` while the optimization was uncommitted; executable hashes, the source commits here, and the exact-output fence identify the actual variants.

## Correctness fence

The full saved-block comparison covered 1,024 spawn-centered chunks for each seed, not just the smaller timing rectangle. Baseline versus optimized native exports had:

- **301,989,888 block-state cells: zero mismatches**, including properties.
- **4,718,592 biome quarts: zero mismatches**, with complete quart coverage.
- Identical complete vanilla-comparison `diff` objects for all three seeds, including structures, heights, per-layer metrics and confusions.

[optimization-fence.json](optimization-fence.json) records unrounded mismatch counts and server fingerprints. Rounded 100% values alone are not used as an exactness proof. The subsequent deepslate correction intentionally changes generation and is excluded from this optimization fence.

## Reproduction

Build the release `mc-worldgen` test executable at `364315ec` and `db0ba531`, preserving the executable path printed by Cargo after each build. Run all Cargo and measurement commands through the shared 6 GiB wrapper, after the coordinator's measurement GO:

```sh
/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 6G \
  env RUSTC_WRAPPER= CARGO_TARGET_DIR=/absolute/path/to/own-target \
  cargo test --release -p mc-worldgen --lib --no-run
/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 6G \
  python3 -m tools.worldgen_diff.profile \
  --before /absolute/path/to/before-test --after /absolute/path/to/after-test \
  --out .analysis/worldgen-diff/ab
```

The imported content cache must be a real `data/vanilla` directory: the loader rejects a symlink as the cache root. Use the final diff tool for spawn-centered block comparisons; the earlier tooling commit's origin-centered rectangle did not contain seed 81's actual spawn. See [the tool instructions](../../../tools/worldgen_diff/README.md) for the full comparison command and metric definitions.
