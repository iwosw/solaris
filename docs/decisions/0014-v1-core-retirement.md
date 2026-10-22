# ADR 0014 - Retire the v1 core

**Date:** 2026-10-15  
**Status:** Accepted; Phase 5 source retirement completed 2026-10-15.
ADRs 0004/0005/0006 and the v1 parts of 0011 are marked Superseded by 0013/0014;
their historical decision text remains. Final integration verification is
reported separately; source deletion does not fabricate an A/B or gameplay pass.

## Context at acceptance

`solaris-v2` (`mc-edge` + `mc-core`, ADR 0013) covers native gameplay, Anvil
import/export, statistics, advancements and the production component host,
including `before-build`. At acceptance v1 (`mc-server`, `mc-net`,
`mc-simulation`, `mc-entity`, ~77k lines in `mc-net` alone) still existed.
Keeping it duplicated plugin/Loader work, harness lifecycle, merge surface,
test time and memory. These crates are now deleted. AGENTS.md: superseded
Solaris APIs and authorities are deleted once callers move; there is no released
compatibility surface.

## Retirement inventory (2026-10-15)

| v1 piece | Used by | Action |
|---|---|---|
| `mc-net::loader` (Loader manifest/artifact/ack/sound wire contract) | `mc-edge` | Phase 1: moved to crate `mc-loader` (done) |
| `mc-server::wire_client`, `mc-server::vanilla` (process, registry capture) | `mc-test-harness`; content import (production) | Phase 2: moved to crate `mc-vanilla` (done); `mc-test-harness` src no longer depends on `mc-server` |
| `mc-entity`/`mc-simulation` fixture code | `mc-test-harness` `load_matrix_fixtures` | Phase 2: done — deleted the offline generator; `catalog-load` prepares fixtures through public v2 commands |
| Harness profiles and sibling first-party gates | QA, gates, plugin repo | Phase 3: done — v2-only launches/builds; variant flag/environment and legacy Loader fixture branches removed; profile decisions below; sibling `run_integration.py` uses `mc-edge`/`solaris-v2` |
| Operator CLI: content, backup/restore, console, access lists, pregeneration/world checks | server operators | Phase 4 done: content/backup/restore and stdin vanilla commands ported; operators use vanilla `ops.json` + `op`/`deop`. No legacy `--check`, `operator` subcommand, TUI/profile, `check-world` or Anvil rectangle `pregenerate` API is retained; native `world pregenerate --radius` (block radius, native store, resumable) is the v2 replacement. |
| Earth WorldPlan (ADR 0012, world prepare/bounds) | v1 prototype/evidence | Not ported; ADR 0012 is Proposed for the native runtime. No Earth fallback is introduced. |
| OLD baseline binary for v2-vs-old A/B | Dropped by owner 2026-10-16 | No A/B: frozen executable baselines are deleted and `tools/coreperf` runs native-v2 durability/scale only. Historical receipts remain historical evidence, not an active comparator. |

## Decision

1. `mc-loader` owns the Loader wire contract (ADR 0010). `mc-edge` depends on it,
   not on `mc-net`.
2. `mc-vanilla` owns vanilla-jar process control, registry capture and the
   minimal protocol client used by content import and the harness.
3. Every actual harness server lifecycle uses `solaris-v2`; variant flags and
   environment selectors are removed. Utility profiles need not launch a server.
4. The operator surface follows the explicit port/delete inventory above.
5. Phase 5 deletes `mc-server`, `mc-net`, `mc-simulation`, `mc-entity`, their
   v1-only fixtures and code-health exceptions in one source cutover. No
   compatibility shims or native v2 behavior changes. Live architecture,
   operator, README, routing and tooling docs describe the sole native server.
   ADRs 0004/0005/0006 and v1 parts of 0011 are superseded, not rewritten as
   native evidence. `.ai-bridge`, memory, milestone/night-run/history logs,
   dated evidence and archived releases stay untouched. Owner decision 2026-10-16
   additionally drops the frozen comparator binaries and all active A/B tooling.

## Invariants

- `cargo tree -p mc-edge` never contains `mc-net`, `mc-server`, `mc-simulation`
  or `mc-entity` (from phase 1).
- No phase may change v2 runtime behavior except phase 4 additions.

## Phase 3 profile decisions

| Profile(s) | Decision |
|---|---|
| `build` | Port: build only `mc-edge --bin solaris-v2`, in debug or release. |
| `core-client`, `inventory` | Port: shared Java/native client probes; delete the old Loader-fixture lifecycle. |
| `loader-live` | Port: named scenarios require immutable `--server-bin` and `--plugins-dir`; delete the v1 fixture path. |
| `regression`, `playable`, `replay` | Port: sole v2 launch and vanilla `ops.json`; no variant environment. |
| `player-scale` | Delete: `catalog-load` owns public-protocol player load controls. |
| `load-matrix` | Delete: `catalog-load` owns the public-fixture scenario matrix; retain scenario definitions it consumes. |
| `entity-scale` | Delete: obsolete in-process v1 load benchmark; public `catalog-load` entity scenarios replace it. |
| `living-world-scale` | Delete: obsolete v1 entity-scale wrapper; `catalog-load`/client scenarios cover live worlds. |
| `seed-review`, `seed-contact-sheet` | Port/retain: seed-review launches v2; contact-sheet uses the same client tooling. |
| `installer` | Retain: installer archive/checksum self-test uses a shell fixture, never a server build or launch. |
| `standard-pack`, `settlement-cargo` | Port: sibling runner builds and starts `solaris-v2`; no server `--check`. |
| Other client/oracle/utility profiles | Retain: shared v2 client launch paths or no server lifecycle; no variant selection. |

## Phase 5 closeout

- Current composition: `mc-edge`/`solaris-v2` + `mc-core`; shared wire/tooling
  boundaries are `mc-loader` and `mc-vanilla`.
- Old in-process load fixtures/profiles and native v1 catalog test selectors
  are removed. `catalog-load` retains public-protocol scenarios, not old
  acceptance claims. Bucket resync retains only its real-client path.
- Generated navigation is rebuilt from the surviving workspace/catalog through
  `cargo run -p xtask -- architecture` at final integration; generated files
  never define runtime semantics.
- This closeout records the source/documentation cutover. Exact final checks
  and any still-open native-client/performance acceptance belong to their
  receipts; historical passes are not relabelled.
