# v2 player-capacity receipt (2026-10-21)

**Status: draft, first v2 receipt.** Release `solaris-v2` against headless Rust protocol bots
(`player-scale-bots`), fresh generated world per rung, view distance 8 and 10.
The result is **not** "N players OK": 20 players is borderline and 50 players fails in every
run on this machine, in the serialized `mem-run.sh -p` lane (CPUs 8-11 = 2 physical cores).
The failure is a checkpoint-driven tick stall that sheds connections, not CPU saturation.
Numbers are single runs per cell; there is no variance estimate. Not a VPS or 16-core claim.

## Environment

- AMD Ryzen 5 7535HS (6 cores / 12 threads, L2 3 MiB, L3 16 MiB), 30 GiB RAM + 8 GiB swap,
  Linux 7.0.0-38-generic, cpufreq governor `powersave` (amd-pstate active, boost on).
- Lane: `mem-run.sh -p -m 5G` = `taskset -c 8-11`, `CPUQuota=600%`, `nice 0`, serialized by the
  perf lock. Scope line: `mem-run: scope -p MemoryMax=5G -p MemorySwapMax=1G -p CPUQuota=600% -p CPUWeight=50 nice=0 exclusive=0 perf=1 cpus=taskset -c 8-11 slice=solaris-heavy.slice`.
- **Contention caveat:** ~26 other agents were compiling/testing on CPUs 0-7 the whole time
  (loadavg 5.5-10.6 at run start/end). The lane isolates CPUs, not L3/memory bandwidth or the shared NVMe.
- Two CPU envelopes inside the lane, both recorded per run:
  `2HT` = server on 8-9 (one physical core, 2 hardware threads), bots on 10-11;
  `4HT` = server and bots both on 8-11 (server may use two physical cores; bots used 0.06-0.23 core).
  The whole-machine (`-x`) envelope was not measured.

## Binaries

Frozen release build of clean HEAD `ab1fa2401944af42070e9faf38e04146c394679b` (separate worktree,
own target dir, copied aside; the moving shared tree did not contribute):

```sh
git worktree add --detach /home/kaiserroman/solaris-wt-capacity HEAD
cd /home/kaiserroman/solaris-wt-capacity
CARGO_TARGET_DIR=/home/kaiserroman/solaris/target/capacity CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
  /home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 8G cargo build --release --locked \
  -p mc-edge --bin solaris-v2 -p mc-test-harness --bin player-scale-bots
```

`solaris-v2` sha256 `01e59c5b903b21e38ccd2ab8ac4d23f8040950a64a1b062dda4e0ffc1cfc5542`,
`player-scale-bots` sha256 `3e2927212e026ce812e932e2cde81d575a9df194a65cf3554c4b560347166eb2`
(line-tables-only debuginfo does not change codegen). The bot binary reads registries from the
compile-time `data/vanilla`, so the worktree has `data/vanilla` symlinked to the main checkout.

## Workload

Driver: `tools/coreperf/capacity_ladder.py` (reuses the catalog-load `Session`/`config`/`login_window`
machinery and its `/load-metrics` raw tick stream; adds per-window phase/thread-CPU/network/checkpoint
accounting and an optional perf capture). Per rung: new empty world dir, seed 42, defaults for
everything else (`[server] view_distance = simulation_distance = VD`, offline mode, dashboard on),
operators for all bots (creative landing uses commands). Bots log in 3 at a time in groups of 5
(5x5-block clusters) with groups 256 blocks apart on a grid at (1024,1024)+, so groups overlap views
only partially. Open-loop schedule (wall clock, never waits for responses):

| window | length | movement | block edits | chat |
|---|---|---|---|---|
| steady | 120 s | 20 Hz/bot | `build` 2 Hz, `fight` 1 Hz | 1 Hz |
| burst | 60 s | 20 Hz/bot | `build` 5 Hz, `fight` 2 Hz | 2 Hz |
| recovery | 20 s | none, readers only | none | none |

- `build`: creative bots, daytime (`time set 6000`, no daylight cycle): walking legs of 43-56 blocks with
  sprint/jump, break/place, chat.
- `fight`: `--survival --respawn`, night (`time set 18000`, no daylight cycle): natural hostile spawns
  attack walking survival bots, which die and respawn. **The bots never attack**; "fighting" is mobs
  hurting players, not player-vs-mob damage. Bot-side attacks exist only via per-action mechanic specs
  and were not used.
- Wall-tick = core active iteration `wall_us` (excludes pacing sleep). TPS = completed ticks / window
  length. Operator limits applied for pass/fail: sustained TPS >= 19.5 (5 s bins), p99 <= 50 ms,
  max <= 250 ms, server RSS <= 3072 MiB, no disconnects. A run in which bots lose a connection
  ends early; its window is measured only up to the last completed tick (`partial`).

## Commands

```sh
cd /home/kaiserroman/solaris
B=/home/kaiserroman/solaris/.analysis/codex-logs/capacity-ladder   # bin/ holds the two frozen binaries
.analysis/tools/mem-run.sh -p -m 5G bash $B/run-all2.sh            # one scope, runs sequentially:
python3 -m tools.coreperf.capacity_ladder --server-bin $B/bin/solaris-v2 --bots-bin $B/bin/player-scale-bots \
  --out $B/4ht-build-vd8   --server-cpus 8-11 --bot-cpus 8-11 --workload build --view-distance 8  --clients 20 50 100
  --out $B/2ht-build-vd8   --server-cpus 8-9  --bot-cpus 10-11 --workload build --view-distance 8  --clients 20 50 100
  --out $B/2ht-perf50      --server-cpus 8-9  --bot-cpus 10-11 --workload build --view-distance 8  --clients 50 --perf-seconds 40 --perf-delay-seconds 2
  --out $B/4ht-fight-vd8   --server-cpus 8-11 --bot-cpus 8-11 --workload fight --view-distance 8  --clients 20 50 100
  --out $B/4ht-build-vd10  --server-cpus 8-11 --bot-cpus 8-11 --workload build --view-distance 10 --clients 20 50 100
```

Receipts (ignored, local): `$B/<run>/summary.json` (headline per rung), `$B/<run>/rung-N/result.json`
(full windows, phases, thread CPU, checkpoints, bot report), `samples.jsonl`/`login-samples.jsonl`
(raw per-poll tick samples), `server.log`, `bots.log`, `perf-by-*.txt`. An earlier round on the same
binaries (`build-vd8`, `build-vd8-r2`, `build-vd8-perf50`, `fight-vd8`; driver without partial-window
handling and with a 10 s server-stop limit) agrees with the numbers below.

## Results

Steady window unless marked; `p50/p95/p99/max` are wall-tick ms; CPU = server cores used
(`/proc` utime+stime over the window); net = server bytes written per player (dashboard counter).
"shed" = connections the server closed with `hub queue full` (see below); `ckpt` = longest
60 s-cadence checkpoint commit.

| env | load | VD | N | outcome | measured | TPS (sustained min) | p50/p95/p99/max | CPU | RSS MiB | net KiB/s/pl | shed | ckpt s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 2HT | build | 8 | 20 | PASS | 120+60+20 s | 20.0 (20.0) | 5.9/12.6/16.1/27 | 0.32 | 353 | 13.8 (burst 23.9) | 0 | 6.3 |
| 2HT | build | 8 | 50 | shed | 52 s of steady | 19.83 (18.4) | 14.2/37.1/67.7/93 | 0.86 | 492 | 16.9 | 50 | 19.3 |
| 2HT | build | 8 | 50 (round 1) | PERF_FAIL | 120+60 s | 19.87 (17.8) | 18.1/40.7/61.3/107 | 0.97 | 490 | 16.7 (burst 28.9) | - | - |
| 2HT | build | 8 | 100 | shed | 32 s of steady | 17.88 (15.0) | 34.1/86.8/108/156 | 1.58 | 674 | 21.3 | 1 | 36.7 |
| 4HT | build | 8 | 20 | PASS | 120+60+20 s | 20.0 (20.0) | 5.4/10.9/13.4/17 | 0.33 | 373 | 13.8 (burst 23.5) | 0 | 7.6 |
| 4HT | build | 8 | 50 | shed | 24 s of steady | 19.99 (20.0) | 9.2/21.8/27.4/33 | 0.89 | 508 | 16.9 | 4 | 20.4 |
| 4HT | build | 8 | 100 | shed | 13 s of steady | 18.67 (19.0) | 19.4/37.5/155/673 | 1.66 | 691 | 20.5 | 100 | 61.6 |
| 4HT | fight | 8 | 20 | shed | 23 s of steady | 19.96 (20.0) | 4.1/23.2/34.3/41 | 1.13 | 391 | 28.9 | 6 | 12.7 |
| 4HT | fight | 8 | 50 | shed | 26 s of steady | 19.97 (20.0) | 11.5/29.4/39.9/49 | 1.53 | 541 | 22.3 | 39 | 26.2 |
| 4HT | fight | 8 | 100 | shed | 19 s of steady | 16.14 (6.4) | 22.4/65.4/188/1633 | 1.85 | 714 | 24.8 | 20 | 60.0 |
| 4HT | build | 10 | 20 | PERF_FAIL | 120+60+20 s | 20.0 (20.0); burst 19.38 (13.0) | 7.8/16.8/22.6/46; burst 13.1/21.8/53.5/1146 | 0.44 | 396 | 15.2 (burst 25.9) | 0 | 13.7 |
| 4HT | build | 10 | 50 | shed | 7 s of steady | 19.92 (20.0) | 10.1/22.5/33.2/37 | 0.80 | 561 | 20.2 | 50 | 23.5 |
| 4HT | build | 10 | 100 | shed | 38 s of steady | 19.85 (19.4) | 27.4/47.3/61.2/83 | 1.96 | 771 | 24.5 | 18 | 51.8 |
| 2HT | fight | 8 | 20 | PERF_FAIL | 120+60+20 s | 19.85 (16.4) | 5.6/19.1/33.0/693 | 0.60 | 390 | 25.5 | - | - |

(The last row and the "round 1" row come from the earlier round of the same binaries; their
server was SIGKILLed after the old 10 s stop limit.) Short partial windows (7-38 s) make p99 and
sustained TPS weak evidence; the login rows are more reliable for the 100-player case.

Login burst (3 concurrent logins, every bot through landing; not a steady-state metric):

| env | load | VD | N | seconds to all-ready | TPS | p50/p95/p99/max ms |
|---|---|---|---|---|---|---|
| 4HT | build | 8 | 20 | 46 | 20.0 | 12.0/20.8/26.7/34 |
| 4HT | build | 8 | 50 | 108 | 19.9 | 16.8/29.6/39.1/423 |
| 4HT | build | 8 | 100 | 206 | 17.7 | 21.5/46.6/66.6/6153 |
| 2HT | build | 8 | 100 | 384 | 17.8 | 20.6/66.2/97.6/4622 |
| 4HT | build | 10 | 100 | 256 | 16.8 | 21.6/53.5/77.3/13129 |

Steady-state CPU scales almost linearly: 0.016 (N=20), 0.018 (N=50), 0.017 (N=100) cores per player
(4HT build VD8). Server RSS grows 0.37 -> 0.51 -> 0.69 GiB for 20/50/100; bots use <= 0.23 core.
Tick phase means (4HT build VD8 steady, ms): wall 6.3 / 11.5 / 26.3; `input` 3.6 / 4.5 / 7.4;
`block` 1.0 / 2.8 / 5.3; `entity` 0.5 / 1.2 / 2.5; `chunks` 0.3 / 0.9 / 1.4; `journal_build` 0.3 / 0.7 / 1.5;
`journal_queue` 0.007 / 0.010 / **5.2** (max 659 ms at N=100).

## Failing step and top bottleneck

First failing step: **N=50**. In round 2 all five 50-player runs shed connections; the one round-1 run of
the same binaries that did not (2HT build) finished but missed sustained TPS (17.8) and p99 (61 ms).
N=20 passes in 2 of 5 runs (2HT build, 4HT build); the other three (4HT fight, 4HT VD10, 2HT fight) hit a
stall: 6 sheds, a 1146 ms and a 693 ms `journal_queue_us` tick respectively.

Observed:

- The server closes players with `hub queue full; connection closed ... exit="submit-shed"`
  (`mc-edge/src/server.rs`: 64 consecutive `hub.submit` refusals; the core input queue is
  `input_queue_cap = 4096`). The bot process then exits (`bot N: peer closed`), ending the rung.
- The last tick the harness saw completed began 1.9-3.0 s before the first shed in the three 50-player runs
  I checked: the tick in progress had already been running that long. Sheds land 0.8-7 s before the next
  `checkpoint committed` line, which itself follows the previous one by ~60 s
  (`checkpoint_every_ticks = 1200`); e.g. 4HT N=50: commit 19:38:44, sheds 19:39:44.09, next commit 19:39:50.7.
- Checkpoints rewrite essentially every resident chunk each time (`rewritten_chunks` 1.2-2.1k at N=20,
  3.8-4.6k at N=50, 6.4-7.4k at N=100 vs `chunks_loaded` 1.6k / 3.9-4.1k / 6.7k at VD8) and the longest per run
  took 6-14 s / 19-26 s / 37-62 s wall, 120-712 MiB chunk payload each. At N=100 the checkpoint takes
  about as long as its 60 s interval. The final save on SIGTERM took 16-30 s at N=50 and 48-83 s at N=100
  (the harness default of 10 s would have SIGKILLed it).
- Section-granular capture (after these receipts): `ChunkWorld` records which block sections `fold_overlay` edited
  per dirty chunk and the native checkpoint examines only those (`NativeChunkSnapshot::dirty_sections`);
  metadata-only chunks no longer encode any section overlay. `checkpoint committed` now logs
  `sections_encoded` (overlay evaluations) and `section_records_written`; measure `rewritten_chunks` with them.
- Ticks that complete during commits block in `journal_queue_us` ("seal plugin events and enqueue WAL,
  including bounded writer backpressure", `mc-core/src/boundary.rs`): `2ht-perf50` tick 5153 spent
  1554 ms there, five ticks in a row 0.14-1.55 s, inside the 4800 checkpoint's 21 s commit window.
  That 1.5 s stall did not shed anyone; the shed came a minute later with a stall that had not completed
  when the harness stopped sampling. Rolling `/stats` `tick.max_us` at the end of shed runs: 0.42-1.8 s at
  N=50, 5.2-10.2 s at N=100 (round 1).
- CPU is not saturated at the failing step: N=50 uses 0.86-1.5 cores server-wide, the tick thread 0.19-0.23
  core. At N=100 the 2HT lane saturates (1.58 of 2 threads) and p50 tick reaches 34 ms.

[INFERENCE, not separately proven] The causal chain is: the single `mc-core-native` writer thread is busy
with the full-world checkpoint encode/commit, the WAL enqueue backpressures the tick, input is not drained
for >= ~3 s while 50-100 bots keep sending 20 Hz movement (about 1,000-2,000 inputs/s against 4096 slots),
the queue fills and the edge sheds players. Checkpoint-window `journal_queue_us` stalls were observed
(0.7-1.6 s); the multi-second one that actually sheds was not captured to completion.

Profile (2HT, N=50, 40 s inside steady starting 2 s after Run, `perf record -F 499 --call-graph dwarf,16384`,
reported with `--no-children --no-inline -g none`; the capture ended before the stall, and `perf.data` was deleted
after reporting, so only the flat report survives: `2ht-perf50/rung-50/perf-by-comm-sym.txt`).
Share of all server samples by symbol:
`mc_edge::light::LightBaseline::update` 10.4 % + `<LightSection as PartialEq>::eq` 7.2 % (edge-fanout threads,
17.6 % together), `mc_core::farming::sample_chunk` 6.8 % (tokio worker; hot with no farmland in the scene),
`mc_world::native::codec::overlay` 6.5 % + `Vec::from_iter` 3.3 % (`mc-core-native`, the checkpoint writer),
`ChunkWorld::block_at` 4.8 %, `plan_viewer_inner` 4.0 %. By thread (sum of listed symbols): tick 16 %,
native writer 14 %, edge-fanout 25 %, tokio 9 %. Steady thread CPU in that window (cores):
tick 0.225, tokio 0.162, fanout 0.127 + 0.110, native writer 0.124, chunk-load 0.029.

Conclusion: the capacity limit on this configuration is checkpoint cost and its WAL backpressure
(grows with resident chunks, i.e. with players x view distance), not steady per-player CPU.
Secondary CPU costs worth a separate look: light baseline diffing per viewer in edge fanout and
`farming::sample_chunk` per resident chunk.

## Open issues / not covered

- No `-x` whole-machine run and no 16-core comparison; single runs, no repeats.
- Perf capture missed the stall; one more capture timed to a checkpoint (and with the call graph kept) is
  needed before blaming `overlay`/`from_iter` specifically.
- Bots abort when one connection is shed, so shed runs give partial windows only.
- `fight` is hostile-vs-player only; player attacks and survival edits were not driven.
- One `SIGTERM` during a 100-player login (round 1, mine) logged
  `generated chunks not saved error=storage: native source: native generated: duplicate participant`;
  not reproduced, not investigated.
- A leaked idle Gradle daemon scope from another agent blocked `mem-run` admission for ~2 h
  until stopped; `rust-analyzer` (4-5 GiB) did the same twice and was killed.
