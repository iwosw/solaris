# Show HN: Solaris — a Minecraft Java server in Rust that ticks 100k mobs in 50 ms

**Historical draft, not current publication copy:** this article describes the
retired v1 regional-owner architecture and its recorded benchmark. ADR 0013/0014
replace it with the native snapshot/intents/merge core. Deleted source/profile
commands below are provenance only; the headline is not a v2 performance claim.

## Title options (all <= 80 chars)

1. Show HN: Solaris, a Minecraft server in Rust — 100k mobs inside the 50 ms tick
2. Show HN: 100k simulated mobs per 50 ms tick on 6 cores (Rust Minecraft server)
3. Show HN: Solaris — region-owned simulation for a vanilla-compatible MC server

## Post body

I'm building Solaris, a Minecraft Java Edition 26.1.2-compatible server written
in Rust. The headline number: on a 6-physical-core laptop, it simulates 100,000
mobs inside the 50 ms tick — mixed population p50 41.9 ms / p95 48.9 ms,
measured in a release build with 50 real-protocol headless clients connected[^1].

The design bet is region ownership instead of shared mutable world state. The
world is cut into fixed 8x8-chunk regions; each region has exactly one lane
owner per tick, and mutable ECS state never leaves its lane. What crosses lanes
is immutable tick input and compact results. Anything touching more than one
owner (chunk crossings, breeding, combat, transfers) goes through a
prepare/commit/finalize transaction, and an indeterminate commit outcome halts
the runtime rather than guessing[^2].

A few things this is not: it is not a fully parallel world tick — players,
blocks, containers and block entities still commit through one ordered
simulation owner[^2]. It is not a vanilla clone — gameplay is roughly
vanilla-like, while protocol and world format are held exact against packet
captures and `javap` output, never memory. And it is not finished: our own
conservative coverage audit counts 0 of 46 scoped mechanics rows as ready, so
treat this as a performance-architecture preview, not a server you should run
for your players yet[^3].

Scaling is close to linear in entity count above ~25k (~0.42 ms p50 per 1k
entities at 6 cores) and stops scaling past physical cores — SMT buys nothing,
which the 1/2/4/6-core curve shows directly (p50 108.4 / 67.7 / 49.3 / 42.4 ms,
2.56x at 6 cores)[^4]. Memory is ~26 KB per entity plus ~600 MB base[^1].
200k entities need roughly twice the compute and do not fit the budget on this
machine[^1].

Plugins are Wasmtime components behind a versioned WIT contract
(`solaris:plugin@0.7.0`): typed bounded values in, no world/session/socket
handles out, every command re-validated by the native owner before commit[^5].
A vanilla 26.1.2 client connects normally; there is also a native Rust client
in a sibling repo. Repo: https://github.com/kaiserproger/solaris

What I'd value from HN: criticism of the region-ownership model, especially
the transaction path and the granularity bound (25 equal regions over 6 lanes
caps load balance at max/mean 1.20 — that bound, not locking, is what the lane
imbalance numbers show)[^4]. Happy to answer anything about the numbers;
methodology details are in the first comment.

## Numbers

Every row carries its own source so this table can be refreshed when new runs
land. Pending refresh: `.analysis/codex-logs/perf50-20261008/RESULT.md`
(does not exist yet at time of writing).

| Workload | Entities | p50 (ms) | p95 (ms) | p99 (ms) | RSS | Source |
|---|---|---:|---:|---:|---:|---|
| mixed (40% villagers + 5 animal species) | 100k | 41.9 | 48.9 | 53.2 | 2921 MB | `.analysis/codex-logs/profiles-20261008/RESULT.md` row "mixed 100k (reference)" |
| animals (cow/sheep/pig/chicken/rabbit, 20% each) | 100k | 39.9 | 47.2 | 51.7 | 2888 MB | `.analysis/codex-logs/profiles-20261008/RESULT.md` row "animals 100k" |
| villagers (100%) | 100k | 46.9 | 56.3 | 62.4 | 2866 MB | `.analysis/codex-logs/profiles-20261008/RESULT.md` row "villagers 100k" |
| mixed | 50k | 32.1 | 41.0 | 43.9 | 1668 MB | `.analysis/codex-logs/profiles-20261008/RESULT.md` row "mixed 50k" |
| mixed | 25k | 19.5 | 25.0 | 29.1 | 1181 MB | `.analysis/codex-logs/profiles-20261008/RESULT.md` row "mixed 25k" |
| mixed (36 regions) | 200k | 86.8 | 100.4 | 106.3 | 5241 MB | `.analysis/codex-logs/profiles-20261008/RESULT.md` row "mixed (r36) 200k" — over budget |
| villagers (36 regions) | 200k | 89.4 | 104.6 | 124.2 | 5267 MB | `.analysis/codex-logs/profiles-20261008/RESULT.md` row "villagers (r36) 200k" — ~11 TPS, over budget |
| mixed, 1 core | 100k | 108.4 | 118.9 | 124.0 | — | `.analysis/codex-logs/scale-curve-20261006/RESULT.md` row cpuset 0 |
| mixed, 2 cores | 100k | 67.7 | 78.6 | 84.9 | — | `.analysis/codex-logs/scale-curve-20261006/RESULT.md` row cpuset 0,2 |
| mixed, 4 cores | 100k | 49.3 | 58.2 | 64.0 | — | `.analysis/codex-logs/scale-curve-20261006/RESULT.md` row cpuset 0,2,4,6 |
| mixed, 6 cores (release) | 100k | 42.4 | 50.4 | 53.7 | — | `.analysis/codex-logs/scale-curve-20261006/RESULT.md` row cpuset 0,2,4,6,8,10 |
| mixed, 12 SMT | 100k | 41.5 | 48.9 | 52.8 | — | `.analysis/codex-logs/scale-curve-20261006/RESULT.md` row quota 1200% |
| memory slope | — | — | — | — | ~26 KB/entity + ~600 MB base | `.analysis/codex-logs/profiles-20261008/RESULT.md` "Reading" paragraph; `docs/MEMORY.md` top section |

[^1]: `.analysis/codex-logs/profiles-20261008/RESULT.md` — host: 12 logical /
6 physical cores, server cpuset of 6 owner lanes in an exclusive window, release
build (`tools/harness/backends/entity_scale.sh` runs `cargo test --release`), 50
real-protocol clients on 25 regions (72 clients on 36 regions for
200k), tick budget 50 ms.
[^2]: `docs/articles/region-owned-simulation.md`; `docs/ARCHITECTURE.md`;
`docs/decisions/0011-core-crate-topology-and-threading.md` (ADR 0011).
[^3]: `docs/REPLACEMENT_READINESS.md` (M95 audit: 46 in-scope rows, 0 countable
ready rows, 0.00% conservative coverage); `docs/DEFINITION_OF_DONE.md`
(80%-coverage release target; current work labeled `draft`).
[^4]: `.analysis/codex-logs/scale-curve-20261006/RESULT.md` — 100k mixed
entities, 25 regions, 50 in-process clients, release build, one run per point.
[^5]: `docs/PLUGINS.md`; WIT contract `solaris:plugin@0.7.0`
(`crates/mc-script/wit`).

## First comment — technical details (post as author)

Methodology, since several people will ask:

- **What the mobs do.** Benchmark entities are spawned through the normal spawn
  path (`crates/mc-net/src/play/session/load_bench.rs`) and each gets the
  default mob goal from the behavior table
  (`apply_default_mob_goal` — passive ground wander and friends), then full
  per-tick AI, collision physics, and movement publication. They are not frozen
  or teleported props; the equal-work proxy across the core-curve runs is
  ~2.2M entity-move packets per run
  (`.analysis/codex-logs/scale-curve-20261006/RESULT.md`).
- **What the clients are.** Headless bots speaking the real protocol,
  connected while the measurement runs. In the core-curve runs they are
  in-process and share the cpuset; in the profile runs they are real-protocol
  clients on the same machine. State this in both directions: bot CPU contends
  with the server (pessimistic), but there is no rendering, no real network,
  and no more than 50–72 clients (optimistic about client-facing work).
- **What "tick" means.** Owner-tick p50/p95/p99/max per tick over the measured
  window (1200 measured ticks after 200 warmup in the curve runs). The budget
  is the vanilla 50 ms (20 TPS); the scale-curve harness panics above
  p95 <= 50 ms, so FAIL rows there mean "over budget", not a crash.
- **Build.** All numbers are release builds (`entity_scale.sh` runs
  `cargo test --release`).
- **What was not measured.** No comparison against any other server software
  exists and none is claimed. Single run per point on the curve; p50/p95 from
  1200 ticks. Plateau cause past 6 cores (region granularity of the 25-island
  workload, bots sharing the cpuset, SMT, memory bandwidth) is not isolated —
  the receipt says so explicitly, and an Amdahl fit would be misleading.

Architecture pointers: one-tick walkthrough and the transaction model are in
`docs/articles/region-owned-simulation.md`; the crate/threading topology
(one dedicated sim thread, regional lane OS threads, journal thread,
Tokio-for-I/O-only) is ADR 0011
(`docs/decisions/0011-core-crate-topology-and-threading.md`).

## FAQ (expected objections)

**Is it vanilla?**
Gameplay is approximately vanilla (minecraft.wiki-level similarity); protocol
and world format are held exact against local `wire-probe`/`javap` evidence.
Bit-exact RNG order, AI cadence, and terrain are explicitly non-goals
(`docs/ARCHITECTURE.md`). Replacement readiness is a stabilization claim we do
not make: our M95 audit counts 0 of 46 scoped rows as ready
(`docs/REPLACEMENT_READINESS.md`).

**Does it beat Folia/Paper/Pumpkin/whoever?**
Unknown — by owner decision we do not publish comparisons against other
servers, and no equivalent-workload runs exist. The numbers above are
self-measurements with stated methodology, nothing more.

**How is this different from just sharding the world?**
Regions are fixed 8x8-chunk tiles with one lease each, invisible to plugins;
the interesting part is the cross-region transaction path
(prepare/commit/finalize/rollback, deterministic lane order, halt on
indeterminate outcome) plus save barriers that capture all lanes at one
completed phase (`docs/articles/region-owned-simulation.md`).

**What about plugins?**
Wasmtime components on `solaris:plugin@0.7.0`. Guests get immutable bounded
records and return bounded command batches; native owners re-validate every
command. Strict production deployment fails closed on any manifest, grant, or
roster drift (`docs/PLUGINS.md`).

**What does not work yet?**
Maturity is `draft` (`docs/DEFINITION_OF_DONE.md`). Concretely: broad mechanic
coverage is unproven (0/46 ready rows), 200k entities exceed the tick budget on
6 cores (~11 TPS for villagers), scaling plateaus past physical cores, and
client-visible behavior still goes through real-client gates per change. Check
`docs/REPLACEMENT_READINESS.md` for the current gap list before self-hosting.

**How was it measured, exactly?**
See the Numbers table sources above: one source line per row. Headline
conditions: 6 physical cores, exclusive CPU window, 100k entities over
25 regions, 50 connected real-protocol clients, release build. Reproduce with
`python3 -m tools.harness run living-world-scale`.
