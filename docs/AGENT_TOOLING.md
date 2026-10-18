# Agent Tooling

This file keeps agent setup details out of milestone docs. `AGENTS.md`
contains the rules; this file contains the local wiring.

## Current Tools

The paths below are the ones a working checkout actually has. Entries marked
*(owner workstation)* come from the owner's opencode setup: this repository does
not require them, and on a checkout without `~/.config/opencode` those commands
and MCP plugins are simply absent — normal work then uses the repository rules,
the harness, and the agent's own tools.

| Tool | Status | Use |
|---|---|---|
| CodeGraph | installed globally via npm as `@colbymchenry/codegraph@1.2.0`; Codex MCP server `codegraph` registered; omp user-wide MCP `~/.omp/agent/mcp.json` exposes `codegraph_explore` (the v1.2.0 MCP surface is explore-only; `callers`/`callees`/`impact` stay CLI); rust-analyzer via omp `lsp.yml` answers hover/references; telemetry disabled; Solaris index lives in ignored `.codegraph/` | Targeted symbol graph questions: callers/callees, mutation paths, lock holders, affected tests, and blast-radius checks. Refresh with `codegraph sync .` after edits before relying on it. |
| Serena *(owner workstation)* | enabled globally through opencode MCP | Optional targeted Rust symbol search/editing and project memories; do not use it as mandatory startup context. |
| Context7 *(owner workstation)* | enabled globally through opencode MCP and verified 2026-06-11 | External library/framework docs. Use `resolve-library-id` before `query-docs`. |
| RTK | installed at `~/.local/bin/rtk` (verified 2026-09-16) | Compact shell output, called explicitly. The OpenCode auto-rewrite plugin the owner's machine has at `~/.config/opencode/plugins/rtk.ts` is not installed everywhere. |
| Headroom | installed by `uv tool install "headroom-ai[all]"` at `~/.local/bin/headroom` | Optional context compression/proxy/MCP/learning. Do not route opencode provider traffic through Headroom unless explicitly asked. |
| Agent harness *(owner workstation)* | installed globally under `~/.config/opencode/bin/agent-harness` | Optional opencode workflow only when explicitly requested; normal Codex work uses the repo rules and exactly one final reviewer. |
| Minecraft client MCP | embedded in the repo's Fabric 26.1.2 development client; optional Loader clients also support NeoForge and Forge; loopback Streamable HTTP endpoint | Structured real-client observation, connection, inventory/entity waits, input, selected-item drop, and reusable multi-client core gates without screenshot assertions. |

## Validation Harness

The repository validation entrypoint is `python3 -m tools.harness`, run from the
repository root. Profiles reuse the existing Cargo, Gradle and scenario engines;
they do not lower the acceptance bar or turn preparation into gameplay evidence.
Never invoke `tools/harness/backends/` directly: those engines are private, and
only a profile run leaves a canonical receipt. `list` is authoritative for
the current profile catalog:

```sh
python3 -m tools.harness list
python3 -m tools.harness run PROFILE [--client java|native] [--release] [--timeout-seconds N] [--platform fabric|neoforge|forge] [backend args]
python3 -m tools.harness client [--check] [--platform fabric|neoforge|forge]
python3 -m tools.harness mcp [--server-address host:port] [--exercise-input] [--disconnect]
```

All server-launching profiles use `mc-edge`'s `solaris-v2` binary. The
`--server-variant` option and `SOLARIS_SERVER_VARIANT` selector are removed.
The retired `entity-scale`, `living-world-scale`, `player-scale` (including
remote), and `load-matrix` profiles are replaced by `catalog-load`. Its
protocol bots and the control JSON recipes under `load_matrix_scenarios/`
remain in use; the old offline v1 fixture generator is removed. Catalog
classification and work oracles replace the old ladder's acceptance rules,
not its historical performance receipts.

`core-client --check`, `inventory --check`, and `seed-review --check` prepare
the v2 binary, isolated configuration and operator access lists without starting
the server or graphical client; their receipts are `prepared`, never gameplay
passes. Shared client probes honor the read-only `SOLARIS_SERVER_BINARY`
candidate override. The typed `--server-bin` option is reserved for
`loader-live`, which also requires a named scenario and `--plugins-dir`; sibling
scenario deployment helpers are not executed against this immutable deployment.

### Imported Constantinople fixture

`python3 -m tools.coreperf.constantinople census --world CANONICAL_ANVIL_EXPORT
--output .analysis/maps/constantinople/census.json` scans a stopped native
world's `solaris-v2 world export-anvil` output. Add `--counts-only` for large
comparison worlds to retain the same census without allocating or writing the
per-cell `terrain.json` used for population planning. Legacy maps must first
pass the existing offline upgrade/import/export path.
Native borders are owned per dimension through import, WAL recovery,
checkpoints and export. Vanilla's independently saved Nether border center
is not flattened to the Overworld center; unrelated saved-data extras remain
preserved.
The map-index counter (`data/minecraft/maps/last_id.dat`, formerly
`data/idcounts.dat`) may be raw NBT without DataVersion. Import uses vanilla's
1343 fallback for that file and retains its original bytes in conversion extras;
it does not invent a current-version stamp or reset the counter.
The same 1343 missing-version default belongs to the common vanilla
`SavedDataStorage` loader, including scoreboard; it is not the chunk/level floor.
Known saved-data files without an implemented type-specific upgrade stay
byte-exact in conversion extras with an explicit deferred receipt, not a current
stamp or native activation/parity claim. Parsed records still reject malformed,
duplicate and future versions.
Retired vanilla structure/village stores (`Mineshaft`, `Monument`, `Stronghold`,
`Temple`, `Village`, `villages`, `villages_end`, `villages_nether`) are explicitly
opaque: exact file bytes/source-relative paths are preserved without decoding,
inventing a version, transformation or native activation. Their feature trees
need not fit a live NBT allocation budget because the importer never consumes
them. Unknown saved-data names still fail; no NBT/network limits are relaxed.
The chunk decoder accepts vanilla's absent/sparse `sections` and absent
block/biome palettes using empty defaults. During import, non-full chunks with
any non-air blocks are retained as-is (including Status); the receipt counts
them as `non_full_non_air_chunks`. Only non-full all-air chunks without owned
block-entity/tick payloads omit native terrain coverage so the existing loader
generates on demand; their count is `non_full_empty_chunks`. All-air chunks with
such payloads keep the normal native path (`non_full_air_with_payload_chunks`).
Generation-derived tag dispositions are counted under `regenerated_chunk_tag.*`.
Every other non-block chunk-level tag that cannot survive regeneration,
including nonzero InhabitedTime and structure starts/References, is an explicit
`PRESERVATION LOSS` in `known_gaps` with chunk paths and `preservation_loss.*`
per-tag counts (`regenerated_structure_losses` also counts structure-bearing
chunks). Solaris worldgen is only vanilla-like: this is not round-trip
preservation success. Entities keep independent custody. Source files remain
untouched. Census/preservation scans accept
zero-byte unused region placeholders, but reject partial headers.
Import drops vanilla `minecraft:empty` fluid ticks as no-ops and counts them
under `dropped_noop_fluid_ticks`; unknown fluid identities still fail.
Both block and fluid scheduled ticks outside their serialized owning chunk are
discarded, as in 26.1.2 `SavedTick.filterTickListForChunk`, even if the containing
chunk exists. The receipt counts `dropped_foreign_scheduled_ticks`; this is vanilla
load behavior, not a preservation loss or a re-homing rule. Negative coordinates
use floor division. Unknown block/fluid identities still fail strict validation.
Owner-approved import normalization also collapses exact scheduled requests
(position, type, deadline and priority), preserving the first source order and
counting `deduplicated_scheduled_ticks`. Different deadlines/priorities remain
distinct; the live/native scheduler contract is unchanged. The vanilla new-request
hash set is keyed by position/type, but that alone does not prove loaded duplicate
lists execute once, so this is not an exact loaded-list gameplay-parity claim.
Offline native baseline writes use non-fsync staging transactions; the existing
final Immediate checkpoint durably commits them before the only destination
publication. Crashed staging is never a playable world. Ordinary native baseline
and live/runtime checkpoint writes retain their Immediate durability.
Entity UUIDs are admitted deterministically, first body wins, in dimension,
chunk and original list-index order (inline region entities merge into the
chunk's entity record first). Every later body sharing an already admitted UUID
is dropped, including a byte-identical one, and no UUID is reminted or
rewritten, so references stay pointed at the keeper. Each drop is counted under
`duplicate_uuid_dropped` and listed as a `PRESERVATION LOSS` in `known_gaps`
with the source path, chunk, original entity index and UUID. The exact original
source entity NBT is archived beside the receipt in
`<receipt filename>.dropped-entity-nbt/` (one gzip NBT file per dropped body,
stable `<drop-ordinal>-<uuid>.nbt` names). Nil or malformed UUIDs and other
unrelated malformed bodies still fail the import; `reminted_ids` remains the
WireId counter only.
Villager NBT loading applies vanilla's `max(1, level)` minimum before native
validation; zero/negative saved levels become novice without changing the source
NBT or other villager state. Positive representable levels above five are kept
unchanged in native/journal after-images and save/export; gameplay generates only
the five vanilla trade tiers and never unlocks another tier for such residents.
Native/journal after-images still reject level zero.
Versionless standalone player files use vanilla's oldest (`-1`) source epoch,
even after a terrain-only official-jar hop. Legacy numeric spawn eggs create
their missing item `tag` before entity-ID flattening; e.g. Damage 96 retains its
mooshroom identity as `minecraft:mooshroom_spawn_egg`, never a dropped stack.
Owner-approved missing-dimension policy relocates a player from an absent saved
dimension to the overworld's saved world-spawn coordinates, and clears a respawn
target whose dimension is absent. Inventory, statistics and unrelated fields are
retained. Each affected player receives an explicit `PRESERVATION LOSS` receipt
entry with original Dimension/Pos/SpawnDimension/SpawnX/Y/Z/respawn values.
This is an explicit import normalization, not a claim of exact 26.1.2 placement
parity: current vanilla login resolves missing dimensions separately from Pos.
The opt-in `real_vanilla_input_records_upgrade_and_decode_read_only` and
`original_saved_data_real_records_upgrade_and_decode_read_only` mc-core tests
check all local players and level/saved-data records without rescanning terrain
or entity regions. The original preflight accepts
`SOLARIS_REAL_WORLD_PREFLIGHT_SOURCE=/absolute/original-input` for an isolated
build. Run the chosen test with `-- --ignored --nocapture`. Every failing path
is reported together without source writes; older retained envelopes are
explicitly reported as deferred, not decoded as current metadata.
Use the memory guard for this check, and a SHA-pinned **release** `solaris-v2`
binary for full-map stage runs; debug import timings are not performance evidence.

`solaris-v2 --config FILE world census --source STOPPED_NATIVE --receipt OUT.json`
counts native entity roots/passengers, villagers, stored chunks, settlements and
persisted memberships/households directly, without Core ticks or an Anvil export.
The config's seed/generation identity must match. Receipts lie outside the world;
source locking and before/after hashes prove read-only scope. A journal tail newer
than the checkpoint is refused, not silently excluded from the counts.

The existing `core::native_population_tests::native_offline_population_save_kill_reopen`
fixture can extend an imported **copy**, never the pristine Falcons import:
set `SOLARIS_POPULATION_EXISTING=1`, `SOLARIS_POPULATION_SEED=SEED`,
`SOLARIS_POPULATION_ROOT=ABS_COPY`, `SOLARIS_POPULATION_COUNT=5000`,
`SOLARIS_POPULATION_HOUSES=1`, and the explicit `SOLARIS_POPULATION_ORIGIN_X/Y/Z`.
For the bounded Falcons correctness scene, `[8192,310,8192]` is outside the
imported city: a fenced continuous platform with 5000 houses, beds and composters.
The fixture refuses non-air placement, existing cohort UUIDs and unclaimed
worldgen inhabitants; it preserves baseline entities, activates house-grid chunks
through native tickets, disables natural mob spawning, freezes daytime at 6000,
and writes `population.json`. Its default fresh-world storage/kill9/reopen mode
is unchanged. The housing scene is not a city-street placement/performance claim.

Run `python3 -m tools.harness run constantinople -- --native --world ABS_COPY
--server-bin MAIN_DEBUG_BIN --bots-bin SNAPSHOT_PROBE_BIN --vanilla-data-dir ABS_DATA --seed SEED`.
This reuses catalog process ownership with an empty plugins directory and native
console commands: move the copy's spawn to each receipted quarter-grid centre,
`settlement create`, explicitly `settlement bind` each cohort UUID, then restore
the fixture spawn. Neither settlement creation nor membership is automatic.
After command feedback, the harness waits for 2400 actual game ticks using the
dashboard's event-driven clock endpoint, stops cleanly, performs native census,
reopens the real server and checks counts/memberships/households again. No bot,
plugin, wall-time sleep or performance verdict substitutes for the native run.
The probe binary is the existing `player-scale-bots` in its read-only native
snapshot mode; it never connects a player. Each stopped phase verifies every
authored villager UUID and its exact assigned settlement, not merely a count.
Bind alone does not form a household: household counts must remain at baseline
(zero here); native birth or an explicit `FormHousehold` forms one. Long live
runs may create ordinary item loot, so review the two-way UUID delta and lifecycle
causes rather than requiring a constant total entity count. The reopen census
must match the actual stopped run's counts. `--reopen-only` checks that same
already-bound world after diagnosis without repeating create/bind or the long
run.
The empty runtime is reported as `"plugin": null` by `/stats`. For this native
scene only, graceful server shutdown uses the bounded `--timeout-seconds` budget
to allow its large native checkpoint to finish; other catalog scenes retain
their ordinary 10-second shutdown grace.
On reopen, native readiness waits for a real game-clock sample through the
dashboard's push-driven long poll, not merely for its HTTP listener.
The native console awaits command execution and tick publication before reading
the next line. Its serial create/bind setup has a separate
`--setup-timeout-seconds` deadline (default 3600); tick/census fences and shutdown
retain `--timeout-seconds` (default 1800). The receipt measures the bind batch
between real game-clock samples and monotonic wall-clock boundaries, reporting
binds/second and binds/game-tick separately from create, spawn and info commands.
These describe the serial debug-console path, not intrinsic bulk-bind cost or a
performance verdict.

Native memory evidence is appended immediately to each phase's
`rss-events.jsonl` at process/readiness events, each `Founded`, the first and
every 100th `Bound`, bind/tick fences, and shutdown boundaries. Samples reuse
the catalog's `/proc` reader; there is no timer or polling sampler. An exited
process has no current RSS (`null`), and the native receipt records observed
graceful-shutdown elapsed time separately from checkpoint commit time.

`preservation --source STOPPED_IMPORT_INPUT --world CANONICAL_ANVIL_EXPORT
--output .analysis/maps/constantinople/preservation.json` compares the exact
import input with the stopped native export across every dimension. It streams
entity/passenger and block-entity counts, stored container/equipment item totals
(including nested containers), and each player's inventory/ender/equipment
items. It uses the importer's authoritative embedded-player rule, not a second
copy of that player's external record. Trade templates, derived SelectedItem,
and the identical legacy horse `ArmorItems[2]` mirror of `ArmorItem` are not
additional stored items. A distinct armor payload is never hidden as a mirror.
Legacy pig/strider `Saddle` flags count as the attached saddle later materialized
by vanilla's saddle-equipment fix. A mismatch writes a FAIL receipt and exits
nonzero; this is physical-item count preservation, not a byte-exact or
cosmetic/RNG oracle. Large scans require the same memory slot and Main's GO as
the import itself.

`plan --terrain PATH
--output DIR --villagers N --animals 64 --bots K --seed 1453` treats N as the
target total: it preserves existing villagers and tops up only the difference.
It emits seeded exclusive planned home/job pairs, age-zero planting commands
for every farmland cell, four-species green-area placements, a cardinal street
graph and closed bot routes. Existing POIs are reused where reachable and not
already claimed; missing housing/workplaces are authored as disjoint flat
3-by-3 outdoor modules. Live villagers acquire claims naturally; a plan is not
evidence that a Brain acquired its POI.
Outdoor census uses the protocol driver's conservative full-cube terrain
contract, not semantic road recognition; roofs count and partial shapes do not.

Run the bounded functional check through the canonical harness, inside an
ordinary `.analysis/tools/mem-run.sh -m 6G` slot:
`python3 -m tools.harness run constantinople -- --plan DIR --world NATIVE_WORLD
--server-bin ABS_BIN --bots-bin ABS_BOTS --vanilla-data-dir ABS_DATA --seed SEED`.
The profile reuses catalog process ownership, complete received regional windows,
32-command setup fences, durable save acknowledgements and read-only native
snapshots, then reopens the same world. `player-scale-bots --street-routes PATH`
follows those cardinal waypoints only while received terrain admits movement;
it never teleports between waypoints. Scope is at most 5,000 villagers, 500
animals and 10 bots; tick/RSS observations are informational, not a performance
or full-scale claim. Keep all third-party archives, worlds, census/plan payloads
and receipts in ignored `.analysis/`; never commit map or Mojang bytes.

### Owner catalog load harness

`catalog-load` uses the private `catalog_load` backend, the ordinary **v2 release
server**, and a separate `player_scale_bots` process. It does not build binaries,
run a kernel-only benchmark, or impersonate a rendered Minecraft client.
`tools/harness/catalog_load_scenarios.json` is the machine-readable mapping:
each stable scenario `id` records fixture/behaviors, wall-time windows, required
work, oracle/invariants, capability blockers and threshold provenance.

The owner files `.analysis/SOLARIS_MINECRAFT_LOAD_CATALOG.{md,json}` remain
**DESIGN_ONLY**, not execution receipts. Their headings are unnumbered: there
is no “section 7 with 12 controls.” The twelve control names come from
`tools/harness/load_matrix_scenarios/`; their catalog IDs identify related
risks, not full equivalence. `mixed-survival-long` is an owner-requested harness
composition, not an invented catalog scenario ID.

```sh
python3 -m tools.harness run catalog-load -- \
  --scenario JOIN-01 \
  --server-bin /absolute/prebuilt/solaris-v2 \
  --bots-bin /absolute/prebuilt/player_scale_bots \
  --clients 10 --smoke --out /absolute/new/catalog-load-smoke
```

Use a new output directory. Repeat `--scenario` for explicit selections.
The `JOIN-01` public-command path uses one declared operator setup client to
create a bounded safe platform, clear y=64..95 in eight legal four-high fill
slabs, set world spawn and ordinary gamerules, then issue `save-all flush`.
The received `Saved the game` acknowledgement fences the preceding commands
behind a published checkpoint; feedback packet counts are not a completion
oracle because no-op commands need not produce one reply each. Only then
cleanly stop, hash and restart that same isolated world. A fresh unassisted verifier must decode the
five requested center/corner grass columns at feet y=64 with clearance before
the measured phase. Successful `/fill` feedback alone is not fixture correctness.
The probe reads the actual section-palette cells at y=63,64,65: grass followed by
two air cells. Advertised surface heightmaps are not the fixture geometry oracle.
This is a **five-column sample**, not exhaustive verification of every block in
the 65-by-65 platform. Setup uses `load_setup` identities, the fresh verifier
uses `load_verify`, and measured clients use `scale`. Exact `JOIN-01` measured
clients have no operator permissions; explicitly assisted controls do.
This public surface is storage-portable; it does not assert that native compact
storage is currently active or that the owner performance hold is satisfied.

The twelve control recipes are in `tools/harness/catalog_controls.py`. They
author **real public fixtures**, not legacy pregenerate files or internal Core
state: complete 15-by-15-chunk arenas, all declared bodies/crops/machines and
finite inventories. Each preparation region first receives its full
view-distance-eight window; four passive arenas are separately held before
their commands execute. Preparation batches wait for each unique literal
command echo to protect the public input queue. **Measured demand never does.**
Measured controls retain view distance six and explicitly declared operator
assistance; hostile controls enter actual survival mode at the Run epoch.
At the common Run epoch, current received poses, cells and inventories become
the measured baseline. Warm movement, growth, damage and reply totals are not
measured work; steady, burst and quiet recovery retain separate receive-window
accounting. Reports after the wall schedule ends cannot acquire new work credit.
An interrupted rung retains its precise failure classification, incomplete
window status and config/command hashes in its own receipt; the ramp also
records that rung instead of leaving an unclassified partial result.

Base stocking uses real `give`, menu quick-moves and 64 finite drops, not
unsupported container SNBT. It includes all 64 furnaces, 128 hoppers and 512
buttons. Its declared 642-second finite preparation clock, terminal public
save fence and two-second quiet drain are separate from measurement. No stock
is refilled or time extended until a work floor passes. Checkpoint crops are
first planted after stocking, so this preparatory clock cannot silently mature
the entire field before the measured growth window.
Bot support/headroom is derived from received section palettes, not advertised
heightmaps that may still describe pre-fixture terrain. Absolute block deltas
can hand observation between viewers; equal duplicate states do not add work,
and another viewer's delayed chunk image cannot overwrite that delta.
Teleport confirmation also reports the server-resolved position and rotation,
including relative angular flags. This mandatory protocol reply keeps the edge
decoder's pose current even when a setup/recovery window offers no movement;
it is not counted as offered or completed gameplay movement.
Base-family menu and button bursts use predeclared 250 ms teleport leads, not
response-dependent waits. Each four-button group is reachable from one central
pose; button offers retain fixed 125/63 ms steady/burst spacing and all 512
finite presses. Each menu opens, holds a fixed 200 ms for received content,
and closes; the next open is 63 ms later, never response-dependent. All 384
menu opens remain; explicit closes replace the former implicit screen changes.
At 10 clients the declared burst requires at least 10787 ms: select
`--burst-seconds 11` before starting, not an extension until work passes.
Transport commands are not counted as container or redstone work. Missing
authoritative pose/reach still rejects the original offer: there are no retries,
predicted teleport poses, refills or extended work floors.
Rejected base-family offers retain a `mechanic_unavailable` event in `bots.log`:
the actual guard, offered/declared time, current pose, last command write age
and last received position-sync pose/age. This distinguishes client-side
admission from a sent request rejected by the server; it does not retry work.
For a causal diagnostic smoke, `catalog-load --wire-trace-bots 3 6 7 9` writes
an independent `rung-*/bots-wire.jsonl` (not interleaved with stdout telemetry).
Selected bots record a per-bot sequence, Unix nanoseconds and Run-relative
microseconds for command writes, every decoded position sync (raw flags/id,
raw and resolved pose), confirm and pose-echo writes, movement writes, and
finite button offers/writes/refusals. Tracing is opt-in and changes no schedule
or response handling; trace runs are diagnostic, not performance evidence.
The selected trace sink buffers file writes and explicitly flushes after the
bot tasks stop; it never delays the next scheduled offer for a server response.


After clean stop, `--native-snapshot WORLD` pins the actual read-only native
checkpoint for entity/Brain/UUID state. Physical cells and atomic container
stocks come from the **frozen server's read-only native-to-Anvil export**, then
bounded actual palette/NBT decoding. Source hashes must remain unchanged across
these probes; no invented procedural base, writer or repair stimulus is used.
Native dictionary provenance IDs are not mislabeled protocol state IDs.
Native globals follow the committed domain codec: JSON except named-NBT
`scoreboard`, `level_metadata` and `conversion_extras`; player payloads are
named-NBT compounds too, not JSON blobs.
Native container snapshots carry final-only slot ledgers. Restart validation
checks their exact stock conservation and item identities without requiring a
wire-only initial menu image; measured live work still requires both endpoints.
Live hopper work can include received container-slot changes that return to the
initial counts. The dedicated `container_slot_updates` receipt excludes player
inventory/carried changes; generic menu versions or writes alone never prove
circulation.
Checkpoint restart reopens the same path and identity, then obtains new images;
natural simulation may change bytes, but regeneration and ownership loss cannot
pass. The checkpoint proof is an actual received `fixture_checkpoint_saved`
event, never a feedback count or “commands sent” boolean.

**Supported scope:** `JOIN-01` offers all requested measured survival logins
concurrently, without creative teleport/item grants or operator permission.
Every client must reach its complete decoded chunk window, confirm spawn
teleport, verify the platform and receive its own uniquely tagged ordinary chat
echo. Echo matching consumes literal NBT `text` plus ordered `extra` components
without flattening allocations; a root `text` field alone is not the rendered
chat. Dynamic/translatable components are not accepted as literal echo proof.
This establishes the catalog's ready/first-confirmed-action protocol gate,
not rendering, general movement parity or full-world persistence correctness.
`join-ready-assisted` deliberately retains the older bounded preparation,
creative landing and dirt grant; it requires actual received break/place block
updates. `explore` requires measured decoded chunks and the server's
snapshot-derived `SetCenterChunk` transitions. Its endpoint displacement is a
conservative lower bound, not client-predicted distance or a per-movement ACK;
it does not establish persisted generation correctness.
Those two diagnostic IDs have `catalog_closure: false`; their `PASS` cannot
close the related exact catalog case.

The mapping retains all nineteen requested control/compound/long rows:

| ID | Current v2 capability | Concrete scope or prerequisite |
|---|---|---|
| `dense-hostile` | Runnable control | All 1024 authored hostile UUIDs, real survival/damage/respawn and movement/native-owner proof |
| `spread-passive` | Runnable control | Four full arenas, 256 passives each; received wandering plus native 1024-UUID ownership |
| `explore` | Runnable diagnostic | Assisted walking, decoded chunks and received cache-center transitions; not exact CHUNK-01 closure |
| `built-base` | Runnable control | 64 furnaces, 128 hoppers, 64 loose stacks, 512 physical buttons; real transforms and atomic native stock |
| `fluids` | Runnable control | 128 naturally clocked capped sources; finite plug releases and fresh topology/source/level cells |
| `village` | Runnable control | 64 actual farmers, legal native Brain and unique declared bed/job claims; not a full day-cycle oracle |
| `combat` | Runnable control | 128 hostiles, 96 finite primed TNT plus 96 chain targets; real blasts, health and removal |
| `projectiles` | Runnable control | 128 skeletons plus 128 finite initial arrows; continued natural UUID/shooter/victim/damage/removal proof |
| `farms` | Runnable control | All 11440 natural crops and substrates; legal fresh ages and positive measured growth |
| `plugin-heavy` | Runnable with current-WIT towns + economy `--plugins-dir`, at least two bots | Concurrent town/claim and exact balance state queries, idempotent payment, ordinary chest custody, per-package journal commits |
| `checkpoint-restart` | Runnable control | Full mixed base/128 passives/64 villagers/11440 crops, actual save ACK and same-world native restart |
| `long-session` | Runnable control | Finite full base/256 passives; minimum 600 seconds non-smoke, independent resource/queue/work/native stock proof |
| `MIX-01` | Runnable | Native job-site uniqueness, reassignment after workstation loss, burst-time login cohort first actions; full scale 100 clients / 10k villagers / 25 quarters |
| `MIX-02` | Runnable | Declared stock conserved across all native owners, positive free-branch throughput; full scale 25 players / 256 hopper-chest modules |
| `MIX-04` | Runnable | Pinned slime/honey overlap + gravity + water final block image; full scale 8 machines around a 4-chunk corner |
| `MIX-08` | Runnable | Socket-limited slow reader, normal-reader progress, death/respawn identity convergence; full scale 100 clients |
| `MIX-18` | Runnable | Public-command outbound/return waypoints, cold target, accepted edit live + native restart readback |
| `MIX-21` | Runnable | Finite blasts; every reader observes the final block/move/removal live before shutdown; native restart agrees |
| `mixed-survival-long` | Runnable composition | MIX-01 + MIX-04 + MIX-21 pieces at full scale; long-duration soak is a separate measurement |

Additional entries are runnable `JOIN-01`, runnable diagnostic
`join-ready-assisted`, and blocked exact `CHUNK-01`. The new public control
path does not consume legacy `solaris/world.json` or `solaris/entities.dat`.
It verifies actual native entities, inventories, physical POIs and timers
rather than treating import/export existence as preservation proof.
`tools/harness/catalog_mixes.py` keeps separate MIX recipes/validators (tiny for
`--smoke`, catalog scale otherwise; receipts record actual counts). A control
cannot silently replace a compound crisis. Oracles use observed end state
(native image, reader observations, conservation, restart readback), not new
server trace exports. Runnable means implemented, not an executed PASS.

Defaults are 10 clients, seed 42, view distance 4, readiness deadline 30 seconds
and **20/10/20 wall seconds** for steady/burst/recovery. Override with
`--steady-seconds`, `--burst-seconds`, `--recovery-seconds`, `--readiness-seconds`
and `--view-distance`. `--ramp-clients 1 10 25 50` selects strictly increasing
client rungs. Gameplay starts at a shared ready barrier and a monotonic epoch:
JOIN movement offers are20 Hz per client and chat1/2 Hz steady/burst; assisted
edit diagnostics offer1 Hz edits. Control-specific fixed rates/intervals and
finite action slots are receipted, including exact6000/10000ms command intervals.
Recovery offers **zero gameplay**, retaining connected readers and mandatory
protocol liveness, never repair commands. Natural timers are not legacy tick
deadline or wall-time equivalence claims. Non-smoke `long-session` requires
explicit windows totaling at least600 seconds; its short smoke is not soak closure.
Readiness uses event-driven port/child waits; metric consumption uses the pushed
`after_tick`/`wait=true` long-poll rather than periodic wall-clock sleeps.
Unassisted logins rendezvous before one producer-clock release; their burst ends
at the last client's actual ready timestamp, not the sampler's return time.
Steady/burst/recovery boundaries use the bot's shared schedule epoch. Consumers
keep reading until a completed tick starts at or beyond the end, then select
only ticks starting in `[start, end)`. Missing samples, gaps, overwritten rings
or invalid clocks mark windows `valid: false` / `INFRA_BLOCKED`, never PASS/PERF.

Receipts separate offered, transport-admitted, skipped/dropped, actually
observed server admission/completion, and client-observed work. Writing a
movement/command packet is not completion. Keep per-window full tick samples,
sampled work/queue/resource evidence, clean shutdown and saved-fixture proofs,
plus binary, data, fixture, command and schedule hashes; an initial population
or idle process never proves work/drain.
Finite menu-close offers keep their declared wall slots, but an in-flight menu
open defers that one close packet until matching fresh container content is
received (`awaited_menu_content: true`). Open-screen alone or an older image
cannot release it. There is no retry, catch-up burst, shifted later offer, or
extended measurement window; missing content retains an unwritten close and an
explicit deadline failure. A late content-triggered close only finishes an
already-admitted operation, including during recovery; it does not add new load.
Ready/final/failure reports retain the full physical/menu/entity mirror;
periodic samples carry `images_omitted: true` bounded summaries, not duplicated
tens-of-thousands-cell images. Literal command replies and truncation counters
remain separate from successful plugin work or actual saved-game completion.
The public checkpoint receipt recognizes only v2's exact `Saved the game` or
OLD's numeric `Saved N players, N entities, N chunks` success grammar; OLD's
`Save-all failed; see server log` refuses preparation. This reply is not a
substitute for normal typed checkpoint readback and reopen durability proofs.

`plugin-heavy` requires `--plugins-dir /absolute/current-WIT/deployment`
containing real `solaris-towns` and `solaris-economy` packages (manifest, Wasm,
and shipped config; economy currency `coins`). Only those required packages
are copied intact into the isolated preparation directory; the source is never
modified and every copied file hash is retained. Other installed packages do
not silently change this server-only lane.

Preparation creates/claims `load_matrix` and seeds the two measured identities
at 100 coins each, then stops cleanly. Measurement reopens the same native world.
Two bots create/claim `load_actor0`/`load_actor1`, transfer seven coins once and
replay the identical payment token. Both actors issue ordinary state queries in
both active windows: one member/claim per town and balances exactly 93/107.
On the same clock each actor quick-moves one granted diamond into/out of its
own chest twice. Received menu revisions/slot changes, empty chest/cursor, exact
player item conservation and finite action completions are mandatory. Remaining
bots walk alongside 64 ordinary villagers. Operator teleports, grants and balance
seeding are fixture assistance, not earned-survival or build-protection coverage.

```sh
RUST_LOG=info .analysis/tools/mem-run.sh -m 5G python3 -m tools.harness run catalog-load -- \
  --smoke --scenario plugin-heavy --clients 2 \
  --server-bin "$PWD/target-load/release/solaris-v2" \
  --bots-bin "$PWD/target-load/release/player-scale-bots" \
  --plugins-dir "$PWD/../solaris-default-plugins" \
  --readiness-seconds 120 \
  --steady-seconds 12 --burst-seconds 8 --recovery-seconds 5 \
  --out "$PWD/.analysis/catalog-plugins-smoke"
```

At least two clients and 12s/8s active windows are required; allow separate
cold-Wasm startup readiness time (the command uses 120s). Smoke is never
performance evidence. Exact positive literal replies are keyed by actor/window,
not generic feedback counts or command echoes. Real `plugin.runtime_ready`,
journal-ACKed commits/replies for **each** package and `/stats.plugin` deltas remain
separate evidence. Keep `RUST_LOG=info` (or explicitly enable
`plugin.runtime_ready=info,plugin.command_result=info`); missing/truncated state
readback, host evidence or ordinary tick/resource telemetry fails closed.
The named setup town proves preparation durability, not a second post-run restart.

The retired `plugin-settlements` control deployed the settlements package, which can no
longer load: core reserves `/settlement` while the package manifest still declares it.
Native `settlement create/list/info` coverage lives in the graphical Loader client path
`loader-v2-settlements-smoke` (no package deployed).

Correctness-only receipt on 2026-10-15:
`.analysis/catalog-native-final-20261015-heavy-v2/plugin-heavy/result.json`
reports `PASS` with the village-wired server. This is a short assisted-gameplay
smoke, not performance, capacity or full-catalog closure evidence.

The separate `python3 -m tools.coreperf.compare` uses `--mobs 20000`, not
`--cows`. `--mode abab` shares cow/sheep/pig/chicken (5000 each by default)
with frozen OLD. `--mode v2-only` adds zombie/skeleton/spider/creeper
(2500 of each of the eight kinds), using `PersistenceRequired:1b`.
OLD summons all eight kinds and accepts `CustomName`, but rejects
`PersistenceRequired:1b`; named distant hostiles disappear across its normal
startup despite retaining their names in the preceding checkpoint. That actual
limitation, not optimized-binary string absence, excludes hostiles from ABAB.
Evidence: `.analysis/codex-logs/loadset-20261015/shared-eight-final-smoke-02/`
contains the before/after OLD native census and successful crafting/hit receipts.
Shared populations are capped at 65536 mobs (16384 identities per species);
v2-only accepts requests up to 100000, a fixture limit rather than a validated
capacity claim. Both use roofed, species-separated, stone-fenced pens plus
64 villagers and exact count/UUID save-reopen checks. This is not a vanilla-parity claim.
Every fixture mob/villager carries `CustomName:'"solaris-coreperf-fixture"'`.
The typed census counts only that persisted marker, then requires exact counts
and the same UUID set across normal reopen; extra marked entities still fail.
Naturally generated villagers remain enabled and outside this benchmark cohort.
Comparison preparation opts both cores into the same `setup_ingress_pacing`
command schedule: at most 16 commands plus one input-ordered echo per batch,
392 weighted-work burst and 480 work/s refill. OLD's public ingress permits
1024 burst and 512 work/s, charging 24 per command and 8 per chat; the remaining
32 work/s covers protocol replies. Incoming server packets wake the monotonic
budget gate; no sleeps or catch-up bursts are used. Generic catalog setup is
unchanged unless explicitly opted in.
`--prepare-only` (without `--setup-smoke`) runs public preparation and normal
checkpoint/reopen validation at the declared population limits without a
measurement GO receipt. It runs no gameplay/performance windows and cannot
establish their acceptance.
Preparation `receipt.json.regions[].process_samples` separates server and bot
RSS/high-water RSS; cgroup totals also include harness work and file cache.
Before any cohort summon, the first preparation region kills only pre-existing
entities of those eight types and villagers: OLD may have spawned cave zombies
before the bot's first gamerule command. This bounded pre-fixture cleanup is
retained as public commands; there are no measured/cohort kills, refills or
disabled AI. Natural AI, health, cramming, movement and random ticks remain enabled.
Creative bots dig/place, perform finite real crafting menu clicks and give one
barehand, nonlethal cow hit; this is not earned survival. Frozen OLD grants the
log into crafting-menu slot 10 while v2 uses slot 38. The comparison resolves
that ingredient by `slot_item=minecraft:oak_log` from the actually received menu
at the same single offered click; an absent item is unavailable, without retries
or a guessed numeric-slot fallback. A bounded setup-only
invocation is `--setup-smoke --mobs 8
--villagers 2 --clients 2 --steady-seconds 2 --burst-seconds 2 --recovery-seconds 2
--warmup-ticks 0 --measure-ticks 0`. Both variants passed this bounded check
with native village generation enabled:
`.analysis/codex-logs/loadset-20261015/shared-passive-native-village-smoke-04/result.json`
contains the shared OLD/v2 receipts and `v2-eight-native-village-smoke-04/result.json`
contains the v2-only hostile receipt under the same parent directory.
These receipts exercise setup, gameplay and persistence, not performance.

Outcomes are `UNSUPPORTED_MECHANIC`, `INFRA_BLOCKED`, `SETUP_FAILED`,
`CORRECTNESS_FAIL`, `PERF_FAIL`, or `PASS`. Absent required oracle/telemetry
cannot pass; a broken implemented fixture/readiness check cannot be accepted
as a smaller successful load.

The **explicit operator defaults**, not catalog numeric SLOs, are sustained
TPS >= 19.5, tick p99 <= 50 ms, maximum tick <= 250 ms, RSS <= 3072 MiB and no
correctness failures. Sustained TPS uses the minimum rolling five-second
subwindow (shorter login bursts explicitly do not apply that sustained check).
Overrides are `--min-tps`, `--sustained-seconds`, `--max-tick-p99-ms`,
`--max-tick-max-ms` and `--max-rss-mb`; effective values are receipted.
Catalog populations/scale rungs are exploratory, never capacity promises.
Smoke windows must total at most 120 seconds and are non-performance evidence:
even `PASS` with `smoke_only: true` is not a capacity/tail-latency result.
Short p99 windows require independent longer repeats near an actual boundary.

Non-smoke runs remain owner-held: `--parity-resume-receipt /absolute/receipt.json`
must contain actual owner-approved `full_parity_verified: true` and
`native_storage_verified: true` evidence. Never fabricate that receipt or
interpret smoke's exemption as permission to resume performance gates.

### Core v2 Java wave QA

The no-Loader Java automation client uses the real Mojang 26.1.2 client under
the development mod loader. `playable` and `regression` launch `solaris-v2`;
their receipts record the actual server command.
`docs/playable/real-client-v2-java-qa.json` declares the W1/UI, bounded W2/W3,
and legacy 21/22/25/26/28/29 scenarios. Select one at a time with
`SOLARIS_REAL_CLIENT_MANIFEST` and `SOLARIS_REAL_CLIENT_AGENT_SCENARIO`, using
`run playable --client java`. Full 25 and 29 use both
before/after phases and the canonical server restart, not a before-only probe.
The manifest records fixture assistance and remaining parity limits; a
prepared artifact or successful compilation is never gameplay acceptance.
Java regression/playable launches use Gradle `--no-daemon` so the build/client
JVMs remain in the current bounded run scope and no detached build daemon
retains memory after the run. Use it for standalone Java QA compilations too.
Set `SOLARIS_SERVER_BINARY` to an immutable executable for frozen-candidate QA.
Regression/playable launch it directly instead of `cargo run`; prepared and
effective receipts retain the exact path and SHA-256. An invalid override fails
closed rather than building a replacement.
For v2, generated config directories also contain vanilla `ops.json` entries
for the two offline fixture usernames only when the selected manifest scenario
permits debug setup. No-debug scenarios generate an empty operator list.

### Native client gameplay

```sh
python3 -m tools.harness run survival --client native
python3 -m tools.harness run survival --client java
```

`java` remains the default. `survival` is a single Python `minecraft_*` scenario
for both clients: a fresh offline world, empty inventory, ordinary clearance of
the generated snow cover when necessary, dirt mining, observed block removal
and pickup, hotbar selection, ordinary placement and observed inventory
consumption. It does not call `minecraft_run_scenario`,
give items, teleport, or substitute an action acknowledgement for an observed
gameplay result. Native also supports the shared inventory probe, the bounded
`entities` profile and an explicitly labeled `core-client` chat-only subset.
Selecting `native` for any other profile fails before launching Java.

The native backend locates `SOLARIS_CLIENT_BINARY` (relative paths are relative
to `SOLARIS_CLIENT_ROOT`), or `target/debug/solaris-client` in
`SOLARIS_CLIENT_ROOT` (default `../solaris-client`). If the default executable is
absent, it builds that sibling's debug binary with debuginfo disabled. An
explicit missing binary override fails rather than falling back. Supply the
verified local content/cache environment described in the native client's
`docs/AGENT_TESTING.md`; the harness inherits it, never downloads or fabricates
private metadata. The state-only headless launcher requires
`SOLARIS_CLIENT_DATA_DIR`, not rendering asset-root/resource-root pairs.
Additional semantic metadata requirements remain the native client's contract;
use its verified content environment.
`DATA_DIR` alone is insufficient when the pinned protocol/gameplay reports are
not installed below its `reports/` directory: inherit their verified
`SOLARIS_CLIENT_*` overrides (including collision shapes and terrain facts).
Omit rendering resource/asset roots for state-only headless runs, not these
gameplay metadata inputs.

Native runs launch `offline-test --automation --headless` against the
harness-started server with ephemeral loopback MCP. They launch neither Java nor
Xvfb. Descriptor readiness uses inotify complete-write events and process pidfd,
followed by authenticated MCP initialization; scenario readiness uses MCP state
events, not sleeps. Each run uses an isolated descriptor/token directory under
the native client's `.analysis/harness-runs/` sandbox. Its paths are recorded
in the canonical receipt; the 0600 bearer token and private nonce are not.

`result.json` under this repository's `.analysis/validation/` records
`client: native`, the client executable path and SHA-256, tool discovery and
`peak_client_rss_kib` from the kernel's lifetime `VmHWM`, plus the shared
scenario transcript and block/inventory assertions. Missing required tools
produce `failed` with every missing tool named in `error`; they never produce a
skip or a pass. Java launcher/Gradle RSS is not mislabeled as Java game-client
RSS: the Java receipt leaves this metric unmeasured.

The shared `survival` server uses an isolated seed-81 world with no plugins by
default, no operators and hostile spawning disabled. By default it builds the current
debug server. `SOLARIS_SERVER_BINARY` may select a known matching prebuilt
server read-only, avoiding a duplicate build; its path and SHA-256 are recorded.
The shared inventory/entity probes and native core-client subset also accept
this read-only server override. CPU capacity is derived by the native runtime;
the retired `chunk_pipeline.worker_threads` setting has no effect. For CPU-pinned
functional runs, the existing server override can point to an executable launcher
that runs `taskset -c <server CPU> <real solaris-v2> "$@"`; launch the harness and
client on a different CPU. The receipt hashes that selected launcher, so retain
the real server executable's path and digest separately when using a wrapper.
Fresh shared probes follow the canonical default terrain configuration. Earth
is a separate explicit opt-in requiring both `worldgen_mode = "tellus_like"`
and `storage_mode = "native"`; these probes do not silently select or fall back
from Earth storage.

For **graphical (windowed native + capture)** validation add `--windowed` to a
shared native profile, for example:

```sh
python3 -m tools.harness run core-client --client native --windowed
```

This explicitly owns Xvfb, launches `play --windowed`, and requires the pinned
`SOLARIS_CLIENT_RESOURCE_ROOT`, `SOLARIS_CLIENT_ASSET_ROOT` and
`SOLARIS_CLIENT_ASSET_INDEX` inputs in addition to protocol/gameplay metadata.
The native launcher uses libX11 to find the mapped window with the exact launched
PID, requests real X input focus on that owned display, and verifies the X server's
focused window before admitting scenario input. Mapping readiness uses X events,
not sleeps. The client still requires its ordinary native Focused event; there is
no automation focus bypass. Missing libX11 or refused focus fails startup.
After the scenario succeeds, `screenshots/native-final.png` and its JSON receipt
retain an actual-window scene target with frame ID, scene/state/resource revisions
and GPU completion. Missing capture evidence fails the run. `present_requested`
does not prove physical monitor scanout; Xvfb is a virtual display. The default
remains **functional (headless native)** and makes no visual claim.

The five focused `native-v2-p1-{armor-stand,signs,piglin,trader,leaves}` scenarios
run through `run scenario ID --client native --windowed`.
They declare operator fixtures separately, preserve intermediate completed GPU
captures even on failure, and include bounded adjacent-input checks. Piglin setup
equips one gold helmet using ordinary inventory clicks; leaves use a nonpersistent
canopy, ordinary trunk mining with two observed log pickups, high random-tick speed
and a persistent control leaf.
Rendered armor/name, sign text/color/glow, piglin admiration and merchant offers
require screenshot review; inventory debit or menu admission alone is not visual
acceptance. Native action/renderer gaps fail closed rather than receiving command
substitutes. For sign-order diagnosis only, `SOLARIS_P1_SIGN_WIRE_TRACE=1` enables
an unmodified loopback forwarding trace of BlockUpdate, BlockEntityData,
OpenSignEditor and SetEntityEquipment into `sign-wire-trace.json`. Armor stand,
helmet-equip and player-observer scenarios record equipment traces automatically.
The trace never modifies packets and is not a client-side observation envelope.

The same module also supplies `native-v2-p2-{slime,enderman,fishing,creaking}`.
Use the identical windowed native/v2 options and a frozen release binary for
arrow/fishing timing. Slime checks a medium-to-small split and no recursive
small split; Enderman checks a real arrow reversal, same-UUID teleport and no
damage; Creaking checks first gaze, look-away movement and second-gaze freezing.
Fishing casts/reels normally into an explicit source-water pool and checks actual
received inventory, plus an early-reel no-reward boundary. Because canonical
native hook rows omit biting, only its bite wait is supplemented by a forwarded
26.1.2 metadata trace (`fishing-wire-trace.json`); no packets are injected or changed.

`native-v2-p1-helmet-equip` exercises ordinary helmet air-use and an occupied-head
swap, respecting vanilla's four-real-client-tick use interval. The bounded empty
repeat check requires exact helmet custody. `native-v2-p1-player-observer` owns a
second native window on the same server/Xvfb, uses explicit focus handoff, and
retains observer-side player UUID/type and held/empty-hand screenshots. Forwarded
equipment rows are associated with the observer's actual LoginFinished UUID,
never guessed connection ordinals (the client may open status connections).
Signs submit their unique `Sign line 1` through the real `Done` button. They retain
daylight color and nighttime before/after-glow captures, then break/pick up the
waxed sign and replace it at the same position; the new editor must be empty.
Empty fields and rendered text/color/glow require screenshot review. Signs also
retain one ordinary name-tag-renamed armor-stand frame for world-text inspection.
This is not an automated label assertion: vanilla armor stands require
`CustomNameVisible` for labels, and current core command fixtures reject that tag.
A blank label in this fixture alone is not evidence of a glyph regression.
The canonical MCP catalog
does not expose historical `client_observe`; do not claim its entity equipment or
packet counters were captured.

`native-v2-p4-{nameplates,advancements,damage,hopper,statistics}` adds the focused
P4 native surfaces. Nameplates retain explicit-visible/default-hidden armor-stand
and targeted/untargeted pig captures; nameplates, advancements and statistics stop
and restart the owned server on the same binary/world, reconnect the same player, and capture
the recovered state. Advancements supply declared ingredients, craft a stone
pickaxe through ordinary menu clicks, scroll with real wheel events on the owned
Xvfb window, and retain story labels, chat and received recipe-book IDs. Damage captures before/hit/after frames:
health loss alone is not hurt-tilt acceptance. Hopper uses the physical Q callback,
checks the exact hopper stack, then withdraws/reopens to detect duplicates.
Statistics walks/jumps and mines/picks up stone/dirt through ordinary inputs, then
requests authoritative counters through the native Statistics button. It captures
General/Items/Mobs, restarts the server, and requests again; a repeated request
must not double-count absolute mining/pickup counters.

The focused `java-v2-p5-parity` and `java-v2-p5-towns` named scenarios use the
real vanilla Java client under private Xvfb, not native UI observations:

```sh
SOLARIS_SERVER_BINARY=/absolute/frozen/solaris-v2 python3 -m tools.harness run scenario java-v2-p5-parity --client java --timeout-seconds 180
SOLARIS_SERVER_BINARY=/absolute/frozen/solaris-v2 python3 -m tools.harness run scenario java-v2-p5-towns --client java --plugins-dir /absolute/server-deployment --timeout-seconds 600
```

Parity retains vanilla statistics/advancement screens, recipe observations,
nameplate/damage frames, Q-to-hopper custody and post-teleport movement. Statistics
and advancement pixels require visual review; absent native-only observation fields
are not inferred. Towns stops each JVM before starting the next identity, revokes
temporary fixture operator grants before testing the leader/guest, checks block
correction and item custody, then restarts the same server/world and repeats.

`loader-v2-settlements-smoke` runs minimal native settlement create/list/info commands with
the real Fabric Loader client against a frozen v2 candidate. It does not build a
server or deploy any settlements package:

```sh
python3 -m tools.harness run scenario loader-v2-settlements-smoke --platform fabric --server-bin /absolute/frozen/solaris-v2 --plugins-dir /absolute/packages --timeout-seconds 600
```

The run owns its world, operator fixture, Loader permission/cache and screenshots.
Server-only manifests need no client-bundle table. Loader live scenarios require
an immutable `--server-bin` and matching `--plugins-dir`; the v1 fixture/build
path was deleted, not retained as a default.

`native-v2-p5-humanoid-armor` uses one windowed native observer and one headless
actor. It equips all four iron, diamond and given magenta-leather pieces through
ordinary air-use, then crafts a cyan leather chestplate through ordinary menu
clicks. The observer captures both dye paths on the player and armor stand,
stand withdrawal/re-equip custody, an iron-armored zombie and diamond-armored
skeleton authored through `/summon ... {equipment:{...}}`. Received component
colors are checked, not inferred from item names. Run with
`--client native --windowed`; this now requires current dye
and summon-equipment support and fails rather than substituting plain leather.

`native-v2-light-seam` is the real-client chunk-admission lighting gate. An
operator-built floating platform carries a stone roof spanning a chunk border
with a torch under it, plus a stone mass whose one-wide tunnel opens only on the
west chunk and holds a torch in the east chunk. Server residency is the square
of `simulation_distance` around the player, so each staged episode saves,
restarts the server on the same world and logs the player in five chunks short
of the seam: only the first chunk is wanted, loaded and lit. After the client
settles on it, one 16-block teleport makes the neighbour wanted (A then B, then
mirrored B then A), plus a close-up beside the seam. Each comparison waits on
client tick events until the capture's terrain metrics report `geometry_ready`
with zero pending/running sections and a quiet arena light stream, then compares
the client's received light (`minecraft_scan_blocks`), the server's own
`LevelChunkWithLight` / `LightUpdate` stream (decoded by a loopback proxy; the
server has no live light query) and a vanilla-propagation oracle at every open
cell of a 24x7x16 region across the border. It also fails on any fully dark
admitted chunk. Requires `--client native --windowed` and the native resource
environment; the server binary comes from `SOLARIS_SERVER_BINARY`:

```sh
python3 -m tools.harness run scenario native-v2-light-seam --client native --windowed --timeout-seconds 900
```

`java-v2-p5-cursor --client java --timeout-seconds 180`
is a bounded graphical cursor pass: whole-stack pickup, secondary single-item
placement, different-item swap, same-item merge, quick-moving crafting inputs,
and closing/reopening with a carried stack. Every ordered server roundtrip
retains exactly eight apples and three diamonds. It records menu/cursor states
and real screenshots for independent review of the SetCursorItem surface.



### Cheap logical native subset

```sh
python3 -m tools.harness run logical-native --client native --timeout-seconds 120
```

`logical-native` is explicitly **functional headless-only**, not an alias for
`playable`, `regression` or `replay`; `--windowed` is rejected. It uses a fresh
seed-81 isolated server, the native executable/metadata overrides above, and
ordinary eight-tick forward+jump input, positive-Z pose progress, absolute
90-degree look and causal native tick waits. Movement origin is
`locally_predicted_reconciled_by_server`, not an independent server track.

The separately declared operator fixture grants one dirt or water bucket to
hotbar slot zero. Three use-on probes target occupied near/far blocks and
occupied bucket placement. Each checks the clicked/adjacent received block
identity, no fluid and unchanged server-observed held stack after an ordered
own-chat echo and another 20 completed native ticks. This is a **stable-state /
no-mutation subset**, not a distinct rejected-action packet acknowledgement or
full resync-event proof: `minecraft_use_item_on` exposes neither ack identity
nor a causal block-resync event. Local `Fail[]`/`Pass[]` is not server rejection.
Unknown/unloaded fixture cells fail closed; no polling or wall-clock sleeps.
An optional `--plugins-dir PATH` retains the existing read-only candidate option,
but supplies no plugin gameplay assertions.

`result.json`, `logical-probe/scenario.json`, `logical-probe/transcript.json`
and `live-tool-schemas.json` preserve the actual instance, build, fixture,
ordered tools and fixed assertions. The original profiles remain unchanged and
unaccepted by this subset: the 900ms stable window, exact centimetre `move_by`
and 5cm convergence, Rust replay validator, full M94 pack, natural crafting /
restart / day-night mobs, twenty-minute survival and graphical captures are
still separate gaps.

### Candidate plugins in native profiles

`survival`, `inventory`, `entities`, the native `core-client` subset and
`native-lifecycle` accept one optional `--plugins-dir PATH` with `--client native`.
The directory must already exist; it is used directly as `[plugins].directory`
and recorded as `details.plugins_directory`, without copying or modifying its
contents. With no option, the existing isolated empty plugin default is retained.
Java and unsupported native profiles reject the option rather than ignore it.

```sh
python3 -m tools.harness run core-client --client native --plugins-dir /absolute/unpublished-candidate/plugins
```

Use an exact fresh package and a matching prebuilt server via
`SOLARIS_SERVER_BINARY`. This generic launch option does not supply plugin-specific
scenarios, Loader bound-view support, unlocks or proof of manufactured-item use.

Loader wire protocol 3 also admits the honest `native` platform. A schema-2
plugin bundle must explicitly list `native` in `loaders`; a Fabric-only bundle
does not admit a native acknowledgement. The native client activates its own
supported declarative content after verifying transferred artifact size and
SHA-256, and accepts only explicitly configured local permissions. This is not
Java mod execution or a Fabric identity. Unsupported content or missing grants
remain fail-closed. Java platform adapters are unchanged.

### Shared inventory and entity probes

```sh
python3 -m tools.harness run inventory --client native
python3 -m tools.harness run scenario inventory-client --client java
python3 -m tools.harness run entities --client native
python3 -m tools.harness run entities --client java
python3 -m tools.harness run core-client --client native
```

For both clients, `core-client` is the chat-only subset and `inventory` is the
shared inventory probe. Wasm/economy/Loader coverage lives in `standard-pack`
and `loader-live`. The named `inventory-client` scenario
runs the same Python inventory probe as native `inventory`: ordinary slot
clicks put one oak log into the actual 2x2 grid, take four planks, conserve two
apples across cursor pickup/return, and select the crafted stack. The broader
Java inventory gate calls this same probe after its economy fixture; there is
no `minecraft_run_scenario` call in the inventory probe.

The new `entities` profile is shared by both clients. In a fresh isolated world
it summons one declared fixture pig, fences its ID/UUID/type, approaches and
feeds it and confirms the carrot debit before supplying a wooden sword as
declared fixture equipment. It confirms a nonfatal ordinary attack's damage,
then kills the same pig through ordinary attacks and verifies removal, real
player drop pickup and held-stack selection. Both clients use this same
loadout and strict assertions. A local interaction dispatch is not treated
as server-side success.

Native `core-client` proves only Play/connection continuity and its own
ordinary chat echo. Its receipt explicitly excludes Wasm, economy and
Loader-required rejection evidence. There is no separate `chat` profile.
Inventory/entities operator setup is recorded separately in
`fixture_commands`; it is not earned survival evidence. These probes require
neither screenshot nor capture tools and make no rendering/GUI-preservation
claim. Missing required tools fail closed before their fixture/actions.
### Native v2 feature probes

These named scenarios use the same isolated server/native lifecycle and
`ProbeSteps` receipts as `entities`, but require both native and v2 explicitly:

```sh
python3 -m tools.harness run scenario native-v2-redstone --client native --windowed
```

The catalog also includes `native-v2-stations`, `native-v2-brewing`,
`native-v2-jukebox`, `native-v2-tnt-fire-light`, `native-v2-dimensions`, and
`native-v2-ranged` (bow/crossbow/trident ammunition debit and received damage).
`native-v2-automation-storage` adds one crafter pulse with no held-power
duplicate, exact single-slot shelf swap/withdraw conservation, and a sculk
sensor active/cooldown/inactive cycle after ordinary lever use. Powered shelf
chains, disabled-slot buttons and the shrieker/warden chain remain separate.
Each uses bounded, declared operator setup and ordinary block/menu/input
actions with exact received-state assertions; the final own-chat echo fences
the live server connection. Per-scenario `scenario.json` and `transcript.json`
are retained even when an assertion fails. Default scenario deadline is 180
seconds. No loader Java `minecraft_run_scenario` implementation is invoked.
The ranged target fixture uses only `NoAI:1b` on a naturally persistent iron
golem and retains an ordered server chat receipt before waiting for that target;
it does not claim general summon-NBT persistence parity.

The station probe completes one netherite smithing operation and conserves
its result through chest/copper-chest close/reopen/return; anvil, enchanting
and crafter coverage is **menu admission only**. Brewing checks actual water
and awkward potion components plus exact item conservation, not the progress
or fuel HUD. Jukebox checks record insertion/debit/ejection, not audible music.
TNT/fire/light checks torch block-light increase, ordinary fire ignition and
UUID-fenced TNT blast, not fire spread/extinguishing. Dimension coverage is
explicit command travel to Nether/End and death/overworld respawn, not portal
entry or a dragon fight. Headless is received-state evidence only; `--windowed`
adds the shared actual-window final capture, not full feature pixel parity.
An unsupported native Source item owner (for example `FlintAndSteelItem`) is a
client-capability failure, not accepted fire/TNT coverage. These probes never
substitute command ignition/priming or arbitrary packet forwarding.
Native fixture emptiness is explicitly standard hotbar/main/armor/offhand slots
0..40: known empty entries are omitted by the client; any unknown/nonempty
standard entry still blocks. Auxiliary body/saddle slots 41/42 are outside that
assertion and their received state is retained, never assumed empty.

`native-v2-storage-adversarial` is the bounded adjacent-input pass: it closes
with a carried diamond, swaps a carried emerald into an occupied chest slot,
closes/reopens again and withdraws both items with exact conservation. This
does not cover crash/reconnect recovery or a full-inventory overflow.

The shared Python Java launcher passes Gradle `--no-daemon`, preserving the
current harness scope's ownership instead of reusing a Gradle daemon from a
previous bounded run. This is launcher isolation, not graphical/gameplay proof.

V2 operator fixtures use the vanilla `ops.json` beside the isolated server
config, not the ignored v1 `[admin]` section. Shared probes write the declared
offline identities with level 4 and record the path/entries in their receipt.
Headless survival has no operator grant; windowed survival grants its one
test identity for the separately labeled playthrough death fixture. That
fixture runs after ordinary mining/pickup/placement acceptance.
After the actual Death-screen Respawn click, the playthrough uses the bounded
event-driven Play wait before checking live health. Respawn temporarily clears
the native loaded presentation, so an immediate `in_play=false` is not itself
a disconnect. There remains no protocol respawn fallback; a failed UI request,
Play timeout or missing restored health fails the gate.
`ProbeSteps.observe/wait(wait_for_play=True)` opts only declared native
respawn/travel boundaries into that wait; native dimension-command predicates
use it too. Ordinary probe observations retain the strict `in_play` assertion.
The Title/connect probe uses a typed preplay observer rather than monkey-patching
the shared observer: raw Title waits remain legal, while an explicit Play wait
keeps the same bounded strict contract.



Normal server runs write tracing events only to `logs/latest.log` and
`logs/debug.log`. The canonical `run` entrypoint sets
`SOLARIS_HARNESS_LOG_STDOUT=1` for its subprocesses, adding an INFO stdout layer
so existing readiness events and per-run captured server logs remain available.
`RUST_LOG` controls `logs/debug.log`, not the captured stdout threshold.
The reconnect observer waits for the INFO `play session unregistered` event,
not completion of the asynchronous player save; assert reconnect/disk
persistence separately.
Do not enable this variable for ordinary operator launches.

### CPU and memory bound on every run

`run` and `client` put the whole invocation into one bounded systemd user scope
before doing any work, so a workspace test run cannot take the machine away from
the session the owner is working in. Cargo, every test binary, the harness's own
server/client/Xvfb children and the private backends all inherit that one
cgroup, so each bound is a total, not a per-process, allowance.

| Property | Default | Env |
| --- | --- | --- |
| `CPUQuota` (+`CPUWeight=50`) | every physical core | `SOLARIS_HARNESS_CPU_QUOTA` |
| `MemoryHigh` | `off` (no throttle) | `SOLARIS_HARNESS_MEMORY_HIGH` |
| `MemoryMax` | `12G` for `test` / `correctness`; `4G` for other profiles and clients | `SOLARIS_HARNESS_MEMORY_MAX` |
| `MemorySwapMax` | `1G` | `SOLARIS_HARNESS_MEMORY_SWAP_MAX` |

```sh
python3 -m tools.harness run correctness              # the defaults above
SOLARIS_HARNESS_CPU_QUOTA=150% python3 -m tools.harness run test
python3 -m tools.harness run test -- -p mc-core --lib inventory  # focused Rust loop
SOLARIS_HARNESS_MEMORY_MAX=12G python3 -m tools.harness client   # heavier client runs
SOLARIS_HARNESS_CPU_QUOTA=off  python3 -m tools.harness run test  # unbounded, opt-out
```

A memory cap only protects the session if the run fits in what the machine has
free: on 2026-09-16 a `correctness` run under the then-defaults (`5G`/`7G`, ~11 GiB
already held by the desktop) was killed by `systemd-oomd`, which acts on the
*whole* `user@1000.service` tree's 50% memory-pressure limit and then picks the
largest units in it — the run's scope *and*, in the first kill, an interactive
terminal scope. A cgroup cap does not stop that, because oomd never looks at the
scope's own limit. Measured afterwards: a full `test` phase peaks at **1.13 GiB**
(483 samples of the scope's `memory.current`), so the runs never needed the
allowance they had — the tree was already near its limit and the run's share
tipped it. Those historical measurements set the then-current `3G`/`4G`
limits; `MemoryHigh` is now off, and current test-host measurements require
`MemoryMax=12G` for full `test` and `correctness` runs. Other profiles and clients retain
`4G`. `run`/`client` refuse admission when `MemAvailable` is less than the
selected `MemoryMax` + 1 GiB, naming the knob to change:

```text
[harness] refusing to start: the run may take up to 20G but only 12701 MiB is available
```

That refusal is not acceptance evidence — it means the gate did not run, and the
receipt is the harness's own message rather than a profile result. Beyond it, an
over-large run is killed inside its own scope (`oomd`/cgroup OOM) instead of
dragging the desktop into swap: the run fails loudly, the session keeps running.
Full `run test` / `run correctness` use a 12 GiB cap. Shared coding agents run
them through `.analysis/tools/mem-run.sh -x -m 12G`, taking every slot
exclusively; the direct harness defaults to the same 12 GiB cap. Focused or
per-crate runs retain `.analysis/tools/mem-run.sh -m 8G`. The shared guard
applies its slot/admission policy, and an already guarded harness does not
create another independent scope. `SOLARIS_HARNESS_MEMORY_MAX` still overrides
the direct default (including an explicit `8G` for a proven-focused run).

```sh
# Full workspace gate: exclusive, all slots.
.analysis/tools/mem-run.sh -x -m 12G env CARGO_BUILD_JOBS=6 \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  python3 -m tools.harness run test
# The same exclusive cap applies to `run correctness`.

# Focused/per-crate work retains the smaller shared scope.
.analysis/tools/mem-run.sh -m 8G env CARGO_BUILD_JOBS=6 \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -p mc-core --lib entity_state_persistence_tests
```

`run test` (including the test stage of `correctness`) requires `cargo-nextest`
(CI installs 0.9.143) and runs `cargo nextest run --workspace --all-targets`,
scheduling across test binaries with CPU-count parallelism. The gate disables default-filter exclusions,
retries no failed tests, runs the entire selected suite after failures, and fails
closed if nextest is missing, a test fails, or no tests are found. Per-test
durations are retained in `test.log`.

Explicit `RUST_TEST_THREADS` (first priority) or `NEXTEST_TEST_THREADS` selects
the gate's default concurrency; nextest's `--test-threads=N` overrides it.
Focused runs with trailing package/target/name selectors use the same nextest
runner. Unsupported Cargo/libtest arguments and non-executing flags such as
`--no-run` are rejected, never rerouted to another runner (put
`--test-threads=N` before any `--` separator).
No doctests are added: the full gate still uses `--all-targets`.

Criterion 0.5.1 `harness = false` bench targets (including `mc-world`'s
`light_engine`) remain in the all-target gate and execute in test mode, not timed
benchmark mode; nextest supports Criterion's test listing. Explicit `cargo bench`
remains the performance-measurement path.

The host integration fixture remains a real SDK/WIT component. Its release
guest build uses `opt-level=z`, LTO and one codegen unit to reduce repeated
Wasmtime/Cranelift compilation; no fixture test modes are excluded and production
engine limits/configuration are unchanged.

Each concurrent test can carry its own server and component host. Measure the
scope peak before increasing concurrency; use an explicit environment override
for a proven-heavy run, not a process-wide guest mutex/semaphore or changed
production Wasmtime limits. The `component-host` nextest group retains the
previously measured four-concurrent-test host budget; other packages can fill
the remaining CPU slots. Simultaneously live guests and isolation assertions
must remain intact.

Successful `test` receipts record `details.memory_peak_bytes`,
`memory_limit_bytes`, `memory_cgroup`, `memory_events`, `memory_swap_peak_bytes`,
`rust_test_threads_env`, `test_runner`, and `nextest_test_threads_env`. These kernel cgroup-v2 counters are read while the
scope is still alive. An unavailable counter is reported as unavailable, never
as zero. Scope peaks include Cargo, compilation and cached file pages, not just
anonymous test allocations. For a concurrency decision, build first with
`cargo test -p PACKAGE --test BINARY --no-run`, then run only that executable
in its own mem-run scope and distinguish `memory.stat` anonymous and file pages.
Transient scopes disappear on exit, so post-hoc reads are not evidence.

Historical measurements on 2026-10-11 (before v1 deletion), not current native
test-binary names or capacity receipts: shared `mem-run -m 5G`, CPUs 4–11,
`CARGO_BUILD_JOBS=2`, debug information disabled and four test threads:

| Exercised test binary / gate | Scope `memory.peak` | Receipt |
| --- | ---: | --- |
| `mc-net` library | 3,956,330,496 bytes (3.685 GiB) | [passed](../.analysis/validation/20261011T120932-test-6o4lv0tx/result.json) |
| `mc-test-harness` `wasm_npc_operations` | 2,732,822,528 bytes (2.545 GiB) | [passed](../.analysis/validation/20261011T121111-test-zlttw7ic/result.json) |
| `mc-plugin-host` `host_bounds` (no guest gate) | 5,368,709,120 bytes (5.000 GiB) | [passed](../.analysis/validation/20261011T131353-test-yoxr08w1/result.json) |
| Earlier full workspace `run test` (retired whole-test guest gate) | 5,368,709,120 bytes (5.000 GiB) | [passed](../.analysis/validation/20261011T121438-test-wxk3iblu/result.json) |

The earlier full run completed with 6,675 passed, zero failed and 191 existing
ignored tests; its exact-cap peak established no spare headroom. A subsequent
ungated four-thread `host_bounds` scope had only a 37.137 MiB margin even with
zero `max/oom/oom_kill` events and zero swap, so aggregate completion alone was
not used to set policy.

The build-first, test-executable-only measurement under the focused shared
8 GiB cap/all CPUs recorded an exact scope peak of 5,225,897,984 bytes (4.867 GiB).
At 50 ms sampling, anonymous memory peaked at 5,211,590,656 bytes (4.854 GiB),
file pages at 1,675,264 bytes; `max/oom/oom_kill` and swap peak were zero.
This leaves 3.133 GiB of scope headroom, so the four-thread default stays and
no guest gate or per-binary override is retained. Anonymous/file sampled peaks
are not exact kernel per-kind high-water counters. See the
[isolated receipt](../.analysis/codex-logs/parity-w1-gate/memory-host-bounds-isolated-4.json)
and [raw run](../.analysis/codex-logs/parity-w1-gate/memory-host-bounds-isolated-4.log).

The subsequent ungated full workspace run at 8 GiB passed 6,727 tests with
191 existing ignores, but reached its exact cap, recorded 3,964 `max` events
and used the entire 1 GiB swap allowance (`oom/oom_kill=0`). This is pressure
evidence, not an acceptable normal full-run budget:
[8 GiB receipt](../.analysis/validation/20261011T143857-test-874dl6gp/result.json).

The final exclusive `mem-run -x -m 12G` re-proof used all CPUs, six Cargo jobs,
debug information disabled and the unchanged four-thread libtest default.
It passed the canonical 101 result rows: **6,727 passed, zero failed, 191
existing ignored**. Exact `memory.peak` was **5,505,695,744 bytes (5.128 GiB)**
against **12,884,901,888 bytes (12 GiB)**; `max/oom/oom_kill` and swap peak were
all **zero**, leaving 6.872 GiB of scope headroom in this measured run.
See the [final receipt](../.analysis/validation/20261011T150707-test-9x2pjg9v/result.json)
and [test log](../.analysis/validation/20261011T150707-test-9x2pjg9v/test.log).
No whole-test guest gate, production Wasmtime limit change, or per-binary
thread override is retained. Other hosts and cold compilation still require
the normal admission check; this run is not a universal capacity guarantee.

The CPU default is every physical core, and the memory caps are what actually
protect the session: measured on the owner workstation (12 CPUs, 15 GiB), the
freezes were memory pressure and writeback (PSI `cpu full` stayed at 0), so the
quota is generous while `MemoryMax` is the hard wall. Twelve spinners in a 4 s
window consume 45.6 CPU-seconds unbounded, 24.6 under `CPUQuota=600%` and 4.1
under `CPUQuota=100%`; the earlier suite's 1.13 GiB peak (483 one-second
`memory.current` samples) predates the expanded guest/parity tests and is not a
capacity promise for the current suite. A *tighter* quota with unmatched threads
starves every individual test,
which is how fixed 5 s packet waits start failing on machines that pass
unbounded. A run without a user systemd scope prints a warning
and continues unbounded rather than failing closed, because the scope is a
machine protection, not acceptance evidence; the receipt records the profile's
own result either way.

### Prerequisites

- Repository root as cwd; `python3`, `cargo`, and (for Java-client profiles) Java 25
  (`java -XshowSettings:properties -version` must report specification 25),
  an executable `../solaris-loader/gradlew`, and Xvfb for every Java
  graphical profile. Native `survival` needs neither. Runtime readiness on Linux uses inotify/pidfd event waits
  plus Xvfb process management and MCP state events — not sleeps.
  Set `SOLARIS_LOADER_ROOT` to use a different Loader checkout; the default
  is the sibling `../solaris-loader`, never an in-core compatibility copy.
- Wasm package coverage comes from `standard-pack` over
  `../solaris-default-plugins`, not an in-core plugin copy.
  CI pins both sibling source revisions in `.github/workflows/ci.yml`; the
  Java test command runs once and does not retry test failures.
- Loopback must be free on the needed ports (default MCP `39095`, scenario
  server ports such as `25565`/`25567`, per-run agent bridge ports). A run that
  cannot bind fails closed; there is no invented exit code.
- Client launch/check requires credentials in the environment (see below).
  `python3 -m tools.harness client` without `SOLARIS_CLIENT_MCP_TOKEN` exits 2
  before Gradle starts.

### Profiles and recipes

| Profiles | Coverage and recipe |
|---|---|
| `correctness` | Rust L2: formatter, strict workspace Clippy, code-health, workspace/all-target tests. `python3 -m tools.harness run correctness` |
| `fmt`, `clippy`, `code-health`, `test` | Individual Rust gates; CI uses the same profiles. `test` requires cargo-nextest and runs all workspace targets with CPU-count parallelism; trailing package/target/name selectors use the same nextest runner, for example `python3 -m tools.harness run test -- -p mc-core --lib inventory`. |
| `harness-check` | Fail-closed receipt/process lifecycle regressions plus private shell-engine syntax (`bash -n` over every backend). `python3 -m tools.harness run harness-check` |
| `standard-pack` | Dispatches to `../solaris-default-plugins/tools/run_integration.py`; stages the sibling-owned package deployment, builds `mc-edge --bin solaris-v2`, waits for readiness and requests clean console shutdown. There is no server `--check`. Requires the sibling source, vanilla content and guest build target; `SOLARIS_DEFAULT_PLUGINS_ROOT` selects that checkout. `--real-client` adds the sibling graphical `/money` scenario under Xvfb. `python3 -m tools.harness run standard-pack` |
| `build` | Server build (`cargo build -p mc-edge --bin solaris-v2`); `--release` selects the locked release build of the same binary. `python3 -m tools.harness run build --release` |
| `java` | Gradle bridge/agent/Loader module tests (`:bridge-core`, `:java-agent`, `:loader-core`, `:loader-fabric`, `:loader-neoforge`, `:loader-forge`). Requires a Minecraft client jar via `SOLARIS_CLIENT_JAR` or the Loader's documented local path. CI downloads the version declared in Loader `gradle.properties` and verifies Mojang's SHA-1 before testing. `python3 -m tools.harness run java` |
| `fixture-check` | Reproducible Loader fixture verification (`tools/build-loader-live-gate-fixture.sh --check`); requires `ffmpeg` with `libvorbis`, plus ZIP and coreutils commands. CI installs the audio encoder explicitly. |
| `installer` | Installer self-test backend. |
| `survival` | One Python MCP mine/pickup/place scenario for Java (default) or windowless native. `python3 -m tools.harness run survival --client native`; timeout defaults to 120 s, accepted range `(0, 120]`. |
| `core-client` | Java (default) or native: server/own-chat subset only; the receipt excludes Wasm, economy and Loader-required rejection. `python3 -m tools.harness run core-client --client native` |
| `inventory` | Java (default) or native: the shared crafting/cursor/selection probe. Named `run scenario inventory-client --client java` runs the same path. |
| `entities` | One shared, bounded pig feeding/damage/death/drop probe with declared operator fixture, UUID/type fencing and authoritative inventory/removal checks. `python3 -m tools.harness run entities --client native` (or `java`). |
| `loader-live` | Loader gate via `loader.run`; requires `--platform`, immutable `--server-bin` and matching `--plugins-dir`. `python3 -m tools.harness run loader-live --platform neoforge --server-bin /absolute/solaris-v2 --plugins-dir /absolute/packages` (also `fabric`, `forge`). |
| `regression` | Real-client regression backend over `regression.sh`; bare run defaults to `--run`. `python3 -m tools.harness run regression --check` prepares only. |
| `playable` | Ordinary twenty-minute no-debug survival loop (`docs/playable/real-client-playable-loop.json`, scenario `playable-04-twenty-minute-survival-loop`, `playable.toml`, fresh world, default timeout 1500 s). `python3 -m tools.harness run playable` executes; `--check`/`--prepare` only prepare. |
| `replay` | Core replay seed-81 gate (`docs/real-client-regression/manifests/core-replay-seed-81.json`, scenario `core-actions-seed-81`, fresh world, default timeout 180 s). `python3 -m tools.harness run replay` |
| `oracle` | Vanilla oracle backend: bare `oracle` is readiness inspection (`prepared`, blocked/degraded when artifacts are absent); `python3 -m tools.harness run oracle --run` executes comparisons. |
| `oracle-check` | Oracle runner self-check backend. |
| `catalog-load` | Owner-catalog v2 open-loop protocol loads with full public control fixtures, actual wire/native work oracles and classified gaps. Separate prebuilt server/bot processes; no implicit builds. `python3 -m tools.harness run catalog-load -- --scenario dense-hostile --server-bin PATH --bots-bin PATH --clients 10 --smoke`; base preparation is separate from the <=120-second measurement limit. Exact JOIN-01 remains unassisted. Non-smoke requires owner-approved parity/native-storage resume evidence; MIX blockers never become control aliases. |
| `seed-review` | Seed owner-review backend (module invocation, default timeout 120 s). Captures evidence; it does not grant owner `ACCEPT`. |
| `seed-contact-sheet` | Seed contact-sheet capture preparation; always `prepared`, never a gameplay pass. |
| `bucket-resync` | Real-client bucket resynchronization debug loop; `--check` prepares only, `--real-client` (default) executes. Deleted automated/all v1 layers are not available. |


`--release` only changes `build`; every other profile ignores it.
`--timeout-seconds N` overrides the scenario default for `core-client` /
`inventory` (600 s), `loader-live` (600 s), `playable` (1500 s) and `replay`
(180 s); regression/playable/replay require positive whole-second values.
Fractional values fail explicitly. Unknown profiles, nonzero commands,
exceptions, and prepared-only runs never pass.

### Scenario catalog: `explain` and `run scenario`

The catalog in `tools/harness/scenarios.py` is data over the profiles above — it names,
for each discoverable scenario, the existing profile plus the exact selector (or the
profile's own scenario id), the tests it requires, its declared prerequisites, the source
of its assertions, what it proves and what it does not. It is not a second runner: a
`run scenario` dispatch uses the same artifact directory, bounded scope, cleanup and
receipt writer as `run PROFILE`.

```sh
python3 -m tools.harness explain                       # list scenario ids
python3 -m tools.harness explain inventory-client
python3 -m tools.harness run scenario inventory-client
python3 -m tools.harness run scenario native-v2-storage-adversarial --client native
```

`explain` reads nothing but the catalog: it starts no server or client, downloads nothing,
needs no credentials and writes no run artifact. Its "declared prerequisites" line is a
description; the owning profile/backend stays the only authority that enforces them.

`run scenario` adds one fail-closed check the raw profiles cannot express: the receipt must
show that the selected check actually ran. `details.named_scenario` records the id, evidence
kind, resolved profile and selector, the required test paths, the bound `scenario_id`/manifest/
seed/fixture, the declared prerequisites, the assertion sources, what the scenario proves and
does not prove, the revisions of the participating repositories, the effective cwd of every
command, the engine counts (`selected`/`executed`/`passed`/`failed`/`ignored`/`filtered_out`)
and any missing tests. When a named entry declares a `scenario_id`/manifest, `run_replay`
binds exactly those values instead of its `setdefault` defaults; a bare `run replay` keeps the
documented environment overrides unchanged.

A named run fails when the id is unknown, when free-form trailing arguments are passed, when a
required test is absent from the log, when the engine reports no counts, when nothing was
selected or executed, or when a selected test failed. A profile that reports `prepared`
(preparation only) keeps that status and exit 0 — it is still never `passed`. Graphical entries
never fall back to TCP evidence.

`regression`, `playable` and `replay` append `--run` unless a mode token is
already present (`--check`, `--prepare`, `--run`, `--validate-run`). `--check`
validates the Gradle runClient adapter; `--prepare` creates a run directory and
observation templates; `--run` starts Solaris, launches the client, and records
logs; `--validate-run <dir>` checks an existing run shape. `--check` and
`--prepare` receipts carry `mode: prepared` and exit 0 with status `prepared` —
that is explicitly not a gameplay pass. `oracle` without `--run` and
`seed-contact-sheet` always behave the same way.

### Client and MCP launch, check, and credentials

The default (platformless) development client lives under the
`../solaris-loader/fabric-agent/` directory, but despite that
directory name it is a **NeoForge** mod: it applies `net.neoforged.moddev`,
its entrypoint is `dev.solaris.agent.neoforge.SolarisClientAgentMod`, and its
fixed launch task is `:fabric-agent:runClientMcp`. The three Loader adapters
(`loader-fabric`, `loader-neoforge`, `loader-forge`) register the same
Configuration-state manifest/ack payloads through their native 26.1.2
networking APIs and launch as `:loader-<platform>:runClientMcp`.

Required credentials for `client` (launch and `--check` alike):

- `SOLARIS_CLIENT_MCP_TOKEN`: random bearer token, no default.
- `SOLARIS_CLIENT_MCP_PORT`: free IPv4 loopback port, default `39095`.
- `SOLARIS_CLIENT_MCP_GAME_DIR`: isolated game dir; default
  `../solaris-loader/fabric-agent/run-mcp`, or
  `.analysis/minecraft-loader-mcp/<platform>` when `--platform` is given.
- `SOLARIS_CLIENT_MCP_USERNAME`: `1..16` ASCII letters/digits/underscores;
  default `SolarisMcp` (platformless) or `SolarisLoader` (Loader platforms).

```sh
SOLARIS_CLIENT_MCP_TOKEN=local-check-token \
  python3 -m tools.harness client --check
export SOLARIS_CLIENT_MCP_TOKEN="$(openssl rand -hex 32)"
export SOLARIS_CLIENT_MCP_PORT=39095
export SOLARIS_CLIENT_MCP_GAME_DIR=.analysis/minecraft-mcp-primary
export SOLARIS_CLIENT_MCP_USERNAME=SolarisMcpA
python3 -m tools.harness client
python3 -m tools.harness client --platform neoforge   # Loader client on another port/dir/user
```

Normal launch refuses to start when the MCP port is already in use; the
in-client bind remains the final authority if a process races for the port.
Explicit token/port values override stale JVM properties from an earlier run.
An HTTP 401 means the caller reached an endpoint with a different bearer token
— verify the port and process instead of retrying it as a transient failure.
`--check` validates Java 25, configuration, MCP transport tests and the Gradle
adapter without launching Minecraft. Use a second port, token, game directory,
and username for multiplayer gates. The endpoint is
`http://127.0.0.1:<port>/mcp`.

MCP smoke against an already-running client (`SOLARIS_CLIENT_MCP_TOKEN` still
required; default endpoint derives from `SOLARIS_CLIENT_MCP_PORT`):

```sh
python3 -m tools.harness mcp
python3 -m tools.harness mcp --server-address 127.0.0.1:25565 --exercise-input --disconnect
```

The smoke checks the tool catalog, calls `minecraft_observe`, and optionally
connects, waits for Play, reads the block below the player, scans blocks,
lists entities, reads the recipe book, exercises one input, runs a
deterministic in-client scenario (`--scenario-id`), and disconnects. Inventory,
entity, login, and client lifecycle waits block on packet/lifecycle state
notifications; the separate tick notification drives only tick progression
itself. Every timeout is failure, not success. The current bridge also provides
push-driven motion and entity-removal waits; ordinary primary/secondary
container-slot clicks wait for an applied server container update, so plugin
inventory menus work without coordinate clicks. Canonical interaction checks
fence reach, raycast, and authoritative world state first. Focused bridge,
Java, and client-mod tests are tooling-path evidence only.

The regression runner is fail-closed on scenario provenance: for `--check` and
`--run`, `SOLARIS_REAL_CLIENT_AGENT_SCENARIO` must name exactly one scenario in
`SOLARIS_REAL_CLIENT_MANIFEST`; an implemented but undeclared debug scenario is
not valid evidence for a no-debug playable manifest.

### Supported per-run environment overrides

The harness never reads a config file for these; export them before `run`:

| Variable | Effect (default) |
|---|---|
| `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS` | `run test` default concurrency (CPU count by default; `RUST_TEST_THREADS` wins). Nextest's `--test-threads=N` CLI option overrides it. |
| `SOLARIS_REAL_CLIENT_MANIFEST` | Regression pack manifest (`docs/real-client-regression/manifests/m94-regression-pack.json`; `playable` presets its loop manifest, `replay` its seed-81 manifest). |
| `SOLARIS_REAL_CLIENT_RUN_ROOT` | Local artifact root; the harness presets it under the run artifact `regression/` dir (backend default `.analysis/real-client-runs`). |
| `SOLARIS_REAL_CLIENT_SERVER_CONFIG` | Server config (`example.toml`; `playable` presets `playable.toml`). |
| `SOLARIS_REAL_CLIENT_FRESH_WORLD` | `1` copies the config into the run dir with a fresh per-run world (`playable`/`replay` preset `1`). |
| `SOLARIS_REAL_CLIENT_SERVER_SEED` | Optional signed 64-bit decimal overriding `data.seed`. |
| `SOLARIS_REAL_CLIENT_SERVER_ADDR` | Server address for the in-client driver (`127.0.0.1:25565`). |
| `SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS` | Gradle runClient adapter timeout (180; `playable` 1500, `replay` 180). `--timeout-seconds` overrides it for `playable`/`replay`. |
| `SOLARIS_REAL_CLIENT_SERVER_READY_TIMEOUT_SECONDS` | Server-readiness wait (120). |
| `SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL` / `AGENT_SECRET` / `AGENT_PORT` / `AGENT_DRIVER` / `AGENT_SCENARIO` | Loopback JSON bridge URL, per-run bridge secret (required for agent-driver mode), bridge port, internal driver path, scenario id. Second-client mode additionally needs `SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET` plus bridge URL or port. |
| `M79_ORACLE_REPORT_DIR` / `M79_ORACLE_CHECK_ROOT` | Oracle report/check roots (harness presets them under the run artifact). |
| `SOLARIS_ENTITY_BENCH_*` | Scale workload knobs (`MODE`, `OUT_DIR` preset under artifact `bench/`, clients/regions/entities, warmup/measure ticks, budgets, cpuset). |
| `SOLARIS_VALIDATION_RUN_DIR` | Canonical run dir injected into every backend env; used by seed-review evidence. |

Existing `SOLARIS_REAL_CLIENT_*` scenario/configuration overrides remain
supported. Artifact-building utilities are not duplicate validation entrypoints.

### Artifact and result lookup

Each `run` creates an isolated `.analysis/validation/<UTC stamp>-<profile>-<id>/`
directory with `result.json`, per-command logs, and scenario evidence. `status`
is `passed`, `failed`, `prepared`, or `interrupted`; only real completion is
`passed`, and failure exits nonzero. `result.json` records `profile`, `status`,
`scope`, `commands`, `exits` (a command that cannot start keeps an invented
`null`, never a fabricated code), `logs` (repo-relative), `started_at` /
`ended_at`, `duration_seconds`, `details`, and `error`. An unknown profile still
writes a `failed` receipt (`unknown-profile`) listing the known profiles.
Graphical profiles share one Linux process/readiness implementation and clean up
their owned server, client, and Xvfb processes; readiness is driven by process,
log, and client-state events.

### Failing-command diagnostics and interruption cleanup

- Start from `result.json`: take the nonzero entry in `exits`, read the matching
  `commands` argv and `logs` file. `details.error` names the failing backend;
  `details.mode` tells preparation (`prepared`) from execution (`run`).
- Hosted `test` and `loader` jobs upload their failed receipts as
  `test-failure-receipts` and `loader-failure-receipts` (seven-day retention).
  Download the artifact to read the actual command error; the console's
  `[harness] ... see .../test.log` line alone is not the underlying failure.
- Failing backends fail closed: nonzero commands raise with the log path, and
  `loader-live`/`core-client`/`inventory` additionally raise when the scenario
  report is not `passed`. `--check`/`--prepare` never produce a gameplay pass,
  so a `prepared` receipt after you expected execution means a mode flag was
  passed (or defaulted) wrong — not a green run.
- `KeyboardInterrupt` (Ctrl-C) yields status `interrupted`, exit 130, and keeps
  every command, exit, and log recorded so far. Owned server/client/Xvfb
  processes are stopped by the shared runtime; if a port is still held after an
  interruption, find the stray Gradle/Minecraft/Xvfb process holding it and stop
  that process — do not delete `.analysis/validation/` evidence or historical
  logs.
- Second real-client (multiplayer) failures almost always mean a missing
  `SECOND_AGENT_SECRET`/bridge URL/port pair; the backend errors name the exact
  variable.

### Original scenario and performance boundaries

- `playable` retains the complete twenty-minute no-debug survival loop; do not
  shorten it or substitute a debug scenario and call it playable.
- `replay` is the core seed-81 action replay, not a general regression short-cut.
- `regression` defaults to the m94 pack and rejects protocol bots, wire-probes,
  `mc-test-harness` clients, and mocks as participants.
- `oracle` without `--run` reports degraded/blocked readiness only; it needs the
  local artifacts named by each manifest to execute.
- Retired `entity-scale` / `living-world-scale` results retain their original
  release-build and budget scope as history. Use `catalog-load` for current
  public-protocol workloads; debug numbers are not optimized performance evidence.
- Terrain capture (`seed-review`, `seed-contact-sheet`) never grants owner
  `ACCEPT`; that decision lives outside the harness.

### Retained evidence (2026-09-06, not current release status)

This is historical run evidence, not the current workspace version or a fresh
gameplay result. [MEMORY.md](MEMORY.md) owns current checkpoint evidence; the
published version is described in [the README](../README.md). Unrerun failures
below remain unclosed, but do not establish failure on every later revision.

- Green: `harness-check` (9 tests plus shell syntax), 49 manifest tests,
  `java` / `installer` / `fixture-check` / `oracle-check`, and the graphical
  inventory plus core-client compatibility gate at
  `.analysis/validation/20260906T084650-inventory-njrj95iy/result.json`.
- Red: that full alpha `playable` run **failed** after natural spruce
  pickup/crafting — no dry crafting-table placement target in the snowy terrain
  under test (not rerun here; never mark green). The frozen load matrix
  remains 20 PASS / 22 FAIL with no owner terrain `ACCEPT`, and the full core
  redesign is incomplete.
- At that checkpoint, workspace version and latest published prerelease were
  `v0.0.3-alpha.1` (workspace version without the `v`). Tag workflow
  [34356580571](https://github.com/kaiserproger/solaris/actions/runs/34356580571)
  passed, including both native Linux release builds and archive checks.
  Public curl installation, configuration validation and isolated startup passed
  on Linux x86_64; this is not full survival acceptance. Evidence:
  `.analysis/releases/public-v0.0.3-alpha.1/receipt.json`.
  The older frozen local field-test archive remains
  `.analysis/releases/v0.0.3-alpha.1/solaris-x86_64-unknown-linux-gnu.tar.gz`.
  It passed isolated version/config/startup checks, not full survival acceptance.
  SHA-256: `925a825b709e5e44d8ad17957d741e3751ed955dba3e510ee4baa8ee78ed6b36`.
- The owner's field-test instruction at that checkpoint was to stop fixed-seed
  tuning and test the frozen archive before varied-seed graphical exploration.
  Record each seed, generated config, steps, logs and screenshots; retain failing
  seeds as reproductions, not as the only terrain used for acceptance.

## Useful External Candidates

| Tool | Decision |
|---|---|
| `lean-ctx` | Researched 2026-06-11. It offers MCP, shell compression, memory, and context governance, including OpenCode setup. Do not install it on top of RTK+Headroom by default; overlapping hooks/context layers can conflict. Revisit only if RTK/Headroom are not enough. |
| Headroom bundled tools | Available through `headroom sg`, `headroom diff`, and `headroom loc` for AST search, structural diffs, and LOC/repo-shape probes. Use explicitly when they add value; do not replace normal repo validation. |

## OpenCode Commands

Global commands already present in `~/.config/opencode/commands/`:

| Command | Purpose |
|---|---|
| `/agent-harness` | Router that points to the native harness commands. |
| `/harness-run` | Spec-first implementation with native subagent cards. |
| `/harness-refactor` | Behavior-preserving refactor with review gates. |
| `/harness-cleanup` | Behavior-preserving repo slop cleanup. |
| `/harness-cleanup-cli` | Old all-in-CLI cleanup path; use only when explicitly requested. |
| `/harness-preflight` | Deterministic agent/config/repo checks. |
| `/harness-dry-run` | Generate harness prompts/artifacts without LLM phases. |

## Headroom Notes

Headroom's own CLI help says `headroom wrap opencode` does not exist. The
supported opencode route is `headroom proxy` plus provider base URL overrides.
Do not enable that automatically in this repo because the global opencode setup
uses provider auth/plugins and a forced proxy can break model access.

Headroom MCP is not enabled in `opencode.json`. If the owner asks for it, use
`~/.local/bin/headroom mcp serve` as the local command and
check startup cost before leaving it enabled.

## Session Logs

Inspect metadata before content and select at most three sessions whose recorded
`cwd` matches this repository. Large JSONL logs must be stream-filtered rather
than dumped into model context.

| Source | Command |
|---|---|
| Codex CLI JSONL | inspect `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`; verify `session_meta.payload.cwd` first |
| OpenCode session list | `opencode session list` from the repository root |
| OpenCode SQLite DB | `sqlite3 ~/.local/share/opencode/opencode.db` |
| Text parts | Query `part` joined with `message`/`session`; useful content is usually in `part.data` where `$.type == "text"`. |

Avoid `opencode export --sanitize` for detailed local forensic work because it
redacts the text/tool payloads that usually contain the useful facts.

## Negative-Code Gate

For any non-trivial change, include negative-code checks in the one independent
final review required by `AGENTS.md`:

| Diff size | Gate |
|---|---|
| Small/single-file | Self-review, then ask one concise independent reviewer to check scope, duplication, fake abstractions, wider-than-needed config, and stale docs. |
| Non-trivial or risky | Give the same single reviewer the concrete behavior, diff scope, and validation evidence. Do not add a separate slop-review agent. |
| Explicit harness request | Adapt the harness to one final review phase; do not run multiple reviewer roles unless the owner explicitly asks. |

Final reports should say whether the negative-code review ran and what it found.
