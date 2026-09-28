# Archived checkpoint log — part 14 of 17

Era: 09-20. Chronological checkpoint history moved out of `docs/MEMORY.md` so
the live cursor stays small. **Not startup context** (see `AGENTS.md`); read it when
a question is about this era, not to learn the current state.

## Sections in this part

- v0.0.8 release candidate (2026-09-20)

---

## v0.0.8 release candidate (2026-09-20)

**Versioning.** `v0.0.7` is already published at `9e5e20e4`; the next release
therefore targets `v0.0.8`, never retags the published version.

**Outcome.** The harness now reports live stage start/exit records; native
Linux links use mold without leaking flags into nested Wasm guest builds;
default Rust components are deployed by the sibling package repository; guest
fixture builds serialize only their shared Cargo artifact writes. The final
repair restores outbound player survival updates and effect packet dispatch.

**Evidence.**
- `python3 -m tools.harness run correctness`:
  `.analysis/validation/20260920T030823-correctness-sbbjo0y4/result.json`
  — passed.
- `cargo test -p mc-net --lib` — 2301 passed, 8 ignored.
- TCP regressions: shulker effect lifecycle and village golem spawn/attack —
  passed inside the final correctness receipt.
- Independent read-only review `CoreCheckpointReview`: two P1 findings
  (survival-effect dispatch and Wasm-inherited mold flag) fixed before final
  L2; no remaining static blocker in core or the five converted default
  component packages.

**Publication blocker.** The core worktree contains 104 modified or untracked
paths, including owner files outside this release slice. The converted sibling
default-plugin packages are also uncommitted while CI remains pinned to their
earlier revision `2d51ae5559cdd5b7cbab32888ef2b11d931e3e6e`. No commit, tag, push,
or publication is safe until the exact core release set and the sibling
publication decision are authorized.

**Release scope and maturity.** This is a draft release. No fresh graphical
client, oracle, performance, or manual owner gate was run. The converted
packages (`solaris-permissions`, `solaris-essentials`, `solaris-economy`,
`solaris-towns`, `solaris-audit`) were statically verified by the independent
review; their Wasm binaries were not independently executed in this close.

**CP-014 boundary correction and source-owned component migration.**
`solaris-default-plugins` owns the first-party `solaris-settlements` product.
The core server and SDK contain only generic SDK/fixture code: they neither
build, package, test, nor import a first-party settlement component. Generic
durable world, inventory, resident-work, structure, and receipt boundaries
remain available to every plugin without product dependency.

The previous component source was recovered from the saved session unified diff
and verified against its original Git blob
`3b152101304c1a1f052bd6848ea13b0141e913e2`; it now lives in
`../solaris-default-plugins/sources/solaris-settlements/`. The deployed sibling
package now has API 0.7 `entry = "plugin.wasm"` and only the capabilities its
component uses. `main.lua`, legacy config, and client bundle are removed from
the deployable directory; the legacy Luau source is retained only under
`sources/solaris-settlements/legacy/` as porting input. Authoring blueprints
remain under `structures/`.

Focused core extraction checks passed: `cargo fmt --all -- --check`;
`cargo test -p mc-net resident_settlement` (17);
`cargo test -p mc-net settlement_tests` (53); the generated-inhabitant
authority test (1); and
`.analysis/validation/20260920T081931-code-health-_j4x8m7h/result.json`
(`passed`). The sibling source now passes its 8 component behavior tests and a
`wasm32-unknown-unknown --release` build. The generated package passed
`cargo run -p mc-server -- --check --config
/tmp/solaris-settlement-component-check/server.toml`, which strictly loaded
only `solaris-settlements` with its declared grants.

The sibling component now persists owner-scoped settlement creation, roles,
condition, supported site adoption, generic-core survey tokens/tags, a bounded
index of named project buildings, selected specialties, and named residents.
`/settlement project` re-reads its exact site immediately before preparing a
structure, so its revision fence is current rather than the adoption-era value;
a committed plan is stored under a generated building name such as
`warehouse_1`. `/settlement fund <name> <building>` and `advance <name>
<building>` authorize the owner or steward, select only that durable building,
then re-read its exact core structure before material reservation or a bounded
core work portion. `/settlement specialize <name> ...` allows at most two legacy
specialties only when each matching survey tag exists and its named workplace's
current core state is `Committed`; it persists only after those exact status
checks. `/settlement populate <name>` permits a member to exact-query an
adopted core-authored `site_*`, reserve its first free home, and spawn its
core-managed resident. A committed spawn is then persisted behind the same
registry CAS fence under a generated alias such as `resident_1`; `info` exposes
the durable aliases. If that first post-spawn CAS refuses or fails, the component
re-reads the registry once, reapplies the same opaque handle and revision, and
re-CASes it without creating another resident. Its command-level and state tests
total 39; `cargo fmt --manifest-path
../solaris-default-plugins/sources/solaris-settlements/Cargo.toml -- --check`,
the sibling `cargo test`, a `wasm32-unknown-unknown --release` build, component
encoding, and the strict server package check passed. Independent review found
that the first matching workplace could be uncommitted while a later matching
one was committed, and that a specialization status continuation could race a
registry update. The component now queries every matching saved workplace until
one is currently committed, and blocks concurrent registry reads during that
continuation.

The generic SDK now makes `log` a native no-op while retaining the WIT host
import on `wasm32`, so component answer-path tests can execute without a host
stub. `cargo test --manifest-path sdk/rust/solaris-plugin-sdk/Cargo.toml`
passed, alongside the 39 component tests and component package check above.

The independent command-port review caught three authority defects before this
close: named fund/build selected an unrelated plugin-global structure; generated
`village_*` sites could not reserve residents through the generic core authority;
and project reused an adoption-era site revision. The unsafe named
build/populate routes were removed rather than advertised; named funding and
advancement were reintroduced only after a durable project record and exact
core status read. A site-compatible named population route was reintroduced
only for core-authored `site_*`; generated `village_*` sites remain unsupported.
Adoption now rejects unsupported generated sites, and project queries its exact
site before the core call. The current product port is
`create → role → ruin/restore → site → adopt → survey → project → fund →
advance → specialize → populate`; population on generated villages, family,
job, claim, and abandonment remain unported.

Adoption now uses the generic targeted site query rather than a first-page list
scan; a storage CAS server failure is reported as unavailable, not as a version
conflict. The corrected component rebuild, encoding, and strict package check
also passed.

Independent review found stale core-owned settlement regression wiring and two
documentation boundary errors. The obsolete `m94-09` scenario, driver, harness
test, and product package installation path were removed from core; only generic
regression machinery remains. Its replacement validation passed:
`.analysis/validation/20260920T085538-harness-check-snl7oa0g/result.json`.

No real-client R0/R1 product scenario, two-observer flow, or full
food/material lifecycle has run. The component migration proves source
ownership, reproducible component packaging, command-flow unit behavior, and
strict admission—not campaign readiness. CP-014 remains blocked.
- CP-015, CP-016, CP-017, CP-018, CP-019, CP-020, CP-021, CP-022, CP-023,
  CP-024, CP-025, CP-026, CP-027, CP-028, CP-029 remain pending.
- CP-030, CP-031, CP-032, CP-033, CP-034, CP-035, CP-036, CP-037, CP-038,
  CP-039, CP-040, CP-041, CP-042, CP-043 remain pending.
- CP-044, CP-045, CP-046, CP-047, CP-048, CP-049, CP-050, CP-051, CP-052,
  CP-053, CP-054, CP-055, CP-056, CP-057 remain pending.
- CP-058, CP-059, CP-060, CP-061, CP-062, CP-063, CP-064, CP-065, CP-066,
  CP-067, CP-068, CP-069 remain conditional/pending.
- CP-070, CP-071, CP-072, CP-073, CP-074, CP-075, CP-076, CP-077, CP-078
  remain pending.
- CP-079, CP-080, CP-081, CP-082, CP-083, CP-084, CP-085, CP-086 remain
  pending.
- CP-087 remains a conditional handoff; CP-088, CP-089, CP-090, CP-091,
  CP-092, CP-093, CP-094, CP-095, CP-096 remain pending.
**P0 component-host revalidation (2026-09-21).** The real SDK-built component
passes the host safety matrix: raw core-module/ABI refusal before guest code,
pre-compilation artifact cap, fuel/epoch/memory/stack limits, bounded
guest-to-host lifting for oversized, cumulative, nested and now directly
aliased outputs, unpublished trap and canonical cleanup behavior, and
independent-instance isolation. The hostile alias fixture retargets a real
SDK-built component result after canonical lowering; five result records reuse
the first string pointer, have distinct declared lengths, and lift the modified
shared first byte. The strict one-package deployment baseline records Linux
x86_64 hardware/workload/RSS/latency/tick/queue/counter data. Receipt:
`.analysis/codex-logs/p0-revalidation-20260921/receipt.txt`; 14 focused tests,
formatter and code-health passed. Rust 1.94 resolves Wasmtime 36.0.15 and WIT
tooling 0.259. Native Linux AArch64 execution has not run from this workstation:
it lacks both `aarch64-linux-gnu-gcc` and a compatible sysroot. The configured
native Arm CI job is not claimed as execution evidence.
The current target check confirms the recorded blocker rather than merely assuming
it: `CARGO_BUILD_JOBS=2 cargo check --target aarch64-unknown-linux-gnu -p
mc-plugin-host` exits 101 in Wasmtime's build script, before Rust checking,
because `cc-rs` must compile `src/runtime/vm/helpers.c` and
`aarch64-linux-gnu-gcc` is absent. The installed Rust target alone is
insufficient; this is not a linker-only gap. Receipt:
`.analysis/codex-logs/p0-aarch64-target-check-20260921/receipt.txt`.

**P0 accepted — native Linux AArch64 (2026-09-21).** GitHub Actions
[run 35667641281](https://github.com/kaiserproger/solaris/actions/runs/35667641281),
job 106556828374, executed commit
`87af4502a79864392798bcb52634e6d2d68ce9d4` on `ubuntu-24.04-arm` with
Rust 1.94.1 and the explicit wasm32 guest target. All 16 tests in
`host_bounds`, `cleanup_isolation`, `component_roundtrip`,
`contract_refusal`, and `host_baseline` passed. The Arm baseline recorded four
logical CPUs, callback p50/p95/p99/max 15.575/16.215/30.071/47.374 us,
component compilation 19,313.064 ms, and VmHWM 5,772→140,000 KiB. This
closes P0's last target-compatibility gap; the earlier target-install failure
and its setup-only fix remain in the receipt. Receipt:
`.analysis/codex-logs/p0-aarch64-target-check-20260921/receipt.txt`.

**P8 accepted — single Wasmtime production runtime (2026-09-22).** Current
production source, Cargo graph, CI, SDK, examples, and active architecture
contracts contain no Luau runtime/dependency or legacy package caller.
`rules.lua` remains only as an intentionally invalid, no-Loader cutover fixture:
the strict component still reaches Play, emits its greeting, and answers
`/hello`, proving it is not executed as a fallback. P0's native AArch64
component gate and P7's strict five-component server-only/Loader/storage/reload
gates are accepted. Final-tree formatter, strict Clippy, code-health, and
workspace/all-target tests passed; standalone `mc-server` built without a
sibling repository or guest compiler. The only final source correction removes
a redundant `PluginLimits::clone()` of a `Copy` value in the standard-pack test.
Independent review `P8CutoverReview` passed with no findings. Receipt:
`.analysis/codex-logs/p8-cutover-audit-20260921/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 0801c54790bdb37a188bd0699a4d68425ede08b89b68ac57499eaf58e5992bca
diff_hash_recipe: >
  SHA-256 of git diff --binary base_tree --
  crates/mc-test-harness/tests/plugin_standard_pack.rs; cursor/evidence
  updates are deliberately excluded.
changed_files:
  - crates/mc-test-harness/tests/plugin_standard_pack.rs
  - docs/MEMORY.md
validation:
  - fmt: 20260922T000431-fmt-zdfu8fdm passed
  - clippy: 20260921T234139-clippy-3nw87l7z passed
  - code-health: 20260921T234155-code-health-nc6xfimy passed
  - test: 20260921T234210-test-1bzklpuy passed
  - build: 20260921T234155-build-mn7ls_iy passed
  - reviewer: P8CutoverReview passed
next: CP-001 audit existing Rust/WASM cargo-delivery evidence.
```

**CP-001 accepted — cargo delivery through the shipped Settlements component
(2026-09-22).** The dedicated `settlement-cargo` profile rebuilds and
SHA-256-checks `solaris-settlements/plugin.wasm` from its Rust source, then
strictly deploys the full package. Its actual Wasm handler receives production
boundary commands, persists its guest-owned counter, accepts the correlated
bound-warehouse result, and emits an admitted `AssignWork::Haul` from
`ResidentCarry(resident-1)` to the returned bound warehouse for
`minecraft:birch_log` at revision 3. The existing `worker_haul_*` core owner
cases execute that directed request against a real chest and cover merge,
full-destination refusal, restart/replay idempotence, and mixed cargo where a
later stack fits. The tracked component artifact was stale before this gate;
it was rebuilt from reviewed source and its final SHA-256 is
`6411e420d35d9cea17613a9d8102478aeb2ce2c6331aef7d671a0bf55ba8742e`.
Independent review `CP001CargoReview` passed with no findings. Receipt:
`.analysis/codex-logs/cp001-settlement-cargo-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: e56ff322fdaef9352a2b57c3c3204f7ae3667b6e39e446ad3f35a468401fed81
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-test-harness/tests/settlement_component_haul.rs tools/harness/profiles.py;
  cursor/evidence updates are deliberately excluded.
sibling_untracked_content_sha256:
  tools/build_standard_pack.py: 6027ace455668730503225ad664996743b84200891fb9cd91adc5ff282eb2084
  solaris-settlements/plugin.wasm: 6411e420d35d9cea17613a9d8102478aeb2ce2c6331aef7d671a0bf55ba8742e
changed_files:
  - crates/mc-test-harness/tests/settlement_component_haul.rs
  - tools/harness/profiles.py
  - ../solaris-default-plugins/tools/build_standard_pack.py
  - ../solaris-default-plugins/solaris-settlements/plugin.wasm
  - docs/MEMORY.md
  - docs/AGENT_TOOLING.md
validation:
  - settlement-cargo: 20260922T001746-settlement-cargo-d1fz8sa6 passed
  - harness-check: 20260922T002334-harness-check-amjjpmhd passed
  - fmt: 20260922T002401-fmt-ocm4w57r passed
  - code-health: 20260922T002418-code-health-0prcg_mm passed
  - reviewer: CP001CargoReview passed
next: CP-002 reproduce current Wasm settlement readiness under controlled load.
```

**CP-002 accepted — current Wasm settlement readiness (2026-09-22).** The old
Lua `settlement_pause_repro` target remains absent; the removal belongs to
`b454200b` and is not misrepresented as a repaired historical flake. Its
observable wire-client intent is now reproduced by
`settlement_wasm_readiness`: actual shipped `solaris-settlements` Wasm is
strictly deployed, real player commands create/adopt a core site, wait for
world chunk delivery, survey, project a `solaris:watch_post`, provide its real
palette materials, reserve them, and commit a structure portion. The same real
player receives `Structure materials reserved.` and `Structure portion
committed.` through the guest and existing owner boundary. Four owned CPU
spinners leave the complete flow passing in 18.88 s with no swaps. Current
host controls also passed 16 cases across `host_runtime` and
`startup_lifecycle`, covering startup-budget, configure/check, queue-capacity,
capability, and epoch-deadline refusals. No timeout, retry, fuel, epoch, or
queue budget changed. Independent review `CP002ClosureReview` passed with no
findings. Receipt:
`.analysis/codex-logs/cp002-wasm-readiness-20260922/receipt.txt`.

**CP-003 accepted — warehouse-to-worker issuance (2026-09-22).** The shipped
`solaris-settlements` Wasm component emits a real admitted Warehouse →
ResidentCarry/ResidentEquipment haul after its durable guest counter update;
no player principal or second inventory authority participates. The existing
owner commits the container after-image and resident record in one journal
decision. The cargo profile proves named carry and equipment issue with damage,
enchantment, custom-name, and item-model preservation across storage reopen;
insufficient, full, foreign, unknown, stale, and replay outcomes; and an
allowed generated village chest retaining its resolved physical source and one
issued result after reopen. Existing owner-fence tests cover
foreign/unknown/unloaded/stale container state and post-commit publication to
every open viewer. Independent review found the original component-preservation
evidence insufficient; the profile now carries a component-bearing, durable
reopen assertion and passes in 22.5 s. Strict Clippy and the workspace/all-target
test gate passed; no warning suppression was added. Receipt:
`.analysis/codex-logs/cp003-warehouse-withdrawal-20260922/receipt.txt`.


**CP-004 accepted — live warehouse material reservation (2026-09-22).**
Warehouse reserve already fenced every owner-bound physical chest against its
durable ledger. The remaining release transition now rebuilds that chest's live
reservation floor only after its owned-inventory decision is durably committed.
A released three-log reservation therefore makes the same two-log player
after-image admissible; a replay returns the one stored release outcome and
cannot return stock twice. The cargo profile also covers two reservations
exhausting actual stock, overpromise refusal, partial player pickup, admission
against a concurrent chest commit, source loss, and restart/unloaded recovery.
There is no global reservation service or second inventory authority. Independent
review `CP004ReservationReview` found no P0/P1/P2 defect; its caveat that the
new focused test observes the physical admission predicate rather than moving an
item is covered by the profile's existing real chest-commit/resync cases.
Receipt: `.analysis/codex-logs/cp004-warehouse-reservation-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: ecb4443e88cb499233ca219a0207b9f59c62cd660d09115ee15a02ef0516e693
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-net/src/script/storage/world_inventory.rs
  crates/mc-net/src/script/storage/settlement_tests.rs
  tools/harness/profiles.py docs/AGENT_TOOLING.md;
  cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-net/src/script/storage/world_inventory.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - tools/harness/profiles.py
  - docs/AGENT_TOOLING.md
  - docs/MEMORY.md
validation:
  - settlement-cargo: 20260922T014609-settlement-cargo-dspcgrjp passed
  - harness-check: 20260922T014711-harness-check-l3cfexn4 passed
  - fmt: 20260922T014728-fmt-wqjl0np5 passed
  - code-health: 20260922T014716-code-health-4k567se1 passed
  - clippy: 20260922T014735-clippy-l3hcaaav passed
  - reviewer: CP004ReservationReview passed
next: Complete CP-005 controlled villager material logistics.
```

**CP-005 accepted — one atomic building portion (2026-09-22).** The bounded
existing portion transaction remains one server-owned block decision carrying
the same prepared plugin batch for material consumption, structure progress,
and receipt. An underfunded matching-plan reservation now proves refusal leaves
all three unchanged. Independent review found a real ordering gap: a kept
`before-build` ticket could be consumed before its portion waited for journal
order. The ticket now survives that wait and is consumed only after the
cross-region transaction revalidates source images, immediately before its
receipt append and publish. A roster revoked while a predecessor holds the
journal turn produces no blocks or recoverable receipt. Cancellation, changed
footprint, competing builder, append-fault recovery, and reopen replay remain
in the focused profile. The reviewer found no defect in the candidate test
itself; its ordering finding was fixed without a second review, per the
checkpoint review policy. Receipt:
`.analysis/codex-logs/cp005-structure-portion-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: 9a6f5cce8e5730201dedd2dc7508a0d583606ae2668f9acb2c99564332eb9574
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-net/src/play.rs crates/mc-net/src/play/simulation.rs
  crates/mc-net/src/play/simulation/tests/precommit_tests.rs
  crates/mc-net/src/play/tests/scheduled_buttons.rs
  crates/mc-net/src/script/storage/settlement_tests.rs
  tools/harness/profiles.py docs/AGENT_TOOLING.md;
  cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/play/simulation/tests/precommit_tests.rs
  - crates/mc-net/src/play/tests/scheduled_buttons.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - tools/harness/profiles.py
  - docs/AGENT_TOOLING.md
  - docs/MEMORY.md
validation:
  - settlement-cargo: 20260922T020950-settlement-cargo-bz7h7heb passed
  - fmt: 20260922T021048-fmt-48aqtcg7 passed
  - harness-check: 20260922T021059-harness-check-ffuybdna passed
  - code-health: 20260922T021059-code-health-k41pjrr1 passed
  - clippy: 20260922T021104-clippy-1w7i8yol passed
  - reviewer: CP005TransactionReview finding fixed
next: Complete CP-006 pause, cancel, and resume construction safely.
```

**CP-006 accepted — durable construction lifecycle (2026-09-22).** Pause,
resume, and cancel use the existing authoritative structure revision and
reservation projection: a partial portion stays consumed and its placed blocks
are never removed; cancel returns only the remaining material and is
idempotent. New storage-level reopen coverage pauses after two blocks, reads
the identical paused status after a new runtime/storage open, resumes without a
second portion, cancels using that reopened revision, and replays cancel without
a new intent. Existing partial-cancel coverage directly asserts the two blocks
stay in the world; CP-005 journal-fault recovery covers the portion/ledger
boundary. Independent review `CP006LifecycleReview` passed with no P0/P1/P2
finding and notes the new reopen test deliberately observes durable
storage-state, while its fresh fake world cannot stand in for world-image
recovery. Receipt:
`.analysis/codex-logs/cp006-construction-lifecycle-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: bbe33fe302e06c41ad7ad114786beb62e5a9ec80fc417e3ee89ca084113b0eb6
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-net/src/script/storage/settlement_tests.rs
  tools/harness/profiles.py docs/AGENT_TOOLING.md;
  cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - tools/harness/profiles.py
  - docs/MEMORY.md
validation:
  - settlement-cargo: 20260922T021417-settlement-cargo-htnq9mzn passed
  - fmt: 20260922T021525-fmt-560ad23k passed
  - harness-check: 20260922T021534-harness-check-co_wuypr passed
  - code-health: 20260922T021534-code-health-uue76ins passed
  - clippy: 20260922T021540-clippy-nmz3k856 passed
  - reviewer: CP006LifecycleReview passed
next: Complete CP-007 durable mining cargo receipts.
```


**CP-007 accepted — durable mining cargo receipt (2026-09-22).** Mining already
staged canonical break loot, cargo after-images, and the resident work watermark
through the same world-journal decision. Added the missing mining-specific
projection-fault regression: after the world accepts iron-ore removal, a forced
plugin-storage write failure returns durability unknown; reopening projects the
accepted receipt once. The existing full-cargo mine test proves capacity refusal
leaves ore untouched with zero work. Replaying the original fault-injected
operation reports one work unit and leaves exactly one raw iron in worker
cargo—no dropped block, lost progress, or duplicate loot. Independent review
`CP007MiningReceiptReview` passed with no findings. It is
journal/plugin-storage recovery, not a power-loss or full
world-process-restart claim. Receipt:
`.analysis/codex-logs/cp007-mining-cargo-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: bf1065fab5b54cc75ef1b354b531dfb54e7edb3effc5d9be40565ae05a872768
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-net/src/script/storage/resident_order_tests.rs
  tools/harness/profiles.py; cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - tools/harness/profiles.py
  - docs/MEMORY.md
validation:
  - focused test: mine_recovers_cargo_and_progress_after_storage_projection_fails passed
  - settlement-cargo: 20260922T022009-settlement-cargo-upe1k752 passed
  - fmt: 20260922T022106-fmt-eyzrx2hv passed
  - code-health: 20260922T022111-code-health-u6u13y7p passed
  - harness-check: 20260922T022113-harness-check-1uuj39af passed
  - clippy: 20260922T022114-clippy-5ekg9zi7 passed
  - reviewer: CP007MiningReceiptReview passed
next: Complete CP-008 agricultural resource cycle.
```

**CP-008 accepted — real agricultural cycle (2026-09-22), corrected maturity
fence.** Harvest now requires an authoritative registered mature crop state
before it previews canonical break loot: age 7 for wheat/carrots/potatoes, age
3 for beetroot/nether wart/sweet berries, age 2 for cocoa, and the produced
fruit/cane blocks themselves. Unknown or unaged states fail closed. Coverage
harvests and replants, attempts a second harvest while the replanted wheat is
still age 0 (MissingInput with zero work and no loot), applies an external
mature-world update, then completes the second harvest/replant cycle. Both
harvest outputs remain worker-owned. The public matrix also runs missing-seed,
missing-tool, stale-field, and full-cargo refusal cases. This is a real
world-event fixture, not a claim that settlement timers produce crops.
`CP008AgriculturalReview` and the correction review `CP008MaturityReview`
passed with no findings. Receipts:
`.analysis/codex-logs/cp008-agricultural-cycle-20260922/receipt.txt` and
`.analysis/codex-logs/cp008-crop-maturity-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: 2f363e1273363d20076a6b22e9e7031e44cfb12a893af478d043c330344d3d8c
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-net/src/play/resident_work.rs
  crates/mc-net/src/script/storage/resident_order_execution.rs
  crates/mc-net/src/script/storage/resident_order_tests.rs
  crates/mc-net/src/script/storage/resident_settlement_tests.rs
  tools/harness/profiles.py; cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-net/src/play/resident_work.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - tools/harness/profiles.py
  - docs/MEMORY.md
validation:
  - focused test: a_matured_replanted_crop_completes_a_second_cycle passed
  - settlement-cargo: 20260922T024903-settlement-cargo-bp6x49x6 passed
  - fmt: 20260922T025013-fmt-jk74fxbz passed
  - code-health: 20260922T025018-code-health-r3776cqp passed
  - harness-check: 20260922T025021-harness-check-e1fnr137 passed
  - clippy: 20260922T025021-clippy-pjc5rxgz passed
  - reviewer: CP008AgriculturalReview and CP008MaturityReview passed
next: Complete CP-010 honest resident mining.
```

**CP-009 accepted — bounded real-tree logging (2026-09-22).** Tree selection
remains a bounded rooted-trunk-plus-canopy check; grounded non-canopied house
logs remain excluded. CutTree now fences each conditional break through a live
resident's bounded path to a standable neighbour of the tree root. A missing
snapshot, unavailable world route, or blocked route produces a typed refusal
without a block change. The first review found that upper trunk cells were
using their own elevation for the route and could be stranded after lower
progress. The correction reuses the bounded rooted standing height, proven by
a three-log tree. The matrix covers real tree cargo, house protection, blocked
route, missing tool, full cargo, and exact resumption after a permission fence
is removed. Receipt: `.analysis/codex-logs/cp009-real-trees-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: 4d73e66dcedebd0df6fbbd416b2723c6416677e6768d60066eaa286017b7182d
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-net/src/script/storage/resident_order_execution.rs
  crates/mc-net/src/script/storage/resident_order_tests.rs
  tools/harness/profiles.py; cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - tools/harness/profiles.py
  - docs/MEMORY.md
validation:
  - focused tree tests: cut_tree_ and rooted tall trunk passed
  - settlement-cargo: 20260922T024323-settlement-cargo-d09g2sl8 passed
  - fmt: 20260922T024434-fmt-3jveizr_ passed
  - code-health: 20260922T024438-code-health-ww0gav7l passed
  - harness-check: 20260922T024441-harness-check-n6g07cn3 passed
  - clippy: 20260922T024441-clippy-v_4tat9v passed
  - reviewer: CP009RealTreeReview found and localized upper-trunk route defect;
    fixed with rooted-route lookup and tall-tree regression; no second review by policy
next: Complete CP-010 honest resident mining.
```
**CP-010 accepted — honest resident mining (2026-09-22).** Mine retains its
bounded requested area, canonical break preview, tool/drop gate, capacity fence,
protection check, and durable receipt. It now also requires a live resident
snapshot with a bounded open route to an actual mining stance: a standable
horizontal neighbour at ore level or the cell directly above it. Hidden ore no
longer becomes cargo merely because it appears in the request area; missing
snapshots and unavailable route data fail closed. The fixture's normal ore has
a narrow physical stair; sealed ore remains untouched with `BlockedRoute`.
The matrix proves accessible canonical loot/work, unsuitable tool, full cargo,
protection, stale source, unloaded target, later tier gate, and receipt
recovery. `CP010MiningReview` passed with no findings. Receipt:
`.analysis/codex-logs/cp010-honest-mining-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: b1fad33e9974f8fc1ad1c2b2ab49f9cf7101e733ae71e68e54b2db1f0ec63d68
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-net/src/script/storage/resident_order_execution.rs
  crates/mc-net/src/script/storage/resident_order_tests.rs
  tools/harness/profiles.py; cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - tools/harness/profiles.py
  - docs/MEMORY.md
validation:
  - focused mining tests: mine_ and stale preview passed
  - settlement-cargo: 20260922T025905-settlement-cargo-27ktzoiw passed
  - fmt: 20260922T030050-fmt-m2c3kzgn passed
  - code-health: 20260922T030054-code-health-4qf2ojiv passed
  - harness-check: 20260922T030057-harness-check-m_jnd4j8 passed
  - clippy: 20260922T030057-clippy-wz76jbyj passed
  - reviewer: CP010MiningReview passed
next: Complete CP-011 fishing and livestock resource contract.
```

**CP-011 accepted — fishing and livestock resource contract (2026-09-22).**
Fish and tend now require a live resident plus a bounded route to the actual
water shore or adjacent animal stance. A present source behind a closed route
does not become remote cargo or consume feed. Fishing retains its deliberately
deterministic one-cod-per-reachable-column worker contract: the target vanilla
fish table is present locally and weighted, but the worker API does not
evaluate that table or bobber timing, so this checkpoint does not claim vanilla
fishing parity. Tending consumes feed only for a live mature compatible animal;
it produces no virtual livestock items. Tests cover source removal, no storage,
no route, no feed, maturity/compatibility, death, missing source, replay after
storage reopen, and loaded boundary stances. The reviewer found that an
unloaded first candidate incorrectly hid a reachable later candidate; the
fix records unavailable candidates while trying the whole bounded set, with
boundary regressions. No second review by policy. Receipt:
`.analysis/codex-logs/cp011-resource-contract-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: 6f7e3bb326090ea0f77ff8512a8c4497a1f3f360f78d214c2eb2e23b5ac89a1f
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-net/src/script/storage/resident_order_execution.rs
  crates/mc-net/src/script/storage/resident_order_tests.rs
  tools/harness/profiles.py; cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - tools/harness/profiles.py
  - docs/MEMORY.md
validation:
  - focused fish, tend, dead-source, and boundary-stance tests passed
  - settlement-cargo: 20260922T031937-settlement-cargo-glc61_yz passed
  - fmt: 20260922T032052-fmt-vuf1uq8r passed
  - code-health: 20260922T032057-code-health-27u0i5vp passed
  - harness-check: 20260922T032059-harness-check-dyz2op78 passed
  - clippy: 20260922T032100-clippy-i_l83zj9 passed
  - reviewer: CP011ResourceReview found stance-order defect; fixed with
    boundary regressions; no second review by policy
next: Complete CP-012 warehouse-backed production.
```

**CP-012 accepted — warehouse-backed workshop production (2026-09-22).**
Workers now execute shared recipes only at their matching stateless station:
crafting table, lit campfire, or stonecutter. Furnace, smoker, and blast-furnace
recipes remain with their live fuel/burn state rather than becoming free
instantaneous crafts. Each resident craft stages ingredient removal, output,
and item-component remainders on one record after-image, then commits it with
the durable receipt; no-capacity rejection preserves even a named/component
input exactly. Shared tag membership no longer assumes protocol-id ordering, so
valid tag-driven recipes such as cake consume inputs and return their container
remainders. The warehouse flow remains explicit: bound-container withdrawal,
resident craft, and return transfer; the integration test reopens storage
before replaying the accepted craft receipt. `CP012WorkshopReview` found
component-bearing rollback loss and unlit campfire cooking; both have focused
regressions and are fixed. No second review by policy. Receipt:
`.analysis/codex-logs/cp012-workshop-production-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: 444e8192cc34c052fa034b3e4f66172c0c0238ed9d30c8435f84df4054c7f2ce
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-data/src/tags.rs
  crates/mc-net/src/play/resident_work.rs
  crates/mc-net/src/script/storage/resident_order_execution.rs
  crates/mc-net/src/script/storage/resident_order_tests.rs
  crates/mc-net/src/script/storage/resident_settlement_tests.rs
  crates/mc-script/src/resident_order_operations.rs
  crates/mc-script/wit/residents.wit
  tools/harness/profiles.py; cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-data/src/tags.rs
  - crates/mc-net/src/play/resident_work.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - crates/mc-script/src/resident_order_operations.rs
  - crates/mc-script/wit/residents.wit
  - tools/harness/profiles.py
  - docs/MEMORY.md
validation:
  - focused tag, craft, warehouse replay, campfire, and component-capacity tests passed
  - settlement-cargo: 20260922T035355-settlement-cargo-hugvmxzi passed
  - fmt: 20260922T035355-fmt-qpci7xkm passed
  - code-health: 20260922T035400-code-health-4l7h3lhx passed
  - harness-check: 20260922T035422-harness-check-dbq1794p passed
  - clippy: 20260922T035422-clippy-q2_ineib passed
  - reviewer: CP012WorkshopReview found component rollback and unlit campfire
    defects; both fixed with regressions; no second review by policy
next: Complete CP-013 event-driven paused-work resumption.
```

**CP-013 accepted — event-driven paused-work resumption (2026-09-22).**
Paused work now keeps whether `missing_input` waits on a bounded world source
or an addressed inventory transfer. A matching world region resumes only the
former; tools, recipe inputs, seeds, feed, resident capacity, and warehouse
capacity continue to wait for their real inventory event. The source wake
persists across Store reopen and advances the same durable watermark once;
completed work drops from the pause index. Existing native receipts already
carry the real `work-assignment` state and typed reason, including actor-driven
resumes. The active-owner gate remains before execution, and chunk/zone/
warehouse lookups remain bounded indexes rather than a tick scan. Receipt:
`.analysis/codex-logs/cp013-event-resumption-20260922/receipt.txt`.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: 40dfaba4f8430868dbc25189717f715f8336638fdc9e69b71492ba614f9978a8
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD --
  crates/mc-net/src/script/storage/resident_order_execution.rs
  crates/mc-net/src/script/storage/resident_order_tests.rs
  crates/mc-net/src/script/storage/resident_orders.rs
  tools/harness/profiles.py; cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - crates/mc-net/src/script/storage/resident_orders.rs
  - tools/harness/profiles.py
  - docs/MEMORY.md
validation:
  - focused world/inventory wake, Store-reload, owner-active, chunk, zone,
    warehouse, and actor-receipt regressions passed
  - settlement-cargo: 20260922T040652-settlement-cargo-4r1zt4oc passed
  - fmt: 20260922T040840-fmt-3m02_fkl passed
  - code-health: 20260922T040844-code-health-ochclu2a passed
  - harness-check: 20260922T040847-harness-check-75z74c6g passed
  - clippy: 20260922T040848-clippy-qust4u9p passed
  - reviewer: CP013ResumptionReview found inventory-backed missing-input
    world retries; causal classification and a no-receipt regression fixed it;
    no second review by policy
next: Complete CP-014 unified R1/R0 acceptance scenario.
```

**CP-014 recheck — draft remains blocked (2026-09-22).**
The exact deployed Rust component now exposes a bound warehouse funding path,
prints deterministic home coordinates, and persists the real initial work
fence `0` rather than the distinct resident-ledger spawn revision. An accepted
work response remains the source of the successor fence. The cancellation
repair also clears `world_input_wait` before a cancelled durable record is
reopened. None of those component and storage checks establishes the required
R1 product chain.

The real headless package route still reaches `create → site → adopt → survey
→ project → fund → advance`; its CP-002 readiness test passes only with
test-granted construction materials. Extending that route to populate a worker
first refused `Unloaded`; after the exact home chunk was loaded, core correctly
refused `Blocked`: the selected deterministic `site_*` layout has no physical
standable home. A layout is not housing. The removed extension and exact chat
evidence are recorded in
`.analysis/codex-logs/cp014-worker-population-20260922/receipt.txt`.

Static trace found a second R1 blocker: `advance_structure` records a portion's
quantities as reservation consumption, but
`commit_prepared_structure_portion` commits only block edits and its storage
receipt. It has no physical player/warehouse container mutation, then lowers
the warehouse reservation floor. A warehouse-funded structure therefore does
not yet prove physical material consumption. `settlement-cargo` proves the
individual cargo, reservation, structure, source-work, and resume mechanics,
not their required physical end-to-end chain.

The independent `CP14BlockerReview` found one component defect: the spawn
snapshot revision had been advertised as the first work fence although a new
resident's order record starts at `0`. The repair persists/displays `0` and
adds regression coverage; no second reviewer is run by policy. The review
also confirmed that keeping CP-014 draft is justified, not a CP-002 test setup
artifact. Real-client R0/R1, two observers, rejoin, and recovery remain
owner-deferred. No L2 was run because CP-014 is unaccepted.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: 90a3a5305bf58cb5d9217a0bc9291d15ac5954a3e33b58779b3489aeb2aa467f
core_diff_hash_recipe: >
  SHA-256 of git diff --binary 87af4502a79864392798bcb52634e6d2d68ce9d4 --
  crates/mc-net/src/script/storage/resident_order_execution.rs
  crates/mc-net/src/script/storage/resident_order_tests.rs
  sdk/rust/solaris-plugin-sdk/src/inventory_ops.rs
component_hashes:
  source: 26959b232f2450472de854c563191fbcd0e1578dace036d4f54fa7271be42a1a
  wasm: d808dc53f24f99f7913c176631430a6c937c62a7ab9aa0bb0cf3e2a9d66c40a9
  readme: 71f9182014ff017f05a564197fc0b6b011fa8ee0017135d3b113a4206dc1a842
changed_files:
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - sdk/rust/solaris-plugin-sdk/src/inventory_ops.rs
  - ../solaris-default-plugins/sources/solaris-settlements/src/lib.rs
  - ../solaris-default-plugins/solaris-settlements/plugin.wasm
  - ../solaris-default-plugins/solaris-settlements/README.md
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net cancelling_a_world_input_wait_reopens_cleanly --lib passed
  - sibling cargo test passed: 40
  - sibling package source/deployed hash matched: d808dc53f24f99f7913c176631430a6c937c62a7ab9aa0bb0cf3e2a9d66c40a9
  - cargo test -p mc-test-harness --test settlement_wasm_readiness passed
  - settlement-cargo: 20260922T045003-settlement-cargo-2gas66in passed
  - root fmt: 20260922T043939-fmt-iowbfu8z passed
  - code-health: 20260922T044153-code-health-ra7fla7l passed
  - harness-check: 20260922T044159-harness-check-agmhwt55 passed
  - root clippy: 20260922T044207-clippy-niophqv4 passed
  - component strict clippy is not green: seven pre-existing lint failures
    (three too_many_arguments, three clone_on_copy/unnecessary_filter_map,
    and one test field_reassign_with_default)
  - reviewer: CP14BlockerReview found and the author fixed the initial work-fence mismatch
next: Atomically debit the reserved physical endpoint in the same world decision as a structure portion.
```

**CP-014 material-debit repair — draft remains blocked (2026-09-22).**
Every committed construction portion now debits only its bound physical
warehouse chest. The planner produces exact 27-slot before/after images, and
the resident cross-region transaction stages those images, structure blocks,
and the plugin receipt under one durable world-journal decision. A failed
persistence closure publishes neither image nor receipt; the owner rejects a
stale chest image, a stock increase, or an admission already held by another
warehouse transition. The reservation floor is derived from the canonical
before/after item totals, not a second release authority.

Post-commit publication resolves a valid double chest through the canonical
world registry and publishes its combined 54-slot image under the sorted
primary position. A debit from the secondary half therefore refreshes the
active double-chest window rather than being silently ignored.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
core_diff_hash: 23bb1522985745e2bdc8698d5ecd9ca4d2b77d84712643593610b7e3db85b720
core_diff_hash_recipe: >
  SHA-256 of git diff --binary HEAD -- the 12 source/test paths in
  changed_files; cursor/evidence updates are deliberately excluded.
changed_files:
  - crates/mc-world/src/resident.rs
  - crates/mc-world/src/resident/inventory.rs
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/owned_inventory.rs
  - crates/mc-net/src/play/session.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/script/storage.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - crates/mc-net/src/script/storage/world_inventory.rs
  - crates/mc-net/src/settlement.rs
validation:
  - cargo test -p mc-world structure_and_material_chest_share_one_durable_cross_region_decision --lib passed
  - cargo test -p mc-net structure_portion_journals_block_after_image_with_its_receipt --lib passed
  - cargo test -p mc-net settlement_tests --lib: 59 passed
  - cargo test -p mc-net construction_debit_releases_only_its_warehouse_reservation_floor --lib passed
  - cargo test -p mc-net construct_work_consumes_the_reserved_portion_and_commits_the_stage --lib passed
  - cargo check -p mc-net passed
  - fmt: 20260922T131612-fmt-aw56cu2u passed
  - code-health: 20260922T131619-code-health-k1nlvjwe passed
  - reviewer: FinalMaterialDebitReview found double-chest publication; fixed with a regression, no second review by policy
next: resolve the loaded-site physical-home blocker, then run the owner-authorized R1 real-client scenario; R0 acceptance remains deferred by owner.
```

**P5 precommit revalidation (2026-09-21).** Six real-component host hook cases,
15 bounded ticket/roster/deadline cases, and 27 native build/damage precommit
cases passed. The cross-region receipt test now reflects the actual
preparation/submission boundary: a cancelled `before-build` approval reaches the
regional worker but is refused before block mutation or world-journal
reservation; its prepared storage is never projected. ADR 0004 records that
boundary. Formatter and code-health passed again at
`.analysis/validation/20260921T015413-{fmt-du9288he,code-health-wjjcq4_u}/`.
Receipt: `.analysis/codex-logs/p5-precommit-revalidation-20260921/receipt.txt`.

**P6 reload/shutdown revalidation (2026-09-21).** The prior L2 workspace-test
blocker was an unrelated `set -u` defect in real-client prepare: the optional
`SCENARIO_SERVER_PORT` override was never initialized. Its
`${SCENARIO_SERVER_PORT:-}` initialization preserves an explicitly exported
port and otherwise leaves the source `network.port` intact. All 49
manifest-preparation tests passed after the correction; the failed workspace
test stage had already passed on rerun. The prior formatter, strict workspace
Clippy and code-health stages remain green. Receipt:
`.analysis/codex-logs/p6-revalidation-20260921/receipt.txt`.

**P7 package-source recovery and Basic Economy Wasm port (2026-09-21).** The
five API 0.7 standard-pack sources were recovered from core revision
`7f68cc2a`; boundary revision `b454200b` removed those paths from core so the
sibling owns first-party content. The explicit sibling build script regenerates
and byte-verifies the strict five-package deployment plus the `basic-economy`
client fixture; the strict five still stage as the only expected production
pack. The standard-pack profile passed its source check, ignored component
behavior target, strict startup, and five-package configuration:
`.analysis/validation/20260921T121331-standard-pack-7q_6e3_v/result.json`.

Following the owner's port decision, `../solaris-default-plugins/basic-economy`
is now an API 0.7 component with `entry = "plugin.wasm"` and independently
reproducible Rust source at `sources/basic-economy/`. It preserves the
`basic-economy` ID, `shop:economy:<uuid>` v2 ledger, configured zone/menu
entry, one-dirt/two-apple primary purchase, secondary refund, and the
inventory-storage/CAS authority path. Independent review found that an early
departure could retain pending state and that a rejected multi-command callback
could roll back only its final correlation. The component now cleans each exact
departed session (including pending reads, transactions, menus, notices, and
rejection state) and rolls back every correlation in a rejected output batch.
Four component unit tests, including both regressions, and the final
six-component `tools/build_standard_pack.py --write` plus `--check` passed; the
deployed Basic Economy component SHA-256 is
`45a3e70fa5c1a1fad9d3d9105a3e30c6da142d2b43e0711bb723e2d128e45d3d`.

The final real-client core profile passed:
`.analysis/validation/20260921T133734-core-client-a7okwwgp/result.json`.
Its server-only flow mined and picked up natural dirt, purchased two apples
with the ledger at one, then refunded to one dirt, zero apples, and ledger
ownership zero. The final inventory profile passed:
`.analysis/validation/20260921T134225-inventory-omn1dveo/result.json`; it
preserved the purchase result into the existing cursor/held-item/crafting
probe. The inventory-only branch intentionally leaves the bought apples
available to that pre-existing probe; refund remains asserted by `core-client`.
`harness-check` passed at
`.analysis/validation/20260921T122939-harness-check-ud9nbnj3/result.json`.
The latest root `fmt` and `code-health` passes are
`.analysis/validation/20260921T134726-fmt-ueordzad/result.json` and
`.analysis/validation/20260921T134736-code-health-rtmmc29e/result.json`.

The historical API 0.6 and `main.lua` component-rejection failures remain
recorded in the P7 receipt; they are not passing evidence. The host now reports
bounded per-component callback counts and p50/p95/p99/max latency at generation
stop. The strict five-component behavior test asserts that its actual
normal/event-storm economy run produces ordered metrics, and the final
core-client record observed Basic Economy at 13 callbacks with p50 185 us and
p95/p99/max 276 us:
`.analysis/validation/20260921T133734-core-client-a7okwwgp/server-only/server.log`.
The final inventory record observed eight callbacks with p50 172 us and
p95/p99/max 245 us:
`.analysis/validation/20260921T134225-inventory-omn1dveo/server-only/server.log`.

The canonical strict-pack profile binds the five actual components to a real
`mc-net` server under the debug profile, seed 0, zero connected clients, and a
fixed 100-tick post-normal phase. Independent review found and corrected two
gate defects: the telemetry snapshot now must reach `source_tick >= target`,
and a continuously fed queue is sampled rather than asserted instantly empty.
The corrected
`.analysis/validation/20260921T180218-standard-pack-djhdo778/result.json`
recorded tick p50/p95/p99/max 746/1045/1482/1619 us, Economy callback
p50/p95/p99/max 171/263/284/344 us over 165 callbacks, storm queue peak 62,
sampled pre-shutdown depth zero, 67/5 user/system CPU ticks, VmHWM
181204→211744 KiB, and zero host refusals. The strict-pack Xvfb gate passed:
`.analysis/validation/20260921T141704-standard-pack-qvsi6yy9/result.json`;
all five components reached Play and `/money` returned `Balance: 100 coins.`

P7 is accepted. The zero, normal, flood, infinite, slow-storage, event-storm,
and reload scenarios execute in strict component tests; the canonical live
workload adds actual-server tick/latency/queue/CPU/RSS evidence. The owner
explicitly waived the plan's unrecoverable historical Luau comparison after
confirming that the retained P0 baseline is current Wasm-only and recovering
the old runner would cross the 453-file `d4d65062`→`v0.0.7` transition. Receipt:
`.analysis/codex-logs/p7-first-party-source-audit-20260921/receipt.txt`.

```yaml
base_tree: b454200b8249ec307b831838b6b2ddf295db0a4d
diff_hash: 8a27f57ff0fcdbd281049397cecef57cc752558fb6683968a6255fa09fbfd005
diff_hash_recipe: >
  SHA-256 of `git diff --binary b454200b8249ec307b831838b6b2ddf295db0a4d`,
  snapshot immediately before this record update.
changed_files:
  - crates/mc-net/src/play/ingress/player_control.rs
  - crates/mc-net/src/play/simulation/tests/precommit_tests.rs
  - crates/mc-plugin-host/src/limits.rs
  - crates/mc-plugin-host/src/host.rs
  - crates/mc-plugin-host/src/lib.rs
  - crates/mc-plugin-host/tests/host_bounds.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-script/src/tick_delivery_tests.rs
  - crates/mc-server/src/startup/components.rs
  - crates/mc-test-harness/tests/load_scenarios.rs
  - docs/AGENT_TOOLING.md
  - docs/MEMORY.md
  - docs/decisions/0004-staged-single-writer-simulation.md
  - tools/harness/backends/regression.sh
  - tools/harness/compatibility.py
  - tools/harness/precommit.py
  - tools/harness/profiles.py
  - tools/harness/test_harness.py
sibling_checkpoint:
  base_tree: 92c51a37dee282e75b8fce709b8e720509f4f43f
  tracked_diff_hash: f313f6fc2295873e598eddb00d27daaabae58d8a8dd541ee701f8d2676efd754
  basic_economy_component_sha256: 45a3e70fa5c1a1fad9d3d9105a3e30c6da142d2b43e0711bb723e2d128e45d3d
validation:
  - cargo test --manifest-path ../solaris-default-plugins/sources/basic-economy/Cargo.toml: passed (4 tests)
  - ../solaris-default-plugins/tools/build_standard_pack.py --write/--check: passed
  - cargo test -p mc-plugin-host callback_latency_window_reports_bounded_nearest_rank_percentiles: passed
  - cargo test -p mc-plugin-host: failed (480-second timeout; do not treat as package-wide pass)
  - .analysis/validation/20260921T133208-standard-pack-e1l_zsad/result.json: passed
  - .analysis/validation/20260921T133734-core-client-a7okwwgp/result.json: passed
  - .analysis/validation/20260921T134225-inventory-omn1dveo/result.json: passed
  - .analysis/validation/20260921T134726-fmt-ueordzad/result.json: passed
  - .analysis/validation/20260921T134736-code-health-rtmmc29e/result.json: passed
  - .analysis/validation/20260921T180218-standard-pack-djhdo778/result.json: passed
  - .analysis/validation/20260921T141704-standard-pack-qvsi6yy9/result.json: passed (strict pack Xvfb `/money`)
  - .analysis/validation/20260921T142707-harness-check-kky8e73o/result.json: passed
  - .analysis/validation/20260921T180822-fmt-fv2tg8hu/result.json: passed
  - .analysis/validation/20260921T180822-code-health-2uehntbf/result.json: passed
status: P7 accepted under the owner's explicit waiver of the unrecoverable legacy-Luau comparison; strict source, behavior, live-server workload, and real-client evidence passed.
next: P8 remains blocked only by P0's unavailable native Linux AArch64 execution receipt.
```

**P8 production-cutover audit (2026-09-21).** The core production source and
lockfile have no Luau runtime, legacy entrypoint or Lua dependency reference,
and the standalone `mc-server` build passed without a sibling checkout or guest
compiler. The explicit standard-pack profile proves the sibling only participates
in its integration gate. P7 is accepted; P8 remains unaccepted only because P0
still lacks native Linux AArch64 execution evidence. CP-001 onward is no longer
source-blocked, but P8 cannot be treated as a verified prerequisite.
Receipt: `.analysis/codex-logs/p8-cutover-audit-20260921/receipt.txt`.


**Blocker.** The canonical profile list has no settlement scenario, and
`docs/real-client-regression/manifests/` declares none. The client MCP credential
is absent. The sibling package rules prohibit restoring product scenario wiring
to core. Unblock with an approved product-owned canonical R0/R1 manifest and
client credential, then run the declared graphical scenario.

