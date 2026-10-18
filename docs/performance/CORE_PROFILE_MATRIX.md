# Solaris core performance profiles

**Historical campaign:** preserved pre-cutover v1 profile contracts and
measurements. ADR 0014 Phase 5 deleted their runtime/profile commands; the
autoscale TOML and regional-owner defaults below are not native v2 settings.
Use `catalog-load` and its current contract in [AGENT_TOOLING.md](../AGENT_TOOLING.md)
for new public-protocol measurements. Historical results are not revalidated.

Status: frozen test contract for the M91/T07 performance campaign. These limits are selected before profile runs and must not be relaxed after seeing a result. A failed profile remains `degraded` or `blocked` until code or the declared hardware envelope changes in a separately reviewed commit.

## Provenance

- Contract base: `0cc648826a14f25f1aeae759f379d463337bd3bd` plus the T07-01 autoscale change in the same campaign checkpoint.
- Rust/Cargo: `1.94.1`.
- Calibration host: AMD Ryzen 5 7535HS, 6 cores / 12 threads, 32,076,632 KiB host RAM, Linux.
- Runtime build for the recorded calibration: debug, unoptimized + debuginfo.
- Vanilla sidecar: owner-local `data/vanilla`, mounted read-only into the campaign run.
- Calibration artifact: `.analysis/runs/t07-01-a/m52-debug-baseline.log`.
- Calibration result: 4 clients, 8 summons, 5,754 ms elapsed; chunk CPU peak 2, IO peak 1, result queue peak 1; `session_registry max_wait_us=121`, `max_hold_us=5,189`; `world_storage max_wait_us=11`, `max_hold_us=50`.

The calibration is a bounded balanced-profile checkpoint, not proof of the 20-client VD8 or VD32 profiles.

## Runtime profile defaults

These values are the authoritative defaults in `mc_net::AutoscalePolicy::for_profile`.

| Profile | VD range | Chunk send | Chunk load | Chunk generate | Tick target | First chunk target | Queue pressure | Memory pressure | Down/up hysteresis |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `low_end` | 4..8 | 4..8 | 8..16 | 4..12 | 50 ms | 2,500 ms | 70% | 85% | 2 / 8 observations |
| `balanced` | 6..10 | 8..16 | 16..64 | 8..32 | 50 ms | 1,500 ms | 75% | 85% | 3 / 10 observations |
| `high_end` | **8..32** | 16..32 | 32..96 | 16..64 | 50 ms | 1,000 ms | 80% | 90% | 4 / 12 observations |

Tick, queue, memory, first-chunk SLA, or slow-client pressure first yields deferred/random-tick work and then reduces bounded chunk throughput and view distance. Healthy observations restore one view-distance step at a time. `high_end` therefore supports a real `32 -> 8` degradation path and an `8 -> 32` recovery path without dropping below VD8.

## Frozen measurement envelopes

CPU and memory entries are harness/cgroup requirements, not TOML keys. Client/entity limits are workload inputs, not claims about production capacity.

| Profile | CPU quota | Memory limit | Clients | Initial VD / simulation distance | Active entity ceiling | Duration | Current evidence status |
|---|---:|---:|---:|---:|---:|---:|---|
| low | 2 vCPU | 4 GiB | 4 | 8 / 4 | 128 | 10 min | `degraded`: frozen, not run on the declared quota |
| balanced | 6 physical / 12 logical CPUs | 8 GiB | 20 | 8 / 8 | 256 | 20 min | `degraded`: focused 20-client VD8 debug/release gates passed on 2026-07-27, but the exact 20-minute and 8 GiB envelope was not run |
| high | 12 vCPU | 16 GiB | 20 | **32 / 8** | 512 | 20 min | `degraded`: VD32 runtime contract tested; end-to-end VD32 load not yet run |

The entity ceiling counts non-item server entities at the measurement checkpoint. Item drops, XP orbs, projectiles, and primed TNT are reported separately so a transient burst cannot hide inside the mob count.

## Valid profile configuration

The production example intentionally starts at VD32 with conservative initial chunk rates and lets runtime control raise or lower throughput independently.

```toml
[server]
max_players = 20
view_distance = 32
simulation_distance = 8

[chunk_pipeline]
chunk_send_rate = 8
chunk_load_rate = 16
chunk_generate_rate = 16
chunk_prepare_budget_ms = 0
chunk_prepare_batch_size = 8
chunk_result_queue_size = 64
region_cache_size = 9

[simulation]
random_tick_speed = 5
save_interval_ticks = 1200
friendly_spawn_interval_ticks = 400
hostile_spawn_interval_ticks = 20

[autoscale]
enabled = true
profile = "high_end"
min_view_distance = 8
max_view_distance = 32
target_tick_ms = 50
target_first_chunk_ms = 1000
scale_down_after_ticks = 4
scale_up_after_ticks = 12
```

`simulation_distance` stays at 8 for the VD32 profile. Raising simulation distance to 32 is a different workload and must not be silently included in a view-distance result.

## Required metrics

Every profile artifact must include:

- commit, dirty flag, build mode, Rust version, hardware, CPU quota, memory limit, seed, duration, clients, configured/effective VD, simulation distance, entity counts;
- tick and stage `samples/p50/p95/p99/max` for total tick, world time, entity goals, entity physics, entity dispatch, random tick, block tick, fluid tick, entity save, campfire, hostile attacks, and breeding;
- first/ring/full chunk latency p50/p95/p99/max and per-stage fetch, encode, light, compression/frame, and socket-write maxima;
- `active_cpu`, `active_io`, permit maxima, result queue depth/capacity, stop reasons and cancellation counts;
- lock wait/hold counts, totals and maxima for `world_storage`, `session_registry`, `chunk_prepare`, `player_persistence`, and `save_all_flush`;
- RSS/effective memory limit and percentage, dirty chunks/flush timings, prepared/region cache bytes and evictions;
- reliable drops/retries/in-flight maximum, best-effort drops, write timeouts, and slow-client pressure sheds.

## Pass, degraded, and blocked rules

A profile is `pass` only when all mandatory metrics exist and:

- sustained TPS is greater than 18 and tick p95/p99 are at or below 50,000 us;
- no unexplained tick exceeds 150,000 us; the stricter O2 VD8 gate keeps tick max at or below 50,000 us;
- first-chunk p99 is within the profile target; full-window latency is recorded and monotonically ordered after first/ring milestones;
- normal-reader workloads have zero reliable command drops, zero write timeouts, and zero slow-client sheds;
- result queue depth remains below the profile pressure threshold and CPU/IO permits are never exceeded;
- memory remains below the profile pressure threshold and no allocation/queue success relies on OOM recovery;
- `session_registry` and `world_storage` max hold remain below 250,000 us; any 150,000 us or larger tick/lock event is individually explained;
- shutdown drains CPU/IO work, flushes dirty chunks, and leaves no duplicate-pressure or stuck retry state.

A run is `degraded` when it completes but misses a budget, lacks one non-authoritative metric, uses debug build where release evidence is required, or runs below the declared clients/VD/entity/duration envelope. It is `blocked` when required sidecars are absent, the process fails, a required metric/artifact is missing, reliable state is lost, or shutdown does not drain.

## Fixed execution order

1. Validate config and sidecar paths.
2. Record idle and solo baselines.
3. Run the exact profile envelope without changing budgets.
4. On failure, preserve the artifact and optimize the measured dominant stage only.
5. Re-run the same profile and compare against the frozen contract.
6. Never lower clients, VD, entities, duration, or percentile budgets in the same change that claims an optimization.

## Pinned-core player-scale diagnostic (2026-10-02)

`player-scale` uses the Rust protocol clients in `mc-test-harness`, not real Minecraft clients. The standalone runner starts independent release server and bot processes, fixes VD6, disables autoscale, and creates a fresh `vanilla_like` world for each rung. The owner acceptance run uses `--single-core`: server CPU `2`, `[chunk_pipeline] worker_threads = 1`, bots CPUs `4-11`, and no workload on SMT sibling CPU `3`. Without this flag, the diagnostic default remains server CPUs `0,2`. Clients start 256 blocks apart and exercise movement, sprint/jump, creative break/place, keepalive, teleport confirmation, chunk-batch acknowledgement, and occasional chat. Each rung records the actual release binary SHA256 values.

After the integration owner's exclusive-window approval, run:

```sh
/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -x -m 10G \
  taskset -c 4-11 python3 tools/harness/backends/player_scale.py --single-core \
  --out /home/kaiserroman/solaris/.analysis/codex-logs/player-scale/ladder
```

The ordinary unprofiled ladder is `50,100,250,500,1000`, with 200 warmup ticks and 1200 measured ticks per rung; it stops at the first failure. `--seed 42 --fixture-summary /home/kaiserroman/solaris/.analysis/codex-logs/player-scale/terrain-fixture.json` instead selects the owner-approved dry-anchor fixture and the `50,100,250,500,800` ladder. The bounded offline search checked seeds 0–49 on the original 1000-anchor grid; seed 42 was best with 805 anchors at or above sea level. Bots use those dry coordinates in original grid order, not a spawn cluster or an artificial platform. Terrain height alone does not guarantee a safe full-cube surface. A 1000-player result is not certifiable on this 805-anchor fixture.

Receipts include observed simulation-tick progress, source-aligned tick percentiles, separate server/bot CPU and RSS, wire bytes per player, client acknowledgements/errors, generated-chunk throughput, native stage profiles, generation/load preparation backlog, and selected anchor count. The backlog is not a pure generation-queue metric. Use `--clients N --profile perf` only for a separate diagnostic rerun of the first failing rung; profiled results cannot establish capacity. Shortened tick windows are labelled `smoke_only`.

Any action acknowledgement or observation timeout rejects a rung, including a timeout appearing only in final bot counters. Canonical harness receipts map `smoke_only` to non-passing `prepared`, never `PASS`. `fresh_generated_chunks` counts unique newly committed Anvil chunk locations from the per-rung `generated-chunk-header-diff.json`, including centre and neighbour generation across the whole rung. It excludes staged/unflushed chunks and is therefore a lower bound after forced termination. This is distinct from measurement-window `generated_total`, which counts only centre admissions, and from native generation-scope invocations.

The initial two-client VD6 generated-world smoke exposed a production residency defect: the old 289-slot limit stalled two disjoint 169-chunk windows despite only 7,384,303 resident bytes against a 2,424,307,712-byte budget. Production residency now grows on demand under the unchanged byte budget, without a view-distance-derived slot limit or up-front chunk-table reservation. Explicit count-limited fixtures remain supported. Admission blocked by retained chunks is terminal and disconnects the requesting connection instead of leaving subscribers pending indefinitely; flushable dirty pressure remains retryable.

The release protocol regression streams both complete disjoint VD6 windows from a persisted air-world fixture while the first client remains connected, and separately proves prompt connection termination under retained count and byte limits. These three checks passed. The subsequent generated-world two-client single-core smoke also completed with zero disconnects/errors and three observed break/place acknowledgement-and-update pairs. Its shortened window is correctness evidence, not capacity evidence.

| Main-tree attempt, generator revision 24 | Evidence scope | Tick p50 / p95 | Server CPU (one-core basis) | Peak server RSS | Reported generated chunks/s |
|---|---|---:|---:|---:|---:|
| 2 clients | Shortened smoke; rolling window includes warmup | 0.861 / 1.958 ms | 25.01% during sampled measurement | 233.89 MiB | 0 during sampled measurement |
| 50 clients | Failed readiness; not a post-warmup acceptance window | 8.593 / 16.150 ms | 99.65% across readiness | 605.66 MiB | 0.92 across readiness |

The first 50-client attempt stopped because bot 3 found no safe ground within the driver's old 15-block search radius, not because a server capacity criterion failed. Terrain selection now searches all already-loaded view summaries deterministically. The failed attempt streamed 42.90 chunk packets/s; this network counter is distinct from the reported generation counter. Receipts are under `.analysis/codex-logs/player-scale/smoke/20261002T104033-31f3af94` and `ladder/20261002T104142-10fa3890`, with a compact summary in `first-main-attempt.json`. No completed 50–1000-player acceptance result or VPS-capacity claim is recorded by these attempts. Scale acceptance requires the corrected unprofiled ladder on the consolidated generator revision.

Cleanup of that failed attempt also missed the server's existing 6-second shutdown-drain bound: `final save completed late`, with server exit code 1. This separate shutdown failure is preserved rather than hidden or addressed by increasing the bound.

### Seed-42 dry-anchor result

| Dry-anchor attempt, generator revision 24 | Evidence scope | Tick p50 / p95 | Server CPU (one-core basis) | Peak server RSS | Reported generated chunks/s |
|---|---|---:|---:|---:|---:|
| 2 clients | Shortened smoke; rolling window includes warmup | 0.970 / 9.506 ms | 31.79% during sampled measurement | 226.18 MiB | 0 during sampled measurement |
| 50 clients | Cold-world readiness failure; no player-capacity acceptance window | 23.747 / 84.388 ms | 99.83% across 296.017 sampled readiness seconds | 501.28 MiB | 0.426 across sampled readiness |

The dry-anchor smoke completed with zero disconnects/action timeouts, 399 movement packets, and three observed break/place acknowledgement-and-update pairs. Client-requested distance is not authoritative accepted travel: 86 teleport confirmations were also observed. The 50-client run admitted all 50 connections but reached zero complete client view windows before the unchanged 300-second readiness deadline. It streamed 15.303 chunk packets/s across sampled readiness; final bots received 4,592 chunk packets. There was no wet-anchor terrain-selection failure. The stopped ladder did not run 100–800 clients and establishes neither player capacity nor VPS capacity.

The saved native failure profile spans 297.006 seconds and attributes 140.475 CPU-seconds to chunk generation (47.38% of process CPU), 111.419 to saving (37.58%), 12.541 to lighting (4.23%), and 10.740 to simulation (3.62%). Its requested allocation interval is 299,669,028,933 bytes across 1,238,980,895 allocations, not live memory. The latest sampled rolling `block_tick` p95/max is 78.195/531.946 ms; the later native capture has a different rolling p95 of 62.237 ms with the same maximum. Aggregate maxima alone do not establish correlation with the saving scope's 2.707-second maximum CPU execution. This is cold-world preparation/saving evidence, not a completed moving-player capacity result. Server shutdown returned zero in this attempt, so the earlier late-save shutdown bug was not reproduced here.

Artifacts: `fixture-smoke/20261002T113823-aecc6223`, `fixture-ladder/20261002T113842-30bb3570`, `terrain-fixture.json`, and compact `dry-anchor-main-attempt.json`, all under `.analysis/codex-logs/player-scale/`. Both dry-anchor runs used server SHA256 `ba1d1192d298a3260970ba028843cdd66cc4175b94404a3fb0015e580a67d23c` and bot SHA256 `fbcd0fe1e8adcfc9cd0f475b6b4d1667b1171dcaa59b36431c45e8c2a4a08f8b`.

### Existing-command pregeneration

The existing Anvil `mc-server --config CONFIG pregenerate --from X,Z --to X,Z` command prepared seed 42 without adding a generator or modifying storage code. The 805 dry anchors require 136,045 disjoint VD6 view chunks, or 181,125 with the existing one-chunk lighting border. Consecutive dry anchors in each row were coalesced into 99 supported rectangles, including the one-chunk gaps between adjacent light-bordered windows: 191,715 requested chunks plus 225 startup spawn chunks.

All 191,940 unique chunks are present in 268 Anvil region location headers; both dry-anchor coverage checks have zero missing chunks. Region files occupy 788,389,888 bytes. Accumulated CLI wall time, including startup/flush and the preserved 0.006-second initial configuration failure, was 1,064.007 seconds: 180.182 requested chunks/s. Preparation used four workers on CPUs `4-11`, `mem-run` effective MemoryMax `5G`, MemorySwapMax `1G`, CPUQuota `600%`, and nice `19`; it was offline preparation, not a capacity measurement.

The command receipt and per-rectangle logs are in `.analysis/codex-logs/player-scale/pregenerated-seed42/`, with the rectangle plan in `pregenerate-plan.json`. Runtime measurements use `--pregenerated-world /home/kaiserroman/solaris/.analysis/codex-logs/player-scale/pregenerated-seed42/world`: the backend copies this source into an independent world for each rung. The frozen `ba1d1192…` server artifact permits a baseline before saving optimizations; native `world prepare-region` is not a substitute because it stores derived-region metadata, not Anvil chunks.

### Pregenerated frozen-server baseline

| Clients | Tick p50 / p95 / p99 | Actual TPS | Server CPU, one-core basis | Peak server RSS | Saving / generation CPU share | Outcome |
|---:|---:|---:|---:|---:|---:|---|
| 50 | 51.093 / 148.355 / 652.508 ms | 12.173 | 99.51% | 826.17 MiB | 41.05% / 21.58% | Failed: 15 action observation timeouts, TPS below 19, p95 over 50 ms; forced server shutdown |
| 100 / 250 / 500 / 800 | Not run | — | — | — | — | Ladder stopped at first failure |

All 50 bots reached readiness and the final 1,200-tick percentile window is entirely post-warmup. The measurement spans 98.989 seconds; native CPU shares use its corresponding 98.934-second profile capture. The whole-run committed header difference is 1,212 fresh chunks, all outside the original prepared light-bordered windows, at nearest-anchor Chebyshev distances of 8–13 chunks. Walking legitimately expands the loaded area; no route restriction was added. Native generation records 900 generator invocations during measurement, including auxiliary lighting-neighbour generation, while the centre-only dashboard counter increased by 21. Disk decoding is separately attributed.

The server exceeded the unchanged harness 12-second shutdown grace and received SIGKILL. Matched logs show shutdown at 12:43:50.779, all sessions unregistered by 12:43:51.056, the outer six-second classification deadline at 12:43:56.779, a periodic checkpoint flushing 8,087 chunks in 8.127 seconds, and its completion/ticker fence at 12:43:58.052/58.058. No final-save completion is logged; this does not prove final save never started. The owner accepted this as the baseline before the subsequently integrated saving fix.

Receipt: `.analysis/codex-logs/player-scale/prepared-ladder/20261002T123923-a38f1b16/clients-50/receipt.json`; CPU profile: sibling `measurement-end-profile.json`; fresh-location witness: sibling `generated-chunk-header-diff.json`. The run used the frozen `ba1d1192…` server on CPU `2`, one chunk worker, bots/orchestrator CPUs `4-11`, unused sibling `3`, and owner-approved ordinary `mem-run` admission (effective `5G`, CPUQuota `600%`, nice `19`). No larger-player acceptance or VPS-capacity claim follows from this failed rung.

### Pregenerated combined-release rerun

The owner-authorized rerun used the exact release copy `.analysis/codex-logs/main-build/bin/mc-server-release-nativeplat`, SHA256 `51362c9923f0251f51d889b1008a6e4e54828c8a34945e397dd3ed4c1bcc4046`, after green combined-tree L2 (`20261002T143348`). This includes saving, absorption, and tick changes; it is not an isolated saving-only comparison. The bots remained the frozen `fbcd0fe1…` build. The fixture, independent pregenerated-world copy, VD6, routes, server CPU `2`, one chunk worker, unused SMT sibling `3`, bots/orchestrator CPUs `4-11`, and ordinary `mem-run` limits were unchanged. `SOLARIS_HARNESS_LOG_STDOUT=1` retained a run-local server log.

| Clients | Tick p50 / p95 / p99 | Actual TPS | Server CPU, one-core basis | Peak server RSS | Saving / generation CPU share | Outcome |
|---:|---:|---:|---:|---:|---:|---|
| 50 | 39.384 / 113.523 / 582.774 ms | 14.006 | 99.74% | 803.16 MiB | 38.35% / 21.52% | Failed: 15 action observation timeouts, TPS below 19, p95 over 50 ms; forced server shutdown |
| 100 / 250 / 500 / 800 | Not run | — | — | — | — | Ladder stopped at first failure |

All 50 bots reached readiness. The authoritative 1,200-tick rolling window covers ticks 2,301–3,500; actual progress is separately measured as 1,193 ticks in 85.180 seconds. The tick maximum is 1,070.354 ms. Native CPU accounting covers 85.228 seconds and 85.002 process-CPU seconds: saving takes 32.601 seconds (maximum synchronous execution 4.965 seconds), generation 18.291 seconds across 621 calls, simulation 12.525 seconds (14.73%), disk decoding 4.414 seconds (5.19%), and lighting 2.589 seconds (3.05%). Saving remains the largest attributed CPU consumer, with repeated mass flushes.

The automatic per-rung Anvil location-header witness records 191,940 initial and 193,003 final stored chunks: **1,063 unique fresh committed chunks**, including centre and neighbour generation over the entire rung. Forced shutdown makes this a lower bound; it is neither the 621 measurement-window generator calls nor the centre-only dashboard delta of six. The final bot report has zero ACK timeouts and zero disconnects. All 15 observation gaps concern breaks: 1,072 break ACKs versus 1,057 observed break updates, while all 966 place ACKs have matching observations. These aggregates do not distinguish overload-delayed updates from rejected, lost, or observer-missed break updates; they do not demonstrate a missing-ACK bug. No reliable-backlog shedding/overflow messages were found in the retained server log.

Shutdown again exceeded the unchanged 12-second harness grace and ended in SIGKILL (server return code `-9`; bots exited `0`). The log records shutdown at 16:13:02.272, all sessions unregistered by 16:13:02.482, a 7,616-chunk periodic flush taking 6.739 seconds, periodic-save completion/ticker fence at 16:13:07.797/07.800, and the outer six-second deadline at 16:13:08.273. Unlike the frozen baseline, final-drain chunk-flush completion is visible: 7,825 chunks in 5.835 seconds at 16:13:13.639, with zero remaining dirty chunks. No final save-all completion or clean process exit is recorded before forced termination. The integration owner reopened the mass-flush issue for further saving work; no timeout was increased.

Artifacts: `.analysis/codex-logs/player-scale/prepared-after-savefix/20261002T160929-96592e63/clients-50/receipt.json`, with sibling `measurement-end-profile.json`, `generated-chunk-header-diff.json`, and `server.log`. This failed one-core diagnostic does not establish larger-player acceptance, two-core VPS capacity, or the original 1,000-player goal.

### Pregenerated all-fixes wandering rerun

The next owner-approved run used `.analysis/codex-logs/main-build/bin/mc-server-release-l2green2`, SHA256 `c2b3770a4f554646c1556eeb59a317a98f1b99bf5d557aeaf89549e0868e9c94`, after combined L2 `20261002T190201`. Saving metadata/journal, reliable-backlog, chunk-loading, tick, and content fixes are combined; this is not a single-change causal comparison. The frozen `fbcd0fe1…` bots, seed-42 independent prepared-world copy, original wandering routes, VD6, server CPU `2`/one chunk worker, bots/orchestrator CPUs `4-11`, and ordinary `mem-run` limits were retained.

| Clients | Tick p50 / p95 / p99 | Actual TPS | Server CPU, one-core basis | Peak server RSS | Saving / generation CPU share | Shutdown / outcome |
|---:|---:|---:|---:|---:|---:|---|
| 50 | 32.975 / 116.441 / 422.866 ms | 15.177 | 99.69% | 668.30 MiB | 20.88% / 24.67% | No forced kill, server exit `1` after late final save; failed tick/TPS/action-observation acceptance |
| 100 / 250 / 500 / 800 | Not run | — | — | — | — | Ladder stopped at first failure |

All 50 clients became ready in about 98.16 bot-seconds. The authoritative rolling window is ticks 2,101–3,300; actual progress is 1,199 ticks over 79.003 seconds. Tick maximum is 887.858 ms. Native interval CPU is 78.763 seconds over 79.001 seconds: generation 19.433 seconds, saving 16.446, simulation 14.815 (18.81%), disk decoding 7.435 (9.44%), network 4.575 (5.81%), lighting 3.067 (3.89%), and uninstrumented CPU 11.075 (14.06%). The latter is **process execution CPU**, not idle or suspended I/O time; those are excluded by the accounting clocks.

Per-rung receipts now include raw `/proc/stat` CPU2/CPU3 snapshots and foreign-load attribution. Across this measurement CPU2's busy-time residual after subtracting the solely CPU2-pinned server is 0.0633%; CPU3, with no harness process pinned to it, is 6.9365% busy. Busy includes steal and excludes idle/iowait; jiffy quantization and snapshot skew remain explicit, with negative raw residuals preserved and displayed percentages clamped at zero. A multi-core server's CPU time is not falsely assigned to an individual core.

Measured saving is separate from shutdown. The 26 periodic flush completions inside the sampled interval flushed 5,306 chunks (67.162 chunks/s); individual counts are retained in `periodic-checkpoints.json`. Their inclusive `flush_us` sum is 50.603 wall-seconds, or 0.641 wall-seconds per measured second; the first flush straddles the interval start. Wall times are not additive CPU shares. Native saving consumes 0.208 CPU-seconds per measured wall-second, excluding the later shutdown flush. This binary has no live dirty-source counters; SavePerf's historical attribution is not presented as this run's reasons.

Final header difference is 191,940→193,041: **1,101 unique fresh committed chunks over the whole rung**, not the 687 measurement generator calls. The final report has 24 observation timeouts, all breaks (961 ACKs/937 observations); place ACKs/observations are 862/862, with zero ACK timeouts/disconnects. Thus neither stationary steady-state capacity nor a missing-ACK cause follows from the run.

Shutdown has no forced termination, but remains an error under the unchanged six-second drain bound. All sessions are gone at 19:39:45.058; the ticker is fenced at 19:39:45.096. Final drain flushes 8,158 chunks in 5.719 seconds, reaching dirty count zero at 19:39:50.818; the deadline error is logged at 19:39:50.940 and final save-all completes at 19:39:51.616 before server exit `1`. The storage policy deliberately keeps accumulated `InhabitedTime` pending without ordinary periodic bulk promotion; `persist_inhabited_time_tail` calls `prepare_inhabited_time_save` at final persistence, promoting pending clean chunks, while released-view eviction can promote them earlier. This explains a final metadata tail despite ordinary checkpoints, but the exact reason split of these 8,158 chunks was not instrumented. The late-drain bug is not hidden by a timeout increase.

Artifacts: `.analysis/codex-logs/player-scale/prepared-after-allfixes/20261002T193628-ce776395/clients-50/`, containing `receipt.json`, `measurement-end-profile.json`, `generated-chunk-header-diff.json`, `periodic-checkpoints.json`, and `server.log`. This failed one-core wandering diagnostic does not establish a two-core VPS or 1,000-player capacity.

### Bounded-route fixture correction

The backend's opt-in `--steady-state` now passes the bots' opt-in `--anchor-chunk-walk`: both safe landing and normal walking stay inside each original anchor chunk. Exploration and authored-arena defaults are unchanged. The backend validates the committed source footprint before startup and rejects fresh committed chunks in a steady-state receipt.

The first bounded attempt, `.analysis/codex-logs/player-scale/steady-state-after-allfixes/20261002T195638-e23106bc/clients-50/`, used the same `c2b3770a…` server with newly built bot SHA256 `36da1c058e2bfb7d60428644c4551cc887a5bee50f206a9b5c7a351aa051b1ea`. All 50 clients became ready in 61.31 seconds. Measured bot chunk crossings and newly received chunks were both zero, and all original VD6+one-border source footprints were complete. Nevertheless the whole-rung header witness records **1,350 fresh chunks, every one exactly eight chunks from the nearest selected anchor**. This disproves the claim that all fresh generation in the earlier measurements was walking alone: prewarm uses radius VD+1, then its lighting neighbourhood can request another chunk beyond it.

This is therefore an incomplete-preparation diagnostic, not accepted steady state. Tick p50/p95/p99 is 26.284/95.304/347.026 ms; actual TPS is 15.837, server CPU 81.50%, and RSS 652.25 MiB. Native generation is 24.810 CPU-seconds across 1,056 calls (39.56% of process CPU); saving is 6.951 seconds (11.08%). The final report has six break-observation timeouts, zero ACK timeouts/disconnects, and no forced kill. Final save serializes 8,251 promoted dirty chunks in 6.522 seconds and completes late with exit `1`. System/user process CPU is 3.77/58.96 seconds; the 8.995 seconds of uninstrumented execution CPU cannot all be system CPU and is not idle/I/O-wait time.

The corrected source footprint is VD6+two preparation/light borders, covering observed stationary radius eight without disabling the production prewarmer or changing route timeouts. Its pregeneration and accepted/rejected rerun evidence are recorded separately rather than relabelling this failed attempt.

### Complete-halo steady-state result

Offline preparation copied the pristine original source and extended all 99 supported pregeneration rectangles by one chunk on each side. The existing command skipped stored chunks, adding exactly 17,428 chunks in 241.598 CLI-work seconds on CPUs `4-11` with four workers. The independent source now has 209,368 committed chunks, zero missing required positions, and unchanged original source location headers. Evidence is `.analysis/codex-logs/player-scale/pregenerated-seed42-halo8/{plan,receipt}.json`; this preparation is not a capacity measurement.

| Clients | Tick p50 / p95 / p99 | Actual TPS | Server CPU / RSS | Measured saving / uninstrumented CPU share | Fresh committed chunks | Outcome |
|---:|---:|---:|---:|---:|---:|---|
| 50 | 19.749 / 52.866 / 76.082 ms | 19.467 | 67.13% / 663.05 MiB | 22.72% / 23.83% | **0** | Failed: four final break-observation timeouts, p95 over 50 ms, late final save exit `1` |
| 100 / 250 / 500 | Not run | — | — | — | — | Stop at first failed rung |

The `c2b3770a…` server and `36da1c05…` bounded bots are unchanged from the preceding attempt; only the independently prepared source coverage changes. All 50 clients are ready in 64.83 bot-seconds. Authoritative percentiles cover ticks 1,501–2,700, with maximum 178.678 ms; sampled actual progress is 1,191 ticks in 61.180 seconds. Both the whole-rung Anvil header difference and measurement-window generator calls/CPU are zero. Client chunk crossings/new chunk packets and preparation/subscriber backlog are also zero during measurement: this **does** exercise the requested spatially bounded, generation-free workload, but does not pass player-capacity acceptance.

The native CPU interval has 41.061 execution seconds over 61.180 wall-seconds: simulation 13.394 seconds (32.62%), saving 9.330 (22.72%), lighting 3.057 (7.45%), disk decoding 2.480 (6.04%), network 2.314 (5.64%), and uninstrumented execution 9.783 (23.83%). Sampled process user/system CPU is 37.77/3.30 seconds. Even attributing all system CPU to the uninstrumented bucket leaves roughly 6.48 seconds of user execution; it cannot be explained as idle or suspended I/O time. Foreign runnable/busy share is 0.7028% on CPU2 and 5.6555% on CPU3. These `/proc/stat` busy shares exclude iowait and must not be equated to a reported idle-plus-iowait occupancy.

The 14 periodic flush completions inside measurement flush **286, 412, 552, 413, 274, 90, 84, 88, 123, 92, 106, 122, 116, 137** chunks: 2,895 total, or 47.320 chunks/s. Inclusive flush wall time totals 30.182 seconds (0.493 wall-seconds per measured second); native saving CPU is 0.153 CPU-seconds per measured second, separately from shutdown. Current dirty-source attribution is still required: historical lifecycle dirty-reason counters are not substituted for this zero-generation run.

Final observations are break 685/689 ACKs and place 641/641 ACKs, with zero ACK timeouts/disconnects; aggregate counters alone do not identify why the four breaks lacked matching observations. No forced termination occurs. A periodic save completes/ticker fences at 20:24:40.590/40.603; final metadata-tail promotion flushes 8,260 chunks in 6.453 seconds, reaches dirty count zero at 20:24:47.059, and final save-all completes at 20:24:48.923 after the unchanged six-second deadline, producing server exit `1`. Final metadata serialization remains a documented shutdown failure rather than measured periodic CPU.

Receipt, native profile, header witness, periodic completion records, and log: `.analysis/codex-logs/player-scale/steady-state-halo8/20261002T202217-ec154702/clients-50/`. Profiling runs, when used for remaining CPU attribution, are separate diagnostics and never promoted to an unprofiled VPS/1,000-player capacity claim.

### Sampled user-CPU attribution

A separate 50-client diagnostic uses the same sealed `c2b3770a…` server and complete-halo fixture: `.analysis/codex-logs/player-scale/steady-state-halo8-perf/20261002T203601-d6e58bb3/clients-50/`. It remains generation-free but is not an unprofiled baseline. Actual TPS is 19.662; tick p50/p95/p99 is 18.861/54.073/73.187 ms. Server CPU is 66.80%, with foreign busy shares CPU2 0.6729% and CPU3 4.9236%. Native uninstrumented execution is 9.567 CPU-seconds, **23.53%** of 40.666 process-CPU seconds; sampled user/system time is 36.48/4.22 seconds.

`perf` records user cycles at 199 Hz using the monotonic clock; analysis is restricted to the exact receipt measurement interval, 574965.884854933–575026.815262961 monotonic seconds. About 8,000 measured samples identify these **self/leaf shares of sampled user cycles**, not exact shares of the uninstrumented CPU residual:

| Measured symbol | Sampled user-cycle self share |
|---|---:|
| `WorldReadView::snapshot_chunks` | 9.44% |
| `WorldMutationView::backfill_hopper_ticks` | 4.34% |
| `WorldReadView::brewing_snapshots` | 4.16% |
| `ScheduledTickView::fluid_due` | 2.51% |
| `due_scheduled_block_ticks` | 1.68% |
| `InhabitedTimeAccumulator::observe_tick` | 1.37% |

The sealed source snapshot (`6cfe026ea621131ca7b2dbde7de57d0fd01e67a0`) confirms a concrete accounting gap: `spawn_scheduled_block_tick_job` spawns a separate task without a CPU scope, outside the entity ticker's `Simulation`-scoped future. That job constructs the whole loaded-position list, backfills hopper ticks, snapshots all loaded chunks, then selects due ticks. Brewing also scans the loaded list. These source-visible world/station scans are real execution work, not idle or suspended I/O; not every sample of a shared helper such as `snapshot_chunks` can be assigned exclusively to this task.

Caller chains from the 32-KiB DWARF capture are unreliable (mostly self-only with invalid return frames), so no exact caller-based partition of the 23.53% residual is claimed. The whole capture reports six lost samples; loss within the selected interval is not separately established. The integration owner assigned the scheduled-scan fix based on the validated leaf hotspots and source-visible task boundary, and cancelled the optional frame-pointer rebuild. No permanent CPU-scope instrumentation or profiler-flag changes were retained. Generic dirty-source instrumentation was likewise deferred by the owner in favour of the independently owned Anvil region-image write-amplification fix.

### Worldgen revision 25 fixture regeneration (2026-10-03)

The scale fixtures above are **worldgen revision 24** (seed 42, 805 dry anchors). Revision 25 changed overworld terrain to the native 26.1.2 noise router, so the rev-24 pregenerated world is refused at startup ("use a fresh world_dir") and the old dry-anchor receipt no longer describes the terrain. A scale run on the `mc-server-matrix-h` release (SHA256 `1388ce6d1f4491530ba2083782835c9910f8fbd2d6a45dee5001dc90fcd4b609`) therefore needs a fixture regenerated for revision 25; no rev-25 scale number exists yet.

The same bounded protocol (seeds 0–49, the 1000-anchor 32×32 grid at spacing 256 from origin 1024, `surface_y >= 63` via `TerrainGenerator::surface_height` with the vanilla router, no village plans, no chunk materialization) now selects **seed 19 with 628 dry anchors** (`terrain-fixture-rev25.json`, worldgen revision recorded in the file). Seed 42 no longer passes the protocol. 628 dry anchors cover the 50/100/250/500 rungs but not 800 and never 1000. Terrain height alone does not guarantee a safe landing surface; the village-plan height analogue is not sampled.

The seed-19 world's `world.json` spawn is `(0, -320)`, not the origin, so harness fixtures that assume an origin spawn do not apply to it.

Offline preparation with matrix-h's own `pregenerate` CLI (one rectangle of anchor chunk ±8 per contiguous dry-anchor run, 176 rectangles, 173,808 requested chunks, 166,258 unique required) was started on CPUs 4–11 under the ordinary `mem-run` envelope, which runs at `nice 19` / CPU weight 50 while other agents were building and testing. It completed 32 of 176 rectangles in about 3,600 wall seconds (≈23,300 stored chunks; the first 86 dry anchors fully covered) and was stopped; that rate is contention-bound preparation throughput, not capacity. The partial world is not a fixture for any rung above the first 86 anchors.

### Revision-25 fifty-client steady-state rung (2026-10-03)

One 50-client rung ran on the partial seed-19 world with the final-candidate server `mc-server-final-cand-r` (SHA256 `d67836d6c7eb4361d396f140d5068d0fd0236de6fbc3cd0f7ed3d77e49f178cb`) and the sealed `common-helpers-v2` bots (`da7c5450367838fa3f39a80ebf5e10b0a2bc375f097f52dd000d6a6f31cf0083`, `--anchor-chunk-walk` supported). Server CPU 2 with one chunk worker, unused SMT sibling CPU 3, bots/orchestrator CPUs 4–11, VD6, ordinary `mem-run` limits, `--steady-state`. Receipt: `.analysis/codex-logs/player-scale/steady-state-rev25/20261003T042926-4b0957d7/clients-50/receipt.json`.

The two earlier attempts failed before warmup, in bot readiness: the stricter footprint-aware bots found no safe full-cube landing inside bot 42's (`ae778a7f`) and then bot 14's (`0191edcb`) anchor chunk. Terrain height alone does not make an anchor landable. The harness gained `--skip-anchors` (positions in the fixture's dry-anchor list, recorded in the receipt as `skipped_dry_anchor_positions`); the measured run skipped dry-anchor positions 14 and 42. No server data was discarded.

The measured run is **FAILED** under the harness's strict acceptance, and only on `action_observation_timeouts = 4`: 693 breaks sent, 691 acknowledged, 687 observed; 687 places sent/acknowledged/observed. Acknowledgement timeouts and disconnects are zero; shutdown was graceful (server and bots exit 0). The rev-24 run above shows the same four-unobserved-breaks signature.

**The unobserved breaks are a bot classification bug, not a server defect (localized with `--action-log`).** The first run's own logs could not tell delivery from rejection, so the bots gained an opt-in `--action-log` (bounded: 256 actions per player, 8 events per action; the final report lists every unobserved action with its position, the bot's cached column states, ack and observation timestamps, and any block/section update or chunk resend seen for that position). The same 50-client rung was rerun on the same server and fixture with bots `b7b3e101…`: `.analysis/codex-logs/player-scale/steady-state-rev25-actionlog/20261003T044333-c85ceeb5/clients-50/receipt.json` (675 breaks sent, 674 acked, 671 observed). Three timed-out breaks (bots 37 twice, 40) were each acknowledged within 72–139 ms **and** received a `BlockUpdate` at the exact position at the same instant, with state ids 87/88 = `minecraft:water` level 1/2. The server broke the shoreline grass block at y=62 and vanilla water flowed into the hole; the update was delivered. The bot counted only air-like states as an observed break, so flowing water never matched and the 10 s timeout fired. The fourth unobserved record (bot 46) was an in-flight break with no ack yet and no timeout at shutdown. The first run's four timeouts almost certainly have the same cause, but that run had no logging, so the exact four are unproven. Fix: treat water in the broken cell as an observed break; a reverted break (the original solid block returns) still does not count. The patch is `.analysis/codex-logs/player-scale/bot-water-observed-break.patch` (SHA256 `ab3fd80e890494597d3103b54558da41e39e570d49f9c6cc149109163f52a156`, `git apply --check` clean against live Main); the bot unit tests pass (6, including a new classification test) in a scratch copy of the frozen snapshot. It is not applied to the live tree.

**Open known item: shutdown drain.** The action-log rerun's server exited 1 ("shutdown drain exceeded 6s; final save completed late"): the final save flushed 8,335 dirty chunks in 5.08 s, whereas the clean run flushed 6,910 in 3.29 s and exited 0. That rerun overlapped a release build on CPUs 4–11 (foreign busy CPU2 2.85%, CPU3 6.36%; TPS 19.87, p95 20.0 ms), so its CPU and tick numbers are not a baseline. The late final drain also appeared in the rev-24 runs above. Not investigated further here.

| Metric (1,200 measured ticks, 59.998 s) | Value |
|---|---:|
| Actual TPS | 20.0007 |
| Tick p50 / p95 / p99 / max | 8.182 / 18.669 / 22.413 / 29.654 ms |
| Server CPU (one-core basis) | 28.53% (17.12 CPU-s: user 13.85, system 3.27) |
| RSS at end | 687,112,192 B (655.3 MiB) |
| Foreign busy, CPU2 / CPU3 | 0.167% / 0.917% (excludes idle and iowait) |
| Fresh generated chunks | 0 (whole-rung header diff 23,302 → 23,302), no chunk-boundary crossings, backlog 0 |

Native uninstrumented stage CPU of the 17.111 process-CPU-seconds in the measured profile interval: simulation 53.74%, saving 17.62%, network 12.44%, scheduled blocks 0.71% (now under its own CPU scope), unattributed **15.49%** (2.650 s). The earlier rev-24 diagnostic's unattributed share was 23.5–23.8%.

This is not an A/B against the rev-24 runs: the seed, terrain and anchors differ (seed 19 versus seed 42), as do the bots (`da7c5450…` versus `36da1c05…`) and the server build. It is a 50-client, pinned-single-core result on 50 of the first 86 prepared anchors only; no 100/250/500 rung was run and nothing here is a VPS or 1000-player capacity claim.

## Native villager owner-thread brain (2026-10-16)

MIX-01's prepared world contains 10,000 villagers in 25 quarters, 10,000 beds,
15,000 composters and 2,819 loaded chunks. The supplied 197-sample resident-login
diagnostic has tick-wall p50/p99 **653.425/1091.861 ms**. Entity/block phase
timers exclude the villager brain: it runs in owner-thread commit. Sampled
hotspots include repeated claim-owner scans, string POI/type classification,
whole-population threat/item scans and repeated A* collision checks.

The narrow cutover keeps the same tick owner, persisted villager memories,
collision geometry and 48-block POI reach. It resolves block-state POI kinds
once, rescans only changed resident chunks, searches neighboring chunks in
distance order, rebuilds live same-dimension claims once per tick and updates
new/released claims immediately. Dead/converting owners and player-occupied
beds are fenced before acquisition. Unloaded remembered POIs are retained, not
mistaken for destroyed blocks. Threats, dropped items and golems are filtered
once instead of rediscovered by scanning all villagers for each sensor.

Missing POIs retry after 200–239 ticks; a search tries at most five candidates.
Acquisition rotates through at most 64 due villagers per tick with a reserved
16-route/1,024-node allowance; budget denial does not advance the retry deadline.
Movement has a separate 48-route/3,072-node allowance and courtship reserves
four routes/512 nodes, so each class progresses under overload. Movement still
executes collision resolution each tick. Failed acquisition paths stay in scratch
storage, never replacing the active route or claiming an unreached POI.
Courtship defers rather than charging food when its own allowance is exhausted.
Gossip initiation and golem-spawn candidate scans rotate in batches of 64.
These are deliberately approximate AI cadences, not protocol or storage
changes. The limits are private brain constants; adjust/remove them only with
the same scale fixture and behavior tests, not an alternate AI authority.

The ignored `villager_brain_scale_benchmark` measures only the in-process brain
on flat loaded chunks with N beds/workstations and N villagers (95% initially
claimed jobs), during 80 daytime-work ticks. Construction/publication are
excluded; the cold first tick is reported separately, leaving 79 samples.
Unchanged-fixture release baseline p50/p99 is **20.289/52.155 ms** at 1,000 and
**2496.469/11072.560 ms** at 10,000 villagers. The benchmark and focused
claim/cache/budget regressions live beside the existing villager behavior tests.
Receipts are under `.analysis/codex-logs/villager-perf/`; supplied baseline
samples remain under `mix-full-scale/village-join-profile-2/`. A brain-only
measurement does not establish full MIX-01 setup/readiness or player capacity.

The corrected release run on the unchanged in-process fixture gives:

| Villagers | Before brain p50 / p99 | After brain p50 / p99 | Before / after cold first tick |
|---:|---:|---:|---:|
| 1,000 | 20.289 / 52.155 ms | 0.623 / 4.630 ms | 39.873 / 6.321 ms |
| 10,000 | 2496.469 / 11072.560 ms | 9.040 / 14.298 ms | 2794.541 / 58.271 ms |

These are single paired 79-sample diagnostics, not a sustained capacity gate.
The cold 10k tick still exceeds 50 ms while initializing the POI cache.
Commands use `CARGO_TARGET_DIR=target-vil` and
`.analysis/tools/mem-run.sh -x -m 8G` (600% CPU quota, weight 50, nice 0).
The focused release villager suite passes 72 tests (three ignored); the full
release `mc-core --lib` suite passes 1,761 (ten ignored), and release library
Clippy passes with `-D warnings`. Three independent-review defects were fixed:
birth work now has its own reserve, denied acquisition retains its deadline
and cursor fairness, and failed candidate routes cannot replace active paths.
The initial twenty-tick acquisition stagger is preserved for checkpoint/WAL
ordering. Detailed commands, counts, source and candidate hashes are in
`villager-perf/{brain-before,brain-after,validation}.json` and `*-r3.log`.

The unchanged prepared-world MIX-01 rerun reached **100/100 clients ready**
(25 residents in 67.092 s, then 75 fresh unassisted clients in 7.656 s) under
the original 300-second deadline and completed the three-second steady prefix.
Resident-login active tick-wall p50/p99 improved from the supplied diagnostic's
**653.425/1091.861 ms** to **86.974/164.797 ms** (743 samples, 633 over 50 ms).
The fresh-cohort join window has **137.469/184.358 ms** (56 samples).
These live ticks still fail the 50-ms target: the brain-only speedup is not
full-server latency acceptance.

Receipt status is `PREFIX_READY_NATIVE_BLOCKED`, not a clean persistence or
post-proof census pass. The server reached about 7.51 GiB VmHWM; the proof
driver's ten-second shutdown limit force-killed it, and native export refused
the unclean world. Subsequent attribution found full villager images appended
to WAL each tick, largely for clock fields such as `last_work`; that persistence
work is separate from the bounded POI index. No clean census is claimed here.
The exact candidate SHA256 is
`c06d9667d3df5ea398f76fd7786d3c7c748887aa35b07e73da48be8aa26f2260`;
receipt and both raw join sample streams are in `villager-perf/mix-after/`.

