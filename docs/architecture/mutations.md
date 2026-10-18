# Mutation routes

Status: native v2 routes after ADR 0014 Phase 5. The governing model is
[ADR 0013](../decisions/0013-core-model-v2.md). See
[state-ownership.md](state-ownership.md) and [runtime.md](runtime.md).
These routes describe ownership, not a claim that a verification gate was run.

## Player actions and inventory

1. `mc-edge` decodes a protocol packet and associates it with the authenticated
   live session. Connection state handles protocol sequencing and delivery only.
2. The core admits the owned input. `tick/input/` routes session, movement,
   interaction, combat, inventory and social behavior.
3. Native gameplay validates the current entity/session, reach, held stack,
   revisions and affected world/custody state. An intent includes consumed
   resources and dependent effects in the same footprint.
4. Deterministic merge applies against accumulated state. A stale or conflicting
   footprint is refused whole, not partially debited or credited.
5. One tick change set carries the accepted state and native post-images. The
   edge projects ordered player outcomes and current world state; predictions
   are repaired from native authority, never installed as that authority.

## Component operations and build hooks

Components execute on the production Wasmtime worker outside the tick through
`mc-script`'s bounded value/admission contract. The native core revalidates
opaque operation/session/generation identities and state fences at apply.
Plugin KV, native effects and operation receipts share the tick WAL/checkpoint.
Callbacks follow successful append completion, not an fsync on the reply path.

For `before-build`, the edge obtains a read-only native plan and requests the
registered handler chain outside the tick. One queue-inclusive 100 ms deadline
ends at the host answer. A timely approval may await merge, but is consumed
only once after live source/roster, native state and plugin-KV fences validate.
Refusal repairs prediction without world edits. Unsupported hooks fail startup;
no guest callback runs on a tick worker. See [PLUGINS.md](../PLUGINS.md).

## Block entities, leaves and sign decoration

Block/random/scheduled work runs through native core authority. Leaves use
cell-local scheduled distance changes and native random decay, not a whole-world
foliage scan. Sign edits validate the editor's block/session/revision and retain
side-aware dye/glow/wax semantics. Block-entity NBT post-images share the tick
journal and native chunk checkpoint; Anvil export carries their interchange
representation. Native tests are not graphical acceptance receipts.

## Commit, publication and restart

Tick changes become one journaled change set and an immutable published snapshot.
The edge computes per-viewer fanout on separate workers within queue credit.
World delivery can reconcile to a newer snapshot; per-player results stay
ordered and bounded. A packet failure does not undo a committed effect.

The journal worker persists tick batches and frozen checkpoints. Recovery uses
native state plus WAL; a crash may roll back the unflushed tail consistently
across player/world/plugin domains. There is no independent plugin journal or
Anvil-serving recovery path. Offline import/export keeps sources unchanged,
requires new destinations and writes conversion receipts.

## Harness lifecycle

`python3 -m tools.harness run PROFILE` validates the registered profile and its
prerequisites, owns server/client/Xvfb lifecycle where needed, and records
`passed`, `failed`, `prepared` or `interrupted` plus commands/logs/errors under
`.analysis/validation/`. Every actual server launch uses `solaris-v2`; there is
no variant selector. Preparation and timeouts never count as gameplay passes.
See [AGENT_TOOLING.md](../AGENT_TOOLING.md#harness-entrypoint).

## Extending this map

A domain-specific route must name provenance, bounded admission, live fences,
full resource footprint, authoritative writer, commit/unknown-outcome boundary,
publication, recovery source and exact test scope. Do not copy historical
multi-journal warehouse/inventory cards into the native tick model or relabel an
old receipt as current verification. The [warehouse pilot](pilot-warehouse-binding.md)
is retained only as an explicitly superseded historical extraction record.
