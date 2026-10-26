# Quirk and farm parity gate

Solaris deliberately keeps vanilla behaviours that players rely on (TNT duping,
quasi-connectivity, update order, attribute swapping, diagonal sprint speed) and
must not change farm output rates (iron, villager breeder, mob, crop farms).
This gate proves that against a **real Mojang 26.1.2 server** run on the same
construction, instead of from memory or wiki text. Vanilla is the oracle; a
scenario that vanilla itself does not show is a failing scenario.

```sh
# whole gate (needs .analysis/server.jar = vanilla 26.1.2 bundler, Java 25, a built mc-core test binary)
python3 -m tools.harness run quirk-parity -- --family quasi-connectivity
python3 -m tools.harness.quirk_parity --out DIR [--scenario ID] [--family F] [--vanilla-only|--solaris-only]
python3 -m tools.harness.quirk_parity --out DIR --fuzz-seed 1 --fuzz-cells 256 --fuzz-style dense|mixed|bud|slime3d
```

Receipts: `DIR/receipt.json` (per scenario `status`: `pass`, `fail`,
`known-divergence`, `findings` for fuzz; diffs, vanilla self-check failures,
wall times), `DIR/vanilla-results.json`, `DIR/solaris/receipt.json` and the
`vanilla.log` / `solaris.log` of every run. The harness profile `quirk-parity`
(`tools/harness/profiles.py`) wraps the same module.

## How a scenario runs

Both servers execute the **same console command strings** (`setblock`, `fill`,
`summon`, `gamerule`, `time`, `weather`) from the scenario file:

* vanilla: a void superflat world, `tick freeze`, `forceload` of every box,
  `tick sprint N` per phase (completion is the server's own "Sprint completed"
  log line), then `save-all flush` and the saved Anvil files are decoded
  (`tools/harness/quirk_parity/anvil.py`: blocks, entities, container items).
* Solaris: the real native tick in-process
  (`crates/mc-core/src/quirk_parity_tests.rs`, `#[ignore]` test run by the
  harness with `SOLARIS_QUIRK_SCENARIOS` / `SOLARIS_QUIRK_RECEIPT`): the same
  commands through the console command path, `run_tick` per tick, same
  snapshot format. A command Solaris rejects but vanilla accepts is a failure.

Nothing sleeps: every wait is a specific server log line / tick count.

## Scenario file (`tools/harness/quirk_parity/scenarios/*.json`)

```json
{
  "id": "qc-piston-bud", "family": "quasi-connectivity", "description": "...",
  "box": [x1,y1,z1,x2,y2,z2],              // or "boxes": [[...],...] - independent cells on one timeline
  "prelude": true,                         // default: spawn_mobs/random ticks/time/weather frozen identically on both sides
  "seed": 1, "seeds": [1,2,3],             // seeds => one run per seed on both servers (rate scenarios)
  "setup": ["fill ...", "setblock ..."],   // before tick 0
  "settle_ticks": 1,                       // ticks run after setup, before the first snapshot (default 1)
  "phases": [{"name": "p", "commands": ["..."], "ticks": 20}],
  "trace": false,                          // snapshot every tick of every phase (small scenarios only)
  "compare": {"blocks": "exact", "entities": "counts", "containers": "exact",
              "phases": ["p"], "entity_types": ["minecraft:tnt"], "ignore_entity_types": [],
              "entity_tolerance": 0, "ignore_blocks": ["minecraft:fire"],
              "entity_positions": {"types": ["minecraft:tnt"], "epsilon": 0.05}},
  "vanilla_expect": [                      // the oracle self-check: the quirk exists in vanilla
    {"phase": "p", "check": "block", "pos": [x,y,z], "equals": "minecraft:piston[extended=true,facing=east]", "cell": 0},
    {"phase": "p", "check": "count", "key": "minecraft:tnt", "min": 1, "max": 1, "cell": 0}],
  "rate": {"metric": {"kind": "container_items", "item": "minecraft:iron_ingot"},   // farm rate scenarios
           "baseline_phase": "warmup", "phase": "run", "tolerance_pct": 5, "min_vanilla": 1},
  "known_divergence": "one-line reason + repro; the marker itself fails the gate once Solaris matches",
  "allow_solaris_command_errors": false
}
```

`rate.direction: "drop"` measures a metric that shrinks (consumption/harvest) as
baseline minus end value.

Metric kinds: `container_items` (all hoppers/chests/barrels/...), `item_entities`,
`entity_count`, `block_count` (by block name), `block_state_count` (`equals`: exact
`name[sorted props]`, e.g. `minecraft:wheat[age=7]`). Rate comparison: mean production per seed,
Solaris vs vanilla; passes inside `tolerance_pct` of the vanilla mean or two
standard errors of the seed noise, whichever is wider.

Blocks are compared as `name[sorted props]` (non-air only); entities by count
per type (item entities per item id, weighted by stack size); randomised
quantities (TNT launch direction, explosion block selection) are compared as
counts, not positions. Keep exact-state scenarios shorter than any fuse that
ends in an explosion (80 ticks).

## Scenario families

| family | scenarios | what must match vanilla |
|---|---|---|
| quasi-connectivity | `qc-*` | piston/dropper/dispenser powered through the block above, only reacting on a neighbour update |
| tnt-duping | `tnt-*` | BUD TNT duplication through piston push order (destroy-list/shape-update), controls |
| update-order | `fuzz-*` | seeded piston/TNT/rail/carpet/coral/observer contraptions: every vanilla duplication cell must match, other differences are `findings` |
| zero-tick | `zerotick-*` | 0-tick behaviours that still exist in 26.1.2 |
| player | `player_scenarios/*` | diagonal sprint speed acceptance, attribute swapping (see `tools/harness/quirk_parity/player.py`) |
| farm-* | `farm-*` | production per hour over several seeds within tolerance |

Rules: never "fix" a divergence by making Solaris stricter than vanilla. A
Solaris divergence is either fixed with a regression test (small fix) or
recorded as `known_divergence` with an exact repro in the receipt/report.
Mojang bytes never enter Git: the jar and worlds stay under `.analysis/`.

## Findings (26.1.2 oracle, 2026-10-22)

Fixed in Solaris with regression tests (each proven by a vanilla-vs-Solaris gate run):

| area | divergence found | fix |
|---|---|---|
| quasi-connectivity | pistons/droppers/dispensers ignored the block-above rule and a piston was powered through its own face | `redstone::piston_powered` (every side except the push face + QC through the block above); dispenser/dropper QC; still only reacts on a neighbour update (BUD) |
| sugar cane / cactus | grew a segment on every random tick (6.5x vanilla) | `plant_rules::vertical_plant_growth_edits` ages `age` 0..15 and only age 15 extends the stalk, as `SugarCaneBlock`/`CactusBlock.randomTick` |
| crops | night stopped growth: Solaris used the sky-darkened brightness (wheat 0.31x vanilla over 3 days) | crops/stems/bamboo gate on `raw_brightness` (= `getRawBrightness(pos, 0)`); spawning and saplings keep the darkened value |
| chicken eggs | `egg_interval` was an arithmetic progression of sequential ids (first eggs clustered early, +6% eggs) | SplitMix roll over `nextInt(6000)+6000` |
| mob fall damage | mobs never took fall damage (kill chambers did not kill) | `mob_fall::after_move` accumulates the drop (incl. the landing move) and owes `ceil(d-3)`; `#fall_damage_immune`, flyers, swimmers, water/climbables, slow falling/levitation excluded |
| player fall damage | the landing packet's own drop was not counted | `PlayerState::fall` adds `dy < 0` before the ground test (`Entity.checkFallDamage`) |
| iron golem loot | the simulated golem blueprint carried no loot table (no ingots/poppies) | blueprint falls back to the catalog's `entities/<name>` table |
| /summon | villager `VillagerData` and item entities (`Item`, `PickupDelay`, `Age`, `Motion`) were refused | accepted (validated by the existing villager/item parsers) |
| console `tp <player>` | panicked the tick thread (`executor survived teleport`) | falls back to the player's own dimension |

Gate results (`g7`/`g8` receipts): `qc-piston-bud`, `farm-crop-wheat`, `farm-crop-cane`,
`farm-chicken-eggs`, `farm-iron-golem` pass; the player diagonal-sprint gate passes
(0 corrections both sides).

Known divergences (marked `known_divergence` in the scenario, reported not failing, stale marker fails):

* `tnt-duper-found-bud`: TNT duplication needs vanilla's `moving_piston` window; Solaris pistons move
  blocks instantly, so the three oracle-found BUD cells (vanilla: 1 TNT built -> 2 primed) end with 1.
  Architectural (piston movement model). Same root for carpet/rail dupers.
* `tnt-bud-push-destroy-list`: TNT outcome matches; flowing-water/waterlogged state of the removed
  destroy-list neighbours differs (fluid tick order around the push).
* `farm-mob-kill-chamber`, `farm-gold-nuggets`: idle-wander drive is applied while a mob is airborne, so
  one skeleton per wave drifts onto the glass rim and its drops never reach a hopper (~101 of ~128 bones;
  vanilla 123). Loot, fall damage and hopper pickup match (100-tick waves: 123 bones).
* `farm-villager-breeder`: farmer harvest is ~2.4x vanilla (35 vs 14.8 carrots per work day).
* quasi-connectivity of dispensers/droppers has no oracle scenario yet.

Harness notes: Solaris draws are a pure function of (entity id, tick), so `quirk_parity_tests` burns a
seed-dependent run of wire ids per `seeds` variant (otherwise every seed replays one sample);
`--vanilla-results PATH` reuses a previous run's vanilla oracle output (the oracle is the slow half);
Solaris polls every reactive cell once after chunk hydration, so scenarios that must stay un-updated
(BUD) set `settle_ticks` before placing their power sources.

## Native block interaction regressions (26.1.2)

`mc-core`'s `interact_parity_tests` covers axe stripping/scraping/unwaxing,
held-use brushing of suspicious sand/gravel, decorated-pot and chiseled-bookshelf
custody, bounded sponge absorption, dragon-egg teleportation, eternal-fire
supports, spawn protection, and painting placement/support removal. These are
focused native regressions, not a claim that the graphical-client parity gate
has exercised every interaction.

Brush loot is revealed into retained block-entity NBT before completion; book
and pot contents use native inventory custody and hydrate from retained NBT.
Painting variants/directions are retained in entity metadata and native saves;
the protocol regression pins the registry-holder wire encoding to local
26.1.2 `javap` evidence.

`server.spawn_protection` (vanilla `spawn-protection`, default 16) protects the
Overworld spawn square only when the persisted operator list is nonempty.
Operators bypass it even when other operators are offline; zero disables it.

