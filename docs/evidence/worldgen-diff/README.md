# Saved-block world generation evidence

This comparison uses the stock **26.1.2 gameplay oracle**, retained by the 26.3 protocol release plan. It compares three 32×32-chunk rectangles centered on vanilla spawn, Y −64..319: **3,072 chunks / 301,989,888 block-state cells**, plus 4,718,592 biome quarts. These are saved blocks from vanilla versus Solaris native pregeneration followed by Anvil export, not just density samples.

## Final revision-30 results

| Seed | Exact state, all cells | Exact state, non-air union | No-small-decor state, non-air union | Biome quarts | Surface MAE, blocks | Surface p95 / max, blocks | Structure starts vanilla / Solaris / same type+chunk |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 | 91.267% | 74.763% | 74.977% | 99.968% | 0.178 | 0 / 81 | 4 / 1 / 0 |
| 81 | 91.238% | 72.527% | 72.949% | 99.945% | 1.492 | 12 / 73 | 7 / 7 / 3 |
| 497625 | 91.104% | 73.699% | 74.013% | 99.958% | 0.304 | 0 / 64 | 7 / 5 / 2 |

Shared air inflates all-cell agreement; the non-air union excludes cells that are air in both worlds. `no_decor_state` removes the tool's explicit small-plant/decor set but preserves other state properties. Structure numbers compare start metadata only, not piece layouts or contents. Missing/non-FULL chunks fail closed. See [the generated report](report.md), [raw report](report.json), and [metric definitions](../../../tools/worldgen_diff/README.md).

## Landed improvements

1. **Climate-column reuse**, `db0ba531`: measured generation time 22.923→19.634 ms/chunk on seed 0 and 19.901→16.593 on seed 81; shared-lane measurements, not a quiet-window or VPS claim. Seed 497625 is inconclusive. The baseline/candidate fence is **zero block-state mismatches across 301,989,888 cells and zero biome mismatches across 4,718,592 quarts**. [Performance detail](PERFORMANCE.md), [samples](measurements.json), [exact fence](optimization-fence.json).
2. **Deepslate gradient**, separate commit `85df594b`: corrected the −8..0 reciprocal transition to the datapack's 0..8 linear rule. Exact all-cell agreement improves from 90.133→91.267%, 90.169→91.238%, and 90.115→91.104%. Vanilla-deepslate/Solaris-stone confusions fall from 1,578,103→218,606; 1,498,976→211,032; and 1,392,862→199,373 cells. Height, shape, biome and structure report metrics are unchanged. [Measured parity change](parity-improvement.json).

The deepslate fix intentionally changes generation, so it is **not** folded into the output-preserving optimization claim. It advances `WORLDGEN_REVISION` to 30. [Upgrade/startup and native-client far-fixture acceptance](VALIDATION.md) includes the focused 29→30 test, strict Clippy and build receipts.

### Remaining gaps

The largest remaining material differences are missing andesite/diorite/granite/tuff placements and bedrock thickness. Surface maps show much sparser Solaris forests; seed 81 also differs around ice and shoreline features. Mineshaft start chunks disagree on every sampled mineshaft; the other sampled structure starts align where reported. Seed 81's surface p95 error is 12 blocks despite its high all-cell agreement. These results do not claim full vanilla parity, all-dimension coverage, piece/loot/entity parity, or graphical-client validation.

## Images

Maps use generated flat colors, not redistributed Minecraft textures. **Left: vanilla; right: Solaris.** Height-error maps use white for equal terrain surface, red for Solaris higher, blue for Solaris lower; color saturates at ±16 blocks. Each footprint is 512×512 blocks, north/up is decreasing Z. Trees/decorations are excluded from terrain-surface height error, not from the paired top-view maps.

- Seed 0: [paired map](map-0.png), [height error](height-0.png) — matching rivers and hills, visibly sparse Solaris forest.
- Seed 81: [paired map](map-81.png), [height error](height-81.png) — ocean/frozen shore; the largest surface discrepancy of the three.
- Seed 497625: [paired map](map-497625.png), [height error](height-497625.png) — matching coastal outline, different vegetation and local cuts.

## Reproduce

From the repository root, after building the release server and importing the local 26.1.2 cache:

```sh
/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 6G \
  python3 -m tools.worldgen_diff \
  --server-bin /run/media/kaiserroman/games/solaris-work/targets/worldgen/release/solaris-v2 \
  --vanilla-jar /home/kaiserroman/solaris/.analysis/server.jar \
  --data-dir /run/media/kaiserroman/games/solaris-work/wt/worldgen/data/vanilla \
  --out .analysis/worldgen-diff/reproduce
```

The output directory must not already exist. Tool prerequisites, the frozen-oracle lifecycle, cache behavior, and A/B reproduction are in the [tool README](../../../tools/worldgen_diff/README.md).

## Artifact identity

Baseline/optimization reports are [baseline.json](baseline.json) / [optimized.json](optimized.json). Final `report.json` records the checkout HEAD at launch (`db0ba531`), before the already-built correction was committed as `85df594b`; it is not the compiled-source commit. The final executable SHA-256 is `a2ca60290a7c3c7b560125c7d2ee23bcd28ccb5781f14775e26a689616326375`. [parity-improvement.json](parity-improvement.json) makes that mapping explicit. Vanilla bundle SHA-256 is `cd47e7c38328f64768fd17af8fcd8b22496b40b63d4ffee81e71ae059fedcb42`. No jar, content cache, saved world or exported program fixture is committed.
