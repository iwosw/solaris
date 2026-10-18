# Core architecture program — cutover complete

Status: the CP-ARCH v1 extraction program is superseded by
[ADR 0013](decisions/0013-core-model-v2.md) and the completed Phase 5 of
[ADR 0014](decisions/0014-v1-core-retirement.md). Its old extraction queue is
not an active implementation plan.

## Current boundary

`mc-edge` composes the `solaris-v2` binary over `mc-core`. The native core owns
the snapshot/intents/deterministic-merge tick, authoritative gameplay and one
tick WAL/checkpoint recovery boundary. The edge owns connections, fanout,
authentication, CLI and presentation. `mc-loader` owns shared Loader wire
contracts; `mc-vanilla` owns vanilla process/capture/minimal-client tooling.
Production code does not depend on `mc-test-harness` or a sibling source tree.

Phase 5 deletes `mc-server`, `mc-net`, `mc-simulation` and `mc-entity`, their
v1-only fixtures and architecture exceptions. It does not alter native v2
runtime behavior or port the old regional-owner model into the new core.
ADRs 0004/0005/0006 and the v1 topology/threading/durability parts of 0011
remain explicitly superseded historical decisions, not current constraints.

## Where to work now

- [ARCHITECTURE.md](ARCHITECTURE.md): current architecture summary.
- [architecture/runtime.md](architecture/runtime.md): process/module boundaries.
- [architecture/state-ownership.md](architecture/state-ownership.md): native
  authority versus edge projections.
- [architecture/mutations.md](architecture/mutations.md): input, commit,
  publication and recovery routes.
- [OPERATING.md](OPERATING.md): actual CLI and supported configuration.
- [AGENT_TOOLING.md](AGENT_TOOLING.md): v2-only harness and evidence contracts.
- [ADR 0014](decisions/0014-v1-core-retirement.md): deletion inventory and
  explicit port/delete decisions, including unsupported legacy operator APIs.

## Evidence and preserved history

The [2026-09-28 audit](evidence/core-architecture-audit-2026-09-28.md), CP-ARCH
receipts, warehouse pilot, milestone logs, performance results and memory/night-
run records describe the implementations that existed at their recorded trees.
They are not rewritten into v2 evidence. Historical passes do not establish
current graphical gameplay, load envelopes, crash coverage or release readiness.
The maturity label remains **draft**; final cutover checks are reported by the
integrating agent, not inferred from documentation changes.
