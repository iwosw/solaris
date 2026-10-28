# Benchmarks: weak hardware, core scaling, worldgen, VPS

Solaris is a **draft prerelease**. Each number published for it comes from **one documented
command** that runs a release build with a fixed seed and writes `result.json` + `result.md`
next to a hardware record. These commands measure a stated workload on stated hardware; they are
not performance guarantees, capacity promises or vanilla-parity claims.

`tools/bench` is a thin matrix runner. It does **not** start servers, bots or fixtures itself:

- Server workloads (`weak-hw`, `scaling`, `vps`) call [`tools.coreperf.profiles`](PROFILES.md)
  once per cell, so **TPS, MSPT p50/p95/p99/max and RSS keep exactly the definitions in
  [PROFILES.md](PROFILES.md#windows-and-metric-definitions)** (exact core-iteration `wall_us`,
  nearest-rank percentiles, TPS over the full window, RSS steady = median / peak = `VmHWM`).
  The bench only chooses the CPU set and memory cap and tabulates the receipts.
- Chunk generation (`worldgen`, and the generation line inside `weak-hw`/`vps`) runs
  `solaris-v2 world pregenerate` on a fresh world.

Every `result.json` records: the hardware (`lscpu`, `lscpu -e` CPU→core→socket map, RAM, kernel, OS,
governor, load average), the source (commit, binary SHA-256), the exact cpuset of every run and how
many **physical cores** it touches, and the memory cap. A run whose window is shorter than the
canonical 30 s warmup + 120 s measurement, or that used `--smoke`, is marked
`publication_evidence: false` and the markdown starts with a SMOKE banner.

## CPU counts are logical CPUs

The reference host has 6 physical cores / 12 logical CPUs with SMT siblings `0-1, 2-3, … 10-11`.
Therefore **`0-5` is 3 physical cores, not 6**. Two series exist and every table says which one it is:

| mode | server cpuset for N | meaning |
|---|---|---|
| `logical` (default) | first N logical CPUs: `0`, `0,1`, `0-3`, `0-5`, `0-7`, `0-11` | what a "N vCPU" VPS looks like: SMT siblings included, so 2 = one physical core |
| `physical` | one CPU per physical core: `0`, `0,2`, `0,2,4,6`, `0,2,4,6,8,10` | N real cores; counts above the physical core count are skipped |

Protocol bots get the remaining CPUs, preferring physical cores the server does not touch; when
the server takes the whole pool they **share** it (the table says so).

## Prerequisites

Linux, Python 3.11+, `taskset`, `lscpu`, `lsblk`, `findmnt`, cgroup v2, `systemd-run` (a user
session, or root) for the memory cap, and the imported 26.3 content cache
(`solaris-v2 content import --version 26.3 --download`; the dir holding `version.json`, passed
as `--vanilla-data-dir` if it is not `./data/vanilla` or `~/.local/share/solaris/content/26.3`).
`/usr/bin/time` (GNU time) is optional: it adds peak RSS and CPU-ms/chunk to the worldgen tables.
For comparable runs use the `performance` CPU governor (`cpupower frequency-set -g performance`);
the governor is recorded.

Build once (the bench never builds), non-PGO release, exactly as in
[PROFILES.md](PROFILES.md#build-once-and-retain-provenance):

```sh
CARGO_TARGET_DIR=<dir> cargo build --release --bin solaris-v2 --bin player-scale-bots -j 2
```

Defaults: `--server-bin`/`--bots-bin` = `$CARGO_TARGET_DIR/release/…` or `./target/release/…`.
Results go to a new `.analysis/bench/<UTC>-<command>-<pid>/` (`--out` to choose; never overwritten):
`result.json`, `result.md`, `build-receipt.json`, per-run receipts under `runs/` (the full
`tools.coreperf.profiles` receipt: `samples.jsonl`, bot reports, server config, logs).

On the shared agent host wrap any invocation in `.analysis/tools/mem-run.sh -m 8G` for a smoke, or
get Main's GO and use `-x` for a real window (the bench then sees all CPUs; under the shared lane
it only sees `0-7`). The memory-capped server/fixture scopes are created with `systemd-run
--scope`; pass `--slice solaris-heavy.slice` on the shared host so they count in that slice.

## `weak-hw`: the "$6 VPS" shape

```sh
python3 -m tools.bench weak-hw                        # 2 logical CPUs, 4G, idle + explore-20 + native-100
python3 -m tools.bench weak-hw --cores 1 --memory 2G --profiles idle,explore-20
python3 -m tools.bench weak-hw --cpu-mode physical --cores 2
```

The server is pinned with `taskset -c <cpuset>` and runs inside `systemd-run --scope -p
MemoryMax=<memory> -p MemorySwapMax=0` (a real RAM cap, no swap; the scope execs in place so the
profile's PID/RSS sampling is unchanged). Bots and the fixture builder are **not** inside the cap.
Worker threads follow the affinity automatically (`available_parallelism`), so the server really
sees N CPUs.

| option | default | |
|---|---|---|
| `--cores` / `--cpu-mode` | 2 / `logical` | server CPUs |
| `--memory` | `4G` | `none` disables the cap |
| `--profiles` | `idle,explore-20,native-100` | from [PROFILES.md](PROFILES.md#profiles-and-conditions); `native-100` = 100 bots + 100,000 mobs, may exceed 4G: an OOM kill is reported as a failed run, not hidden |
| `--worldgen-radius` | 256 | block radius of the pregenerate run under the *same* limits; 0 skips |

Fixtures are built **once** with the whole CPU pool and `--prepare-memory` (8G), outside any
limit, into the shared `--fixture-cache`; each limited run copies the pristine world.

Reported per profile: TPS, MSPT p50/p99/max, RSS steady/peak (server only), and
`resident chunks/s`. **That last column is a proxy**: the dashboard only exposes resident
chunks (reloads included), see [PROFILES.md](PROFILES.md#windows-and-metric-definitions). The true
**chunk generation rate** is the separate line "Chunk generation under the same limits": a fresh-world
`world pregenerate` of the square around block (0,0) with seed 42 (generated chunks / seconds from
the first generation progress line to completion; world open excluded, light bake and store writes
included).

## `scaling`: one workload at 1, 2, 4, 6, 8, 12 CPUs

```sh
python3 -m tools.bench scaling                                    # native-100, logical series
python3 -m tools.bench scaling --cpu-mode logical,physical        # plus the physical-core series
python3 -m tools.bench scaling --profile explore-20 --cores 1,2,4
```

Same profile, same seed, same window at every count; only the cpuset changes (`--memory none`
by default). `native-100` is the default because it is the only profile that is CPU-bound
(`explore-20` costs about 0.3 core, so it cannot scale). **Speedup = MSPT p50 at the lowest count ÷
MSPT p50 at N**; TPS cannot show scaling because it saturates at 20 once a run keeps up. Efficiency =
speedup ÷ (N ÷ lowest N). With the whole pool used (12 logical) the bots share CPUs with the
server; the table notes it. The `physical` series skips counts above the number of physical cores
(8 and 12 on the reference host) and says so.

## `worldgen`: chunks/s, optional vanilla head-to-head

```sh
python3 -m tools.bench worldgen --cores 1,2,4,6,8,12 --repeat 3
python3 -m tools.bench worldgen --cores 2 --radius 512 --vanilla-jar /path/to/server.jar
```

For each CPU count it runs `solaris-v2 world pregenerate --radius R --center=0,0` on a fresh world
(seed 42, `--seed` to change) `--repeat` times and reports the median generation rate, speedup
versus the lowest count, CPU-ms/chunk and peak RSS. `--radius 256` = 33×33 chunks.

If a Mojang **bundler** `server.jar` is available (`--vanilla-jar`, `$SOLARIS_VANILLA_JAR`, or
`./.analysis/server.jar`) and `java` is on the path, the **same chunk square** is generated by a
real vanilla server pinned to the same CPUs with the same memory cap (`-XX:ActiveProcessorCount=N`,
heap = 60% of the cap): `forceload add` tiles of ≤16×16 chunks, a generated one-function datapack
counts loaded chunks every 10 ticks and says `BENCH_READY`, then `save-all flush`. Vanilla chunks/s
= requested chunks ÷ seconds from the first forceload to the finished save (JVM startup excluded).
Vanilla also generates a halo of neighbours it needs, so per-requested-chunk it is flattered less
than Solaris is. No jar is ever copied or committed; without one the comparison is skipped with a
note. These are different engines doing different amounts of incidental work: read the ratio as a
rough head-to-head, not as parity (see [PROFILES.md](PROFILES.md#in-process-chunk-generation) for
the single-thread in-process generator figures).

## `vps`: a remote host, server there, bots here

Server on the VPS, protocol bots on this machine, so the bots do not steal the VPS's CPUs. The
VPS run is exactly `weak-hw` with the server relocated: `tools.coreperf.profiles --remote-host …`
swaps the local server process for a `RemoteSession` and keeps everything else (fixtures, bots,
open-loop schedule, exact `/load-metrics` ticks, cohort check after the run).

```sh
python3 -m tools.bench vps --host root@203.0.113.7 \
    --remote-data-dir /opt/solaris/.local/share/solaris/content/26.3 \
    --server-bin target/release/solaris-v2 --bots-bin target/release/player-scale-bots
# 26.3 / release tarball server instead of --server-bin:
#   --server-tarball solaris-x86_64-unknown-linux-gnu.tar.gz
# options: --ssh-options="-p 2222 -i KEY" (use `=`: the value starts with a dash)  --cores 2  --memory none
#          --profiles idle,explore-20
#          --worldgen-radius 256  --game-port 25590 --dash-port 25591  --bot-cpus 4-7  --tunnel-game
```

What it does, in order (all remote names are `solaris-bench-*`; **`/opt/solaris` and the `solaris`
unit are never written, stopped or restarted**, only the content cache is read):

1. Local preflight, then one ssh session (ControlMaster, key auth required, `BatchMode`). Remote
   preflight: `rsync taskset systemd-run systemctl tail timeout bash` present, content cache and
   `--remote-dir` (default `/var/tmp`) usable, the game/dashboard ports free, binary glibc ≤ remote
   glibc. Hardware of the **VPS** (lscpu, RAM, kernel, load, busiest processes, state of the `solaris`
   unit) goes into the result; stop the live unit first or the numbers are contaminated.
2. The fixture is built locally once (as everywhere), then per run: `rsync` binary, world fixture and
   `ops.json` into `/var/tmp/solaris-bench-<pid>-<time>/`, write a rewritten `server.toml` (game
   `0.0.0.0:25590`, dashboard `127.0.0.1:25591`), and start **one transient systemd unit**
   `solaris-bench-<pid>-<time>` (`systemd-run --remain-after-exit`, `MemoryMax`/`MemorySwapMax=0`
   when `--memory` is set, `taskset -c <remote cpuset>`; `--user` when not root).
3. Wait on the remote log (`tail --pid`, no polling) for "dashboard bound and listening"; check the
   game port is reachable from here (otherwise rerun with `--tunnel-game`, which forwards bot
   traffic through ssh and costs sshd CPU on the VPS). The dashboard is only reached through an
   ssh port forward.
4. Bots run locally against `<ip>:25590` with the usual open-loop schedule. `/load-metrics` tick
   timestamps are on the **server's clock**; the median remote↔local clock offset (9 ssh round trips)
   and the round-trip time are applied to the window and stored in `runs/*/run/remote.json`. Server
   RSS is read from the remote `/proc/<pid>/status` each sample (`VmRSS`, `VmHWM`).
5. Stop: `SIGTERM` to the unit (final checkpoint, exit status recorded), the saved world is rsynced
   back for the usual cohort check, then the unit is stopped/reset and the remote directory removed.
   This runs in `finally`; **Ctrl-C / SIGTERM** make `tools.coreperf.profiles` cancel and run the
   same cleanup. If the controlling machine dies, remove leftovers with
   `systemctl stop 'solaris-bench-*'; rm -rf /var/tmp/solaris-bench-*` (on a non-root user, `--user`).

Default workloads on a VPS are `idle,explore-20`; add `native-100` explicitly if it fits. Latency
between bots and the VPS is real network latency; it does not change server-side MSPT (measured
inside the server) but does change what the bots can sustain, which the bot report records.
Not covered: the remote SMT topology is not probed (every vCPU counts as its own core), so
`--cpu-mode physical` on a VPS equals `logical`.

## Smoke checks (pipeline proof, not numbers)

`--smoke` shrinks windows to 5 s warmup + 25 s, uses the cheap defaults (`idle,explore-20`,
`scaling` on `explore-20` at 1,2 CPUs, a 64-block worldgen square) and marks the result not
publishable.

For `explore-20`, **ready is not merely logged in**. Three bot preparations run concurrently.
Each bot completes Login/Configuration, teleports to its own 256-block-spaced target, receives
the complete 17×17 chunk window (view distance 8), chooses a walkable runway, receives the
window at its landing position, and observes its own input-ordered preparation echo. The
world is not pregenerated at these exploration targets. Consequently the 300 s smoke deadline
also bounds thousands of cold chunk loads/generations, not just twenty protocol handshakes.
Keep this readiness fence: starting the offered-load clock before the views are usable would
change the workload. A longer readiness override is diagnostic, never evidence that the
300 s smoke bound passed; the canonical non-smoke readiness deadline is 1800 s.

When diagnosing admission on a busy shared host, enable `MC_CORE_PHASE_LOG` and compare
admission thread CPU microseconds per admitted chunk (`chunk admission cpu`, Linux) and
the batch's admitted count (`chunk admission ms`) on the same fixture and CPU allocation.
Thread CPU excludes descheduling; phase wall time and end-to-end ready time do not.
Use deterministic operation-count regressions to explain a solver-work reduction.
These diagnostics do not replace the quiet integrated readiness/TPS acceptance window.

**2026-10-28 admission diagnostic.** A matched 100 s cold `explore-20` run
(seed 42, server CPUs `8,9`, bot CPUs `10,11`, preparation concurrency 3)
compared the original policy/solver with dominance pruning plus real-tick slack:

| Diagnostic metric | Baseline | Candidate |
|---|---:|---:|
| Admission thread CPU per chunk, aggregate mean | 5,213 µs | 4,195 µs (-19.5%) |
| Admitted chunks / observed global tick | 0.996 | 1.602 (+60.7%) |
| Ticks admitting more than one chunk | 21 / 1,998 | 1,145 / 1,996 |
| Total chunks admitted | 1,991 | 3,199 |
| Seed face checks, one-chunk oracle regression | 4,742 | 1,272 (-73.2%) |

The deterministic 16-block-high emitter/sky fixture has the same 791 seed
cells before and after pruning and checks the full light oracle. The two live
runs hit the same fixture cache key
`9e874f55587142f5ddc9c78e13e42729cafc71bce4c7237022cb2be1749fd4a6`
and use the same immutable bot binary. Their different progress changes the
admitted-chunk distribution, so aggregate CPU is not an identical-chunk
comparison. Both hit the deliberately shortened 100 s all-client readiness
deadline; neither is a readiness pass or a quiet 2-/6-CPU acceptance result.
These figures predate the 26.3 rebase and the multi-world progress-reservation
correction; the corrected integrated tree needs its own acceptance measurement.
Receipts in the `pauline/weakhw-login` worktree:
`.analysis/admission-cpu-comparison.json`,
`.analysis/login-cpu-{baseline,candidate}/admission-cost-summary.json`,
`.analysis/test-admission-{before,final}.log`, and
`.analysis/validation-final.json`.

Run it under the memory-capped shared lane:

```sh
M=/home/kaiserroman/solaris/.analysis/tools/mem-run.sh
$M -m 8G python3 -m tools.bench weak-hw --smoke --slice solaris-heavy.slice
$M -m 8G python3 -m tools.bench scaling --smoke --cpu-mode logical,physical --slice solaris-heavy.slice
$M -m 8G python3 -m tools.bench worldgen --smoke --cores 1,2
```

Unit tests of the pure parts (CPU allocation, receipt normalisation, pregenerate parsing, remote
config rewrite): `python3 -m unittest tools.bench.test_bench`.

## Reading a result

`result.md` is the table to publish; `result.json` has everything (`runs[]` for `weak-hw`/`vps`,
`series.<mode>[]` for `scaling`, `results[]` for `worldgen`). Per run:
`status` (`ok`/`failed`/`skipped`), `error`, `tps`, `mspt_ms{p50,p95,p99,max}`,
`server_rss_mib{steady_median,peak}`, `cpus_server`, `physical_cores_touched`, `receipt` (path of
the full profile receipt). A failed run keeps its receipts and makes the command exit non-zero;
partial numbers are never reported as a success.

## Limits of these measurements

- One run per cell unless `--repeat` (worldgen only); the 120 s MSPT p99/max are not stable tail
  evidence at that length, as in PROFILES.md.
- Protocol bots are not graphical clients; the VPS run adds real network latency.
- The memory cap is cgroup `MemoryMax` with swap off, page cache included in the cgroup's accounting
  but not in the reported RSS.
- Governor and background load are recorded, not controlled.
- Vanilla head-to-head compares two different engines' wall time for the same chunk square.
