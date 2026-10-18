# Agent Routes

The active checkpoint's explicit `route` is the only route selector. Never infer
it from words in the persistent goal, quoted history, compaction summaries, or
subagent output.

| Route | Primary document | Code and evidence entrypoints |
| --- | --- | --- |
| `playable` | `docs/playable/ACTIVE.md` after `docs/playable/README.md` | `crates/mc-core/src/`, `crates/mc-edge/src/`, focused harness path and named real-client scenario |
| `plugins` | `docs/PLUGINS.md` | `crates/mc-script/`, `crates/mc-plugin-host/`, native `mc-core` plugin modules and edge session endpoint |
| `parity` | exact `docs/PROJECT_SPEC.md` or active milestone section | ADR 0002, local protocol dump/oracle, exact harness comparison |
| `scaling` | ADR 0013 and exact current profile contract | `catalog-load` and measured native runtime receipts; old regional-owner benchmarks are history |
| `architecture` | `docs/ARCHITECTURE.md`, ADR 0013/0014, `docs/architecture/runtime.md` | current native symbols/callers; state: `docs/architecture/state-ownership.md`; end-to-end routes: `docs/architecture/mutations.md`. CP-ARCH plan/audit describe the retired v1 program, not current paths. |

Additional exact surfaces:

- Long-goal recovery or unspecialized continuation: `docs/MEMORY.md`; use the active owner request and its next verified outcome.
- Closeout/readiness claim: `docs/DEFINITION_OF_DONE.md`, exact milestone, and `docs/VALIDATION_LEDGER.md`.
- Minecraft client MCP/tooling: `docs/AGENT_TOOLING.md` and `../solaris-loader/README.md`.
- Protocol packet ids/layouts: ADR 0002, `.analysis/protocol-dump.txt`, `tools/dump-vanilla-protocol.sh`, and `crates/mc-test-harness/src/bin/wire_probe.rs`.
- Build/run: `README.md` and `example.toml`.
- Owner-facing architecture contract: `docs/ARCHITECTURE.md`.
- CodexPro `/goal` launch, checkpoint granularity, and runtime wrapper:
  `docs/GOAL_WRAPPER_V2.md`.

## Playable Route

`route: playable` optimizes for one useful real-client 20-minute survival loop,
not M100 replacement readiness. Do not read or edit readiness ledgers unless the
checkpoint explicitly requests readiness.

- Server: `cargo run -p mc-edge --bin solaris-v2 -- --config playable.toml`.
- Use focused one-to-three-minute client probes while fixing one phase, then one
  complete 20-minute gate at checkpoint close.
- Prefer deleting/de-scoping broken breadth over adding unrelated subsystems.
- Client MCP `connect` proves dispatch only; await pushed `in_play = true`.
- Privileged diagnostic configs are ignored/local and never survival/parity
  evidence.

## Repository Map

- `crates/` - Cargo workspace members.
- `docs/generated/` - deterministic navigation indexes (crate graph, contract index, scenario index); regenerate with `cargo run -p xtask -- architecture`, verify with `architecture --check`.
- `crates/mc-test-harness/tests/` - wire/integration gates.
- `../solaris-loader/` - independent reusable client MCP and scenarios.
- `docs/` - canonical contracts, ADRs, and process documents.
- `docs/milestones/` - milestone logs and their milestone-scoped reviews.
- `docs/evidence/`, `docs/performance/`, `docs/memory/`, `docs/playable/` - dated
  records; read them only when a checkpoint names that era.
- `tools/` - vanilla extraction, protocol, and client runner scripts.
- `example.toml` / `playable.toml` - documented debug runtime profiles.

This is one Git repository. Cargo members and client tooling are not separate
repositories.
