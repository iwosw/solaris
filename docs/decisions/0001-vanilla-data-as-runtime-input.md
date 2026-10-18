# ADR 0001 — Vanilla data as local runtime input

Status: accepted; current ownership and explicit cache workflow updated for
ADR 0014 Phase 5 (2026-10-15). Earlier embedded-selection and automatic-import
startup descriptions are superseded.

## Contract

Solaris targets Minecraft Java 26.1.2. Registry, tag, recipe, loot, component,
block-property, protocol and world-format facts come from the matching local
vanilla distribution, not remembered layouts or guessed values. The local
owner-supplied oracle is `.analysis/server.jar`.

Extraction, `wire-probe`/`javap`, decompiled-source inspection and side-by-side
execution establish observable behavior. They do not authorize copying Mojang
implementations or mechanically translating them into Rust. Mojang bytes are
not redistributed in Git, crates or release bundles. `.analysis/` and extracted
`data/vanilla/` remain local.

## Ownership and selection

- `mc-data` owns decoded immutable data and reusable loaders.
- `mc-edge` owns content-cache discovery/import and server startup composition.
  `mc-core` consumes validated native registry/gameplay data.
- `mc-vanilla` owns vanilla process control and registry capture used by content
  import and the harness; production import does not depend on test tooling.
- Protocol layouts and provenance belong to ADR 0002.

The operator explicitly derives the complete content cache from a licensed
artifact with `solaris-v2 content import --version 26.1.2 --from <jar>` or
`--download` (Mojang metadata pins size and SHA-1). Import stages and validates
before publishing; failure preserves the previous cache. `content verify`
validates offline without starting the world/listener.

Serving only discovers and validates: `[data].vanilla_data_dir` →
`SOLARIS_CONTENT_CACHE` → XDG/home cache → working-directory/checkout
`data/vanilla`. Explicit invalid overrides fail rather than falling back.
Missing/incomplete content reports the searched locations and import command;
normal startup does not download or derive a replacement implicitly.

Native loot catalogs and the light oracle load their required JSON source
closure once from that same selected root into `CoreRegistry` before core
startup. Missing or invalid files fail closed with their full path. Core
consumers borrow the owned catalogs; chunk producers and data/worldgen contexts
receive the required immutable handles explicitly. No process-global source
cache, alternative discovery path, or compile-time vanilla copy exists. Unit
tests load the checkout's local `data/vanilla`; the tracked derived
`mc-data/data` fact fixtures remain separate and are not an operator-selectable
startup mode.

## Proof and boundaries

Loader changes preserve complete-cache validation, actionable invalid-input
errors and failure before listener startup. `content verify` is content-only,
not a server/plugin deployment `--check` or a gameplay pass. Use focused native
server/content tests and exact startup evidence; wire/client-visible changes
also require matching protocol/native-client scope. A data-loading refactor
alone does not establish gameplay parity.

See [OPERATING.md](../OPERATING.md), [ARCHITECTURE.md](../ARCHITECTURE.md) and
[ADR 0013](0013-core-model-v2.md). Historical import/embedded-selection receipts
keep their original source-tree and runtime meaning.
