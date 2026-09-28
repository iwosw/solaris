# Archived checkpoint log — part 15 of 17

Era: 09-23. Chronological checkpoint history moved out of `docs/MEMORY.md` so
the live cursor stays small. **Not startup context** (see `AGENTS.md`); read it when
a question is about this era, not to learn the current state.

## Sections in this part

- CP-014 R1 code path — headless canonical village chain passed (2026-09-23)
- Deferred client acceptance, CP-015 source-only placement correction (2026-09-23)
- CP-018 inventory/crafting source correction, client acceptance deferred (2026-09-23)
- CP-023 hopper reservation source correction, client acceptance deferred (2026-09-23)
- CP-019 chest opening fence correction, client acceptance deferred (2026-09-23)
- CP-020 combat/death source audit, client acceptance deferred (2026-09-23)
- CP-021 crop support and trample source correction, client acceptance deferred (2026-09-23)
- CP-022 fluid/falling/light source audit, client acceptance deferred (2026-09-23)
- CP-023 machine transfer follow-up, client acceptance deferred (2026-09-23)
- CP-024 terrain/village source audit, client acceptance deferred (2026-09-23)
- CP-025 moving view source audit, client acceptance deferred (2026-09-23)
- CP-026 normal persistence source audit, client acceptance deferred (2026-09-23)
- CP-027 live session and operator authority source correction, client acceptance deferred (2026-09-23)
- CP-028 two-player contention source audit, client acceptance deferred (2026-09-23)
- CP-029 scoped survival acceptance deferred by owner (2026-09-23)

---

## CP-014 R1 code path — headless canonical village chain passed (2026-09-23)

**Status.** `draft`. The CP-014 headless R1 path is proven; CP-014 remains
open because graphical R0/R1 acceptance is `deferred_by_owner`. This is not
`stabilization`, `release-ready`, or evidence of two real clients.

**Current path.** The shipped `solaris-settlements` Rust component carries the
selected `site_id` through WIT/SDK/host/core. A generated, materialized vanilla
village supplies an actual resident home and a bound physical village chest.
Generated-home reservations use stable POI identity and reject an occupied
home. The canonical TCP scenario creates/adopts/populates the village, equips
its resident, harvests a mature carrot crop, deposits food, withdraws one
physical `oak_wood`, crafts four planks at a physical crafting table, and
deposits them. It checks the real chest *before* funding the unchanged
revision-1 `watch_post` blueprint with legal item stacks; it then commits one
structure portion and observes a changed world block. After world/host/server
restart it verifies saved food and the committed block, reads the exact core
structure status, rebinds the existing warehouse, and assigns fresh native
work to the same resident. No virtual material grant or mock host is used.

**Evidence and limits.**
- `settlement-cargo` including component-byte/source equality, real TCP/Wasm
  scenario, native inventory/build/work regressions:
  `.analysis/validation/20260923T003934-settlement-cargo-yom12edb/result.json`
  — `passed`.
- L2 stages on final source: `fmt` passed in
  `.analysis/validation/20260923T004141-correctness-srnrky0z/result.json`;
  an unused test variable caused Clippy failure, corrected without production
  change. Reran only failed Clippy:
  `.analysis/validation/20260923T004203-clippy-cwvjlss4/result.json` —
  `passed`; remaining `code-health` and workspace/all-target tests:
  `.analysis/validation/20260923T004210-code-health-q7h76lt1/result.json`
  and `.analysis/validation/20260923T004216-test-98cyl3o_/result.json`
  — both `passed`. The earlier complete `correctness` receipt
  `.analysis/validation/20260923T000523-correctness-egd2omz8/result.json`
  passed before the final stronger restart assertion.
- Independent read-only `CP014R1Review` found that the original restarted
  `settlement info` asserted only guest registry membership. The final scenario
  now also checks authoritative structure progress, persisted changed block,
  and a fresh core worker assignment; no second review was run after the fix.
- Shipped sibling component SHA-256:
  `e51d45440d543f9265163d36054130b65357afa3c6e774743d164d6b4c59ecba`.
  Real graphical client R0/R1, two observers, and graphical rejoin:
  `deferred_by_owner`, **not run**. Vanilla oracle/performance not run for
  this scenario. The headless client is a direct Minecraft TCP client, not a
  graphical real-client acceptance.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
tracked_source_diff_sha256_excluding_memory: 97dbfe7d4b109fdf4128e93b80b8338a148f5c921d151dc867e93e0c4a9d63db
untracked_canonical_test_sha256: 6949f8a16bdc06a622ed54b54d2c6462691e3cc2fe753cb6fb265aeb6468d15f
changed_files:
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - crates/mc-plugin-host/src/domain_settlements.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-script/src/resident_operations.rs
  - crates/mc-script/src/resident_operations_tests.rs
  - crates/mc-script/src/settlement_operations.rs
  - crates/mc-script/src/settlement_operations_tests.rs
  - crates/mc-script/wit/settlements.wit
  - crates/mc-test-harness/tests/settlement_wasm_readiness.rs
  - sdk/rust/examples/hello/src/settlement_ops.rs
  - sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs
  - tools/harness/profiles.py
  - ../solaris-default-plugins/sources/solaris-settlements/src/lib.rs
  - ../solaris-default-plugins/solaris-settlements/plugin.wasm
  - ../solaris-default-plugins/SETTLEMENT_OVERHAUL_CONTRACT.md
validation:
  - settlement-cargo: 20260923T003934-settlement-cargo-yom12edb passed
  - fmt: 20260923T004141-correctness-srnrky0z passed
  - clippy: 20260923T004203-clippy-cwvjlss4 passed
  - code-health: 20260923T004210-code-health-q7h76lt1 passed
  - test: 20260923T004216-test-98cyl3o_ passed
  - graphical-R0-R1: deferred_by_owner
next: Owner deferred graphical/final acceptance until after CP-096; continue source implementation in checkpoint order while retaining unverified gates.
```

## Deferred client acceptance, CP-015 source-only placement correction (2026-09-23)

The owner explicitly requested that the blocked implementation queue be
unblocked, graphical validation skipped, and overall acceptance performed only
after CP-096. All CP-014…096 TODOs are unblocked; CP-014 and CP-015 remain
pending **acceptance**, not passed. `draft` remains the product label.

In the Loader Java agent, `findUnobstructedPlaceablePair` no longer requires a
clicked dirt-like block: a full-collision, non-menu solid support can now be
chosen for an unobstructed crafting-table/stonecutter placement. The other
placement selectors retain their dirt-support constraint, including the
two-client dirt-drop scenario. The crafting-table blockage diagnostic now
describes unobstructed placement, not dry terrain. This is a code-path
correction, **not** a reproduction or real-client resolution of the historical
no-debug survival failure; the exact failure still needs the deferred graphical
scenario. No terrain/worldgen change was made.

Focused sibling `:java-agent:compileJava` passed twice and
`:java-agent:test --tests dev.solaris.agent.javaagent.PlayableRealClientLoopScenarioTest`
passed after the final correction. Independent read-only review found that
interactive supports could consume the place action; the final selector
excludes menu supports and non-full collision blocks. No second review was run.
Graphical survival and multi-client acceptance: `deferred_by_owner`, not run.
Sibling base `0972926e4431ee597cb4903abc43e96872acd732`; scoped two-file
working diff SHA-256 `fd2d18937193e914319d88e7742c040462548d25e06927d0d1e3412039144fcb`
includes pre-existing unrelated edits in `MinecraftScenarioClient.java`;
preserve them.

## CP-018 inventory/crafting source correction, client acceptance deferred (2026-09-23)

Window-0 and crafting-table clicks previously consumed overflow crafting
remainders and only logged that they were "dropping" them. The owner commit now
receives those remainder stacks as world-drop plans; if item entities or the
bounded drop budget are unavailable, the candidate is rejected without
consuming ingredients. Compatible drops are packed within item stack limits
before the existing 14-entity owner admission fence. The recipe-book
direct-craft path now generates the same bucket remainders rather than silently
consuming them. No new container authority or session mirror was added.

The packet-level Window-0 regression exercised 16 shift-click crafts with
full inventory: the player retained the final empty bucket and the owner
persisted a single 15-bucket world drop. The recipe-book unit regression
exercised a full inventory and observed its overflow bucket. Both focused
`cargo test -p mc-net ... --lib` commands passed; `cargo fmt --all -- --check`
passed and `code-health` passed at
`.analysis/validation/20260923T032201-code-health-3id2vmbj/result.json`.
Independent read-only review found the pre-packing 14-drop admission error;
packing and bounded rejection address it. No second review was run.
Pre-fix source evidence: `.analysis/codex-logs/crafting-remainder-overflow/repro.txt`.
The graphical `inventory` profile and complete CP-018 acceptance remain
`deferred_by_owner` until after CP-096; the product label remains `draft`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 4bdcf1bb8f6236e4be2595bfc1fb8670fb60544cb1e26814a69ea29a49ba8ecc
changed_files:
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/recipes.rs
  - crates/mc-net/src/play/inventory.rs
  - crates/mc-net/src/play/containers.rs
  - crates/mc-net/src/play/containers/crafting.rs
  - crates/mc-net/src/play/command_execution.rs
  - crates/mc-net/src/play/session/creative_slot.rs
  - crates/mc-net/src/play/tests/inventory_settlement.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net full_inventory_crafting_preserves_overflow_bucket_as_world_drop --lib: passed
  - cargo test -p mc-net recipe_book_craft_preserves_bucket_remainder_when_inventory_is_full --lib: passed
  - cargo fmt --all -- --check: passed
  - code-health: 20260923T032201-code-health-3id2vmbj passed
  - graphical-inventory: deferred_by_owner
next: Continue non-graphical CP-018 source audit, then CP-019 container observer audit; defer acceptance to the owner after CP-096.
```


## CP-023 hopper reservation source correction, client acceptance deferred (2026-09-23)

A hopper could pull reserved warehouse stock from a chest: its staged source
after-image was never checked against the CP-004 reservation floor. A focused
regression failed before the fix with two reserved items being moved, then
passed with the floor preserved and only unreserved surplus movable.
`scheduled_hopper_transfer` now checks the source after-image. The resident
owner commit rechecks the floor while holding the existing per-region
reservation admission gate; it includes both physical chest halves and the
canonical viewer position. An occupied gate leaves the resident tick unclaimed.
The coordinator fallback, which writes storage directly, also holds admission
while transferring; a busy gate produces no transfer and schedules a later tick.
No second planner or inventory authority was added.

The 16 focused `scheduled_hopper_` tests passed, including durable journal,
double-chest transfers, a held reservation admission gate and a direct
fallback-path regression that failed before the fallback guard. `fmt` passed
at `.analysis/validation/20260923T042617-fmt-jiliool5/result.json`;
`code-health` passed at
`.analysis/validation/20260923T042609-code-health-chcx4x1x/result.json`.
Independent read-only review found both the initial same-region double-lock
error and the fallback bypass; both were corrected and the failed hopper suite
was rerun after the relevant fixes. The graphical machine/container scenario,
bounded tick workload, unload/restart and complete CP-023 acceptance remain
`deferred_by_owner` until after CP-096; product maturity remains `draft`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 5d3837f90d2cc2683ca152a9c2388504d8aecb16bacdf76983c1ee6a0543df31
changed_files:
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/scheduled_blocks.rs
  - crates/mc-net/src/play/tests/scheduled_hoppers.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net scheduled_hopper_ --lib: 16 passed
  - fmt: 20260923T042617-fmt-jiliool5 passed
  - code-health: 20260923T042609-code-health-chcx4x1x passed
  - graphical-machines: deferred_by_owner
next: Reproduce CP-019 stale chest window after same-position block replacement before changing container authority.
```

## CP-019 chest opening fence correction, client acceptance deferred (2026-09-23)

An old chest menu could spend items from a new block at the same position when
the replacement had identical slots and a matching shared menu state id. The
pre-fix regional test failed as a committed stale click; exact reproduction is
in `.analysis/codex-logs/chest-window-aba/repro.txt`. The open menu now retains
each physical half's existing `BlockMutationToken`. Both resident CAS and the
coordinator fallback compare these tokens with contents under their respective
world locks. Replacing a half or unloading/reloading a chunk rejects the old
menu. The connection closes that menu, unregisters chest viewers, and sends
window-0 inventory/cursor resynchronization. The server-owned warehouse
receipt has no menu opening token and keeps its existing transaction path.

Regional and fallback ABA tests passed (2); chest scope passed (39), including
the packet-level close and inventory resync; mc-world chest tests passed (9).
Production `cargo check -p mc-net` passed. `fmt` passed at
`.analysis/validation/20260923T051613-fmt-wmlnlpe4/result.json`;
`code-health` passed at
`.analysis/validation/20260923T051617-code-health-alg7ti__/result.json`;
`git diff --check` and `codegraph sync .` passed. Independent read-only
`ChestABAReview` found the missing inventory resync and a resident test using
the coordinator fallback; both were fixed and the affected checks rerun.
Graphical two-viewer/double-chest/reconnect and save/reopen acceptance is
`deferred_by_owner` until after CP-096, not passed; maturity remains `draft`.
No commit was authorized.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 60f91239f6d07224ff7a69ef413816a8ab409f722653f5b8119e9cd493af9585
changed_files:
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/containers/chest.rs
  - crates/mc-net/src/play/session/transactions.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/play/simulation/regional_mutation.rs
  - crates/mc-net/src/play/simulation/tests.rs
  - crates/mc-net/src/play/tests/chest.rs
  - crates/mc-world/src/resident.rs
  - crates/mc-world/src/resident/inventory_tests.rs
  - crates/mc-world/src/storage/tests/resident_mutation_tests.rs
  - docs/decisions/0004-staged-single-writer-simulation.md
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net chest --lib: 39 passed
  - cargo test -p mc-world chest --lib: 9 passed
  - cargo check -p mc-net: passed
  - fmt: 20260923T051613-fmt-wmlnlpe4 passed
  - code-health: 20260923T051617-code-health-alg7ti__ passed
  - graphical-two-viewer: deferred_by_owner
next: Audit CP-020 lethal damage, drops, XP and respawn authority without graphical acceptance.
```

## CP-020 combat/death source audit, client acceptance deferred (2026-09-23)

Existing owner paths keep attack/target inventory and health fences together;
lethal survival commit clears ordinary inventory/cursor and XP while spawning
the corresponding item and XP entities, then publishes death to observers.
The connection applies the committed damage publication without using socket
success as the commit gate. This is a bounded source audit, not an assertion
that the entire CP-020 scenario has passed.

Focused tests on this tree: `cargo test -p mc-net player_combat --lib` (24
passed), `cargo test -p mc-net respawn --lib` (11 passed),
`cargo test -p mc-net player_damage --lib` (16 passed), and
`cargo test -p mc-net player_death_inventory_xp_policy_is_atomic_and_idempotent
--lib` (1 passed). These include Keep/Cancel/Replace and lethal drop policy;
they do not substitute for two real attackers, disconnected victim, full
pickup inventory, death/rejoin, or real-client respawn. Those scenarios remain
`deferred_by_owner` until after CP-096. No CP-020 source edit or commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
changed_files:
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net player_combat --lib: 24 passed
  - cargo test -p mc-net respawn --lib: 11 passed
  - cargo test -p mc-net player_damage --lib: 16 passed
  - cargo test -p mc-net player_death_inventory_xp_policy_is_atomic_and_idempotent --lib: 1 passed
  - graphical-combat-death: deferred_by_owner
next: Audit CP-021 crop growth, animal feeding and saved-state reuse without graphical acceptance.
```

## CP-021 crop support and trample source correction, client acceptance deferred (2026-09-23)

Breaking farmland previously left wheat suspended: the shared support-cascade
classifier omitted ordinary crops. Falling onto farmland likewise replaced
only the soil, leaving a mature crop over dirt and no physical item. Both
pre-fix failures and their commands are recorded in
`.analysis/codex-logs/crop-support/repro.txt`.

The shared plant support rule now includes ordinary crops and stems for break
cascades and water washing. Farmland trampling snapshots both block tokens:
nonempty crop loot commits soil, crop, and physical drops together through the
existing regional block-drop owner; a legitimate zero-seed stem roll uses the
existing conditional two-block edit path. Seeded melon/pumpkin stem fallback
uses the local 26.1.2 loot-table rule of three age-weighted seed trials; it
replaces the old unsupported generic stem-block item fallback. The
independent read-only `CropSupportReview` identified the zero-yield stem
regression; it was fixed and the affected test rerun.

On this tree, `cargo test -p mc-net plants --lib` passed 84, `random_tick`
passed 37, `breeding` passed 11, `animal_feed` passed 1, and the animal
age/save-barrier test passed 1. `cargo test -p mc-world plant --lib` passed 7.
`fmt` passed at
`.analysis/validation/20260923T062536-fmt-_8hscj7v/result.json`,
`code-health` passed at
`.analysis/validation/20260923T062542-code-health-czbvbr4n/result.json`;
`git diff --check` and `codegraph sync .` passed. No real farm/animal
graphical scenario, observer client, unload/rejoin lifecycle, or NPC job
end-to-end gate was run; these remain `deferred_by_owner` until after CP-096.
Maturity stays `draft`; no commit was authorized.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: e601eecea69c005cde06f18bde9ee3c94a9c63c4aef9985fb44309049e24cd62
changed_files:
  - crates/mc-world/src/plant_rules_26_1_2.rs
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/survival.rs
  - crates/mc-net/src/play/tests/plants.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net plants --lib: 84 passed
  - cargo test -p mc-net random_tick --lib: 37 passed
  - cargo test -p mc-net breeding --lib: 11 passed
  - cargo test -p mc-net animal_feed --lib: 1 passed
  - cargo test -p mc-net animal_age_countdown_waits_for_entity_save_barrier --lib: 1 passed
  - cargo test -p mc-world plant --lib: 7 passed
  - fmt: 20260923T062536-fmt-_8hscj7v passed
  - code-health: 20260923T062542-code-health-czbvbr4n passed
  - graphical-farm-animals: deferred_by_owner
next: Audit CP-022 bucket/fluid/falling-block/light source behavior without graphical acceptance.
```

## CP-022 fluid/falling/light source audit, client acceptance deferred (2026-09-23)

The current conditional bucket owner commits block and held-item effects
together; scheduled fluid ticks use the shared simulation tick, water/lava
mixing is planned from loaded snapshots, falling-block start/landing retain
block preconditions, and block edits route light invalidation through the
resident mutation result. No CP-022 code change was warranted by the focused
source audit. This does not claim the full bucket/fall/light scenario passed.

On the unchanged source tree, `cargo test -p mc-net fluid_runtime --lib`
passed 18, `falling_blocks` passed 7, `bucket` passed 24, and
`cargo test -p mc-world light --lib` passed 42 with 2 ignored. These
do not cover a real two-client chunk-edge fall, save/restart mid-flight,
or the observed lighting topology against 26.1.2 vanilla. Graphical
acceptance remains `deferred_by_owner` until after CP-096; maturity `draft`.
No commit was authorized.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: e601eecea69c005cde06f18bde9ee3c94a9c63c4aef9985fb44309049e24cd62
changed_files:
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net fluid_runtime --lib: 18 passed
  - cargo test -p mc-net falling_blocks --lib: 7 passed
  - cargo test -p mc-net bucket --lib: 24 passed
  - cargo test -p mc-world light --lib: 42 passed, 2 ignored
  - graphical-fluid-fall-light: deferred_by_owner
next: Audit CP-023 furnace/hopper item conservation and observer paths without graphical acceptance.
```

## CP-023 machine transfer follow-up, client acceptance deferred (2026-09-23)

The earlier CP-023 reservation correction at this cursor remains the source
change: hopper withdrawal checks the physical warehouse reservation floor
under the existing region admission gate, including coordinator fallback.
This pass found no further source discrepancy in the normal furnace/hopper
chain. `cargo test -p mc-net scheduled_hopper_ --lib` passed 16 on the
current tree, including hopper→furnace input/fuel, furnace output→chest,
double-chest transfer, held reservation gate, cross-region atomic commit,
and failure preservation. `cargo test -p mc-net furnace --lib` passed 35.
The earlier fmt/code-health receipts remain applicable to the unchanged
Rust source since CP-021. This is a source/focused-test result, not a
bounded tick measurement or graphical proof; concurrent player clicks,
unload/restart, observers and actual machine-chain capacity are
`deferred_by_owner` until after CP-096. Maturity remains `draft`; no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: e601eecea69c005cde06f18bde9ee3c94a9c63c4aef9985fb44309049e24cd62
changed_files:
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net scheduled_hopper_ --lib: 16 passed
  - cargo test -p mc-net furnace --lib: 35 passed
  - graphical-machine-chain: deferred_by_owner
next: Audit CP-024 terrain generation and vanilla village site behavior without graphical acceptance.
```

## CP-024 terrain/village source audit, client acceptance deferred (2026-09-23)

Generation and adoption were checked separately. `cargo test -p mc-worldgen
village --lib` passed 77 with 2 ignored old prototype tests. A live-cache
village test actually generated 13 entity-bearing pieces and placed 8
villagers (seed 4242); a desert village assembled 87 pieces at chunk
`(0,111)`. The multi-seed Tellus biome/feature fingerprint test passed
one corpus run. The separate chunk inhabitant-marker round-trip and
settlement claim/villager-job entity-storage round-trip each passed one.
No terrain rewrite or CP-024 source change was warranted by these checks.

These receipts do not prove a generated village survives a full fresh
generate→unload→reopen→plugin-adoption scenario, every ACC-02 village
family, accessible entrances/resources, unknown saved contents, or owner
terrain ACCEPT. All remain pending; graphical/owner acceptance is
`deferred_by_owner` until after CP-096. Maturity stays `draft`; no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: e601eecea69c005cde06f18bde9ee3c94a9c63c4aef9985fb44309049e24cd62
changed_files:
  - docs/MEMORY.md
validation:
  - cargo test -p mc-worldgen village --lib: 77 passed, 2 ignored
  - cargo test -p mc-worldgen live_village_populates_its_pieces_villagers --lib -- --nocapture: 1 passed, 8 villagers
  - cargo test -p mc-worldgen desert_village_assembles_deterministically --lib -- --nocapture: 1 passed, 87 pieces
  - cargo test -p mc-worldgen tellus_multi_seed_biome_and_feature_fingerprints_are_distinct_and_bounded --lib: 1 passed
  - cargo test -p mc-world settlement_inhabitant_markers_round_trip_through_chunk_extras --lib: 1 passed
  - cargo test -p mc-net settlement_claim_and_villager_job_round_trip_through_entity_storage --lib: 1 passed
  - graphical-world-village: deferred_by_owner
next: Audit CP-025 chunk/entity visibility during movement and generation fences without graphical acceptance.
```

## CP-025 moving view source audit, client acceptance deferred (2026-09-23)

The chunk stream rejects obsolete generation results, releases their
prepared claims, checks prepared revisions before and after socket writes,
and requeues a result invalidated during writing. Visibility is marked
loaded only after the chunk frame is written; entity spawn commands follow
that transition. `cargo test -p mc-net chunk_stream --lib` passed 80;
focused spawn-before-movement and one-chunk crossing tests each passed one.
No CP-025 source change was warranted by these focused checks.

They do not prove a complete view window/light/entity picture while two
real clients cross regions at different read rates or respawn far away.
Owner-deferred graphical acceptance remains `deferred_by_owner` until
after CP-096; maturity `draft`, no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: e601eecea69c005cde06f18bde9ee3c94a9c63c4aef9985fb44309049e24cd62
changed_files:
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net chunk_stream --lib: 80 passed
  - cargo test -p mc-net planned_spawn_orders_concurrent_entity_movement_before_channel_delivery --lib: 1 passed
  - cargo test -p mc-net one_chunk_crossing_does_not_suppress_other_entity_movement --lib: 1 passed
  - graphical-two-client-movement: deferred_by_owner
next: Audit CP-026 normal save/reopen and unsupported-data behavior without graphical acceptance.
```

## CP-026 normal persistence source audit, client acceptance deferred (2026-09-23)

`cargo test -p mc-world dirty_flush --lib` passed 44 with 2 ignored;
its scoped reopen cases include edited chunks and unknown root extras.
`cargo test -p mc-world anvil --lib` passed 52 with 8 ignored.
`cargo test -p mc-net persistence --lib` passed 93 with 1 ignored,
and `world_journal` passed 31. These exercise internal Anvil/NBT,
player/component and entity storage, and journal replay on this tree.
An embedded server save/stop/rejoin restored inventory and edited block;
the wire sign flush/reopen and natural entity identity restart each passed.
The in-flight campfire restart initially failed on a held journal lease:
the test kept its `SaveHandle` (and its `ServerConfig` world) alive after
the first serve task joined. Dropping that handle before binding the
second server fixed the exact repro; receipt
`.analysis/codex-logs/cp026-campfire-restart/repro.txt`. The same test
then passed and observed cooking resume after reopen.
An independent read-only review of the handle-release correction returned
`pass` with no findings; it ran no validation.

The opt-in real vanilla-world chunk fixture
`.analysis/test-world/region/r.0.0.mca` is absent (the 26.1.2 blocks
report is present); ignored real-world round-trips were not run.
These separate runtime scenarios are not a joined server save/stop/start
with a chest, sign, furnace, component item and entity all in one world.
The graphical rejoin remains `deferred_by_owner` until after CP-096.
Unsupported-data coverage is limited to the named focused cases, not
generic vanilla-world compatibility. Maturity `draft`; no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 4874a7668c9cd112b55afeea5a583503a7da0952b727b60f30e1a2fea460ac03
changed_files:
  - crates/mc-test-harness/tests/block_edit/campfire.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-world dirty_flush --lib: 44 passed, 2 ignored
  - cargo test -p mc-world anvil --lib: 52 passed, 8 ignored
  - cargo test -p mc-net persistence --lib: 93 passed, 1 ignored
  - cargo test -p mc-net world_journal --lib: 31 passed
  - embedded_save_restart_rejoin_preserves_inventory_and_edited_block: 1 passed
  - survival_sign_text_survives_flush_and_reopen --ignored: 1 passed
  - natural_entities_keep_identity_across_save_restart_rejoin: 1 passed
  - survival_campfire_in_flight_state_resumes_after_reopen --ignored: failed pre-fix (journal lease), 1 passed post-fix
  - real-vanilla-chunk-fixture: absent
  - graphical-save-rejoin: deferred_by_owner
next: Audit CP-027 login/relogin/session authority without graphical acceptance.
```

## CP-027 live session and operator authority source correction, client acceptance deferred (2026-09-23)

Login/session source review found that duplicate UUID or case-insensitive
name admission is rejected until unregister. Online mode uses encrypted
challenge plus verified Mojang profile; offline UUID is deterministic but
is **not** authentication. Public hosting rejects the loopback developer
operator fallback. The old `live_permissions_for` retained a login-time
operator bit whenever the live operator set became empty, so revoking the
last explicitly listed operator left a connected remote player's admin
permissions intact. A red regression and exact command are recorded in
`.analysis/codex-logs/cp027-revoked-operator/repro.txt`. Live commands now
resolve against the verified profile, actual socket peer, and shared operator
set. Configuration-to-Play recomputes the grant after the handshake.

Plugin player snapshots also copied the old operator bit at registration;
the pre-fix red reproduction is in
`.analysis/codex-logs/cp027-revoked-operator/plugin-repro.txt`.
Play sessions now retain the peer and a clone of the shared
`CommandPermissionConfig`; script queries, committed-event snapshots, zone
observations, gameplay event contexts and zone build checks all resolve
against its current operator set. Container script context and the
`ServerboundChangeGameMode` ingress also recheck the current role rather
than their initial grant. The frequently called zone, container and
gameplay paths normalize the verified name once and reuse the canonical
UUID without per-packet lowercase allocations. Console mutations do not
rewrite a second role cache. The revised query and committed-block-event
regressions pass after revocation. An exact uncompressed
`ClientboundCommands` frame is decoded after role revocation and its root
has no operator children. ADR 0006 records this authority boundary.
The Play ingress size tripwire rose from 731 to 742 for actual
peer-threading and live-role routing lines; `code-health` passes.

Focused evidence: `mc-net login` 13, `session_auth` 14,
`admission` 21, `stale_session` 15, `script_player_query_endpoint` 4,
`script_gameplay_events` 14, `script_client_view` 4, committed death 1,
duplicate-session rejection 1, role-revoke command-tree wire 1,
revoked-operator focused 2; `mc-script revoked` 1;
`mc-server --test login` 12. Canonical fmt passed at
`.analysis/validation/20260923T073338-fmt-d1ma5auh/result.json`;
code-health passed at
`.analysis/validation/20260923T073400-code-health-p11pq_4x/result.json`.
The independent read-only reviewer returned `pass` for the original
session/command delta but found retained login-time authority in the
zone/gameplay and player-control paths; those findings were corrected
and the focused gates rerun. No second review was run.
Full real-client expired/replaced-session and stale-callback scenarios
remain `deferred_by_owner` until after CP-096. Focused tests do not
establish them; maturity `draft`, no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 9a39022198dcd78b8bd041c166b61d8a1bbb38182d83d55ebc29ce0a98375b11
changed_files:
  - crates/mc-net/src/server.rs
  - crates/mc-net/src/connection_driver.rs
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/ingress/chat_commands.rs
  - crates/mc-net/src/play/session.rs
  - crates/mc-net/src/play/session/session_lifecycle.rs
  - crates/mc-net/src/play/session/script_commit_events.rs
  - crates/mc-net/src/play/session/script_player_query_endpoint.rs
  - crates/mc-net/src/play/session/script_player_query_endpoint_tests.rs
  - crates/mc-net/src/play/tests/admin_commands.rs
  - crates/mc-net/src/play/tests/tab_list.rs
  - crates/mc-net/src/play/tests/outbound_delivery.rs
  - crates/mc-net/src/play/tests/direct_response_write_stall.rs
  - crates/mc-net/src/play/tests/outbound_channel_close.rs
  - crates/mc-net/src/play/tests/outbound_pressure_draining.rs
  - crates/mc-net/src/play/tests/outbound_write_stall.rs
  - crates/mc-net/src/server/tests/admission.rs
  - crates/mc-net/src/server/operator_control.rs
  - crates/mc-net/src/play/session/script_client_sound_endpoint_tests.rs
  - crates/mc-net/src/play/session/script_loader_item_endpoint_tests.rs
  - crates/mc-net/src/play/session/tests.rs
  - crates/mc-net/src/play/use_item_on_adapter_tests.rs
  - crates/mc-net/src/play/script_gameplay_events.rs
  - crates/mc-net/src/play/script_gameplay_events_tests.rs
  - crates/mc-net/tests/enchanting_recipe_settlement.rs
  - crates/mc-net/tests/inventory_settlement.rs
  - crates/xtask/src/main.rs
  - docs/decisions/0006-mc-net-module-boundaries.md
  - docs/MEMORY.md
validation:
  - revoked operator pre-fix red, post-fix admission 21 passed
  - plugin player context pre-fix red, post-fix query 4 passed
  - revoked operator command tree wire: 1 passed
  - committed block event rechecks revoked operator context: 1 passed
  - mc-net script_gameplay_events: 14 passed
  - mc-net revoked_operator: 2 passed
  - mc-server login: 12 passed
  - fmt: 20260923T073338-fmt-d1ma5auh passed
  - code-health: 20260923T073400-code-health-p11pq_4x passed
  - graphical-session-authority: deferred_by_owner
next: Audit CP-028 concurrent two-player actions without claiming graphical acceptance.
```

## CP-028 two-player contention source audit, client acceptance deferred (2026-09-23)

The sidecar-backed deterministic multiplayer transaction replay passed twice
with seed 8102 and checked eight same-target placement races plus a shared
chest pickup with a conserved two-item total. The two-subscriber stale
survival break test passed against peer replacement. Focused owner tests
passed for one claimant/exact item stack, pickup credit after requester
delivery loss, stale chest click resync, and stale crafting grid rebuild.
No CP-028 source correction followed these checks.

These tests are supplemental, not two real graphical clients racing on the
same block/drop/menu. They do not cover every last-stack, concurrent worker
move, or disconnected winning-client acknowledgement in the exact plan
scenario. Server decision plus both-client evidence is
`deferred_by_owner` until after CP-096. Maturity `draft`; no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 9a39022198dcd78b8bd041c166b61d8a1bbb38182d83d55ebc29ce0a98375b11
changed_files:
  - docs/MEMORY.md
validation:
  - checked_multiplayer_transaction_replay_is_deterministic_and_conservative --ignored: 1 passed
  - stale_survival_break_cannot_break_peer_replacement --ignored: 1 passed
  - concurrent_item_pickup_has_one_claimant_and_exact_stack: 1 passed
  - item_pickup_credit_survives_requester_loss_after_owner_apply: 1 passed
  - stale_chest_click_after_peer_mutation_resyncs_without_mutating_storage: 1 passed
  - stale_crafting_click_rebuilds_grid_from_owner_projection: 1 passed
  - graphical-two-client-contention: deferred_by_owner
next: Record CP-029 acceptance deferral, then verify CP-030 WIT consumer route.
```

## CP-029 scoped survival acceptance deferred by owner (2026-09-23)

This VERIFY checkpoint requires the full 20-minute no-debug vanilla-survival
loop, save/rejoin, a second real player, varied seeds and a bounded
adversarial pass. Owner deferred all graphical acceptance until after
CP-096. No `playable` or `regression` graphical profile was run for this
checkpoint; source-focused checks from CP-015–028 do not replace client or
vanilla evidence, close an existing owner failure or establish a percentage
of accepted gameplay. Coverage remains `draft` and CP-029 remains blocked.
No new source correction, readiness ledger or commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 9a39022198dcd78b8bd041c166b61d8a1bbb38182d83d55ebc29ce0a98375b11
changed_files:
  - docs/MEMORY.md
validation:
  - full-20-minute-graphical-survival: deferred_by_owner
  - two-client-varied-seed-acceptance: deferred_by_owner
  - adversarial-graphical-pass: deferred_by_owner
next: Verify CP-030 WIT consumer gameplay kernel while retaining CP-029 acceptance block.
```

