# Architecture Decision Index

Read the ADR owning the decision. Later explicit supersession takes priority;
milestone notes and desired migrations do not silently change runtime truth.
The current server is `mc-edge`/`solaris-v2` + `mc-core`, defined by
[ADR 0013](0013-core-model-v2.md); [ADR 0014](0014-v1-core-retirement.md)
records the completed v1 source retirement. [ARCHITECTURE.md](../ARCHITECTURE.md)
summarizes current boundaries.

| ADR | Status | Scope |
| --- | --- | --- |
| [0001](0001-vanilla-data-as-runtime-input.md) | Accepted; current ownership updated | Licensed local vanilla content cache, explicit import/verify, no vendor bytes in Git |
| [0002](0002-vanilla-protocol-metadata-as-reference.md) | Accepted | Protocol metadata, oracle provenance and packet-layout proof |
| [0003](0003-runtime-world-lock-architecture.md) | Superseded by 0013/0014 | Historical v1 world locks and lock order |
| [0004](0004-staged-single-writer-simulation.md) | Superseded by 0013/0014 | Historical v1 central simulation authority and commit/publication |
| [0005](0005-regional-simulation.md) | Superseded by 0013/0014 | Historical v1 region leases, owner lanes and regional WAL |
| [0006](0006-mc-net-module-boundaries.md) | Superseded by 0013/0014 | Historical deleted modular-monolith/extraction boundaries |
| [0007](0007-connection-liveness-under-outbound-pressure.md) | Liveness principle retained; v1 implementation historical | Keepalive and bounded publication under pressure; current edge behavior governed by 0013 |
| [0008](0008-overworld-density-router.md) | Accepted worldgen design | Worldgen shape authority and deterministic pipeline; old serving-storage descriptions retired |
| [0009](0009-regional-plugin-boundary.md) | Isolation/admission principles retained; v1 implementation superseded | Serial plugins over bounded values; current native host/commit model in 0013 |
| [0010](0010-solaris-loader-contract.md) | Accepted | Untrusted client bundles, Configuration handshake, permissions and cache identity; wire owner `mc-loader` |
| [0011](0011-core-crate-topology-and-threading.md) | V1 topology/threading/durability superseded by 0013/0014 | Historical extraction and multiple durable streams; general ownership principles retained |
| [0012](0012-earth-worldplan-and-native-overlay.md) | Proposed for native v2; not ported | Earth WorldPlan/base/overlay proposal; v1 W0–W5 evidence is historical |
| [0013](0013-core-model-v2.md) | Accepted; sole current core model | Data-parallel snapshot/intents/merge, native state, one tick WAL, off-tick plugins and remote tickets |
| [0014](0014-v1-core-retirement.md) | Accepted; Phase 5 source retirement complete | Shared Loader/vanilla boundaries, v2-only harness/operator surface and clean v1 deletion |
| [0015](0015-multiversion-protocol-translation.md) | Proposed; 26.3 deferred by measured diff | Per-session `mc-edge` protocol translation and profile evidence |
| [0016](0016-mob-capabilities.md) | Accepted; layer 1 implemented | Owned data blueprints, mob capabilities, bounded data-defined mob types and their vanilla presentation |
| [0017](0017-tick-system-graph.md) | Proposed graph; one-system prototype | Declared entity-column resources, ordered row execution and deterministic tick barriers |
| [0018](0018-in-game-computer.md) | Accepted; draft, client QA pending | Operator-only terminal on vanilla carrier blocks and local desktop-capture boundary |
| [0019](0019-voice-chat.md) | Accepted; draft, real-client QA pending | Proximity voice chat: `solaris:voice` Opus relay between native clients, edge-only |

## Routing

- Data loading/licensing and content cache: 0001.
- Packet ids/layouts and vanilla protocol provenance: 0002.
- Per-session protocol version translation: 0015.
- Current state authority, tick/merge, threads, waiting, persistence, shutdown,
  snapshots, plugin intents, block work and tickets: 0013.
- Mob blueprint capabilities, data-defined mob types and their bounds: 0016.
- Tick system graph proposal and bounded row-phase prototype: 0017.
- Current crate ownership, retirement and explicit port/delete decisions: 0013/0014.
- Worldgen topology, climate, rivers, caves and feature ownership: 0008; runtime
  storage/recovery constraints remain 0013.
- Plugin author/operator contract: [PLUGINS.md](../PLUGINS.md) and 0013; 0009
  supplies the historical rationale, not old Luau/reload configuration.
- Loader bundles, trust, negotiation and permissions: 0010.
- Earth WorldPlan proposal: 0012, not a current serving mode.

Superseded ADR bodies intentionally retain deleted names and old test commands
as decision history. Never execute them as current recipes or treat their
receipts as current verification. Update the owning live ADR when authority,
threading, persistence ordering or module policy changes.
