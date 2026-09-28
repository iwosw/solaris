# Archived checkpoint log — part 17 of 17

Era: 09-25/09-26. Chronological checkpoint history moved out of `docs/MEMORY.md` so
the live cursor stays small. **Not startup context** (see `AGENTS.md`); read it when
a question is about this era, not to learn the current state.

## Sections in this part

- CP-048 birth record layer, child service gate, roster evidence (2026-09-26)
- CP-047 combat recovery, treatment and demobilization closed headlessly (2026-09-25)

---

## CP-048 birth record layer, child service gate, roster evidence (2026-09-26)

**Status.** `draft`; durable birth record layer only. Runtime birth (native reserve/spawn execution, housing/food pause gates, migration transitions) remains unwired: the initiation triggers and precondition thresholds are an open runtime gap, not an owner decision. Only relocation waits on an owner decision (identity model + ADR). No commit authorized.

**Current result.** The sibling package records one birth as an immutable plan with reserved identity in the opening CAS: stable decision id first for replay, then per-settlement sequence for the child alias and the `-reserve`/`-spawn` ids (gaps on refusal, never reuse), script-id alphabet validation on all three ids before any mutation, capacity and household-parent checks at record time, all-or-nothing mutation, single sequence reservation (projection advances nothing further), scoped open-birth slot holds in `validate_intent`/`add_member`/roster writer, household-scoped parents, vacant-holder death handling with retire-on-refusal, refused-decision replay, generic-path commit blocked, and 64-byte id bounds. `commit_birth` projects the child once (roster alias, handle, household ref, age origin) and replays idempotently. Hire admits a child to service only at `tick - age_origin >= ADULT_AGE_TICKS` (18 calendar years per SPECIFICATION) with the callback tick threaded from `EventContext`; no-origin residents stay adopted adults. `assert_birth_evidence` pins the package-side roster/household/intent projection; core reserve/spawn/release `Occupied` occupancy stays asserted by the native storage suites.

**Review.** One independent read-only reviewer returned `pass` (confidence 0.86), no findings, after the birth-scope corrections. Fixing findings does not trigger a second reviewer.

**Checks.** Sibling `cargo test`: 82 passed, 0 failed; `cargo clippy --all-targets`: 0 errors; `rustfmt --check` on touched sibling files: 0 diffs. Core `cargo test -p mc-net --lib script::storage::`: 157 passed; `cargo test -p mc-script settlement`: 13 passed. Graphical acceptance `not run` (owner-deferred until after CP-096). No wire, sidecar, or schema change beyond the additive sibling registry fields.

**Blocked.** Only relocation waits on an owner decision (identity model + ADR per sibling IMPLEMENTATION_SPEC §5.5; unauthorized `MoveResidentHome` slice removed from core DTO/WIT/host/SDK/dispatch). Adult migration, birth runtime, housing/food pauses, and guardian-death recovery reissue wait on the open runtime gap: initiation triggers + precondition thresholds.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 087a81e714d963cf386d44970fc271a4104a1b5a9b6b78d5c11316c6ec1cd586  # sha256 of `git diff HEAD` over the core relocation-removal files (DTO, WIT, host, SDK, settlement.rs, residents.rs, tests, lib.rs)
changed_files:
  - crates/mc-net/src/script/storage/{settlement.rs,residents.rs,resident_settlement_tests.rs}
  - crates/mc-script/src/{settlement_operations.rs,lib.rs}
  - crates/mc-script/wit/settlements.wit
  - crates/mc-plugin-host/src/domain_settlements.rs
  - sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs
  - ../solaris-default-plugins/sources/solaris-settlements/src/{demography.rs,demography_tests.rs,hire_runtime.rs,hire_tests.rs,lib.rs}
validation:
  - sibling cargo test: 82 passed, 0 failed
  - sibling cargo clippy --all-targets: 0 errors
  - sibling rustfmt --check touched files: 0 diffs
  - core cargo test -p mc-net --lib script::storage:: : 157 passed
  - core cargo test -p mc-script settlement: 13 passed
status: CP-048 birth record layer complete with review; only relocation blocked on owner decision, birth/migration/pauses/guardian-death runtime on open trigger gap; no commit authorized
next: Pause on insufficient housing and food (record-time capacity holds exist; native gates need triggers); then guardian-death runtime reissue, then CP-049
```

## CP-047 combat recovery, treatment and demobilization closed headlessly (2026-09-25)

**Status.** `draft`; the CP-047 headless native and first-party-component scope is
complete, including the corrections from one independent read-only review. The
owner deferred graphical Minecraft acceptance until after CP-096, so this is not
client-accepted and not `stabilization`. Core keeps generic resident order,
inventory, world and entity authority; first-party settlement policy and its
integration tests live in `../solaris-default-plugins`.

**Current result.** Operational morale is a durable field on each serving
resident's order record (`script/storage/resident_morale.rs`); observed native
ally/officer losses, the physical flank and real arrival move it through
`Steady -> Shaken -> Wavering -> Routing -> Rallied`, and routing replaces the
interrupted chase with a native walk goal at the rally without moving the guard
itself. `/settlement retreat` issues one bounded revision-fenced native
`Retreat` that stops the old chase without teleporting; when the rally cell
cannot be placed the member now receives `Idle` while keeping its
`blocked_route` report (the durable-replay projection returns the same `Idle`,
source-qualified, no separate replay test). Demobilization retains native gear
provenance, refuses missing live residents and mints no second gear owner for a
dead one. `/settlement treat` spends one real warehouse bread for up to three
native health points as one WIF plus a linked world-journal acknowledgement
before publication, and startup projects an unprojected debit only with that
acknowledgement; a lower health revision still fails closed. Capture keeps the
victim's handle, UUID and gear and admits no second army membership, and
startup recovery reads the settlement registry once and now continues past a
refused dismissal to the remaining intents and to capture recovery. Resident
melee damage still commits one off-thread regional batch and then publishes
health, hurt/death and kill rewards through the same session path as every other
damage source; the publication reconciles the live snapshot, so motion, goals
and ticking timers no longer drop the committed health, while a newer hit or a
paid heal keeps ownership of the published value.

**Review.** One read-only reviewer (`Cp047Review-2`) returned `changes` with
three findings - the dropped health publication, the starved guest startup
recovery and the blocked-retreat chase - all confirmed in live code, corrected
and covered by RED/GREEN focused tests. Fixing findings does not trigger a
second reviewer. Raw evidence:
`.analysis/codex-logs/cp047-review-findings/receipt.md`,
`.analysis/codex-logs/cp047-resident-damage-publication/receipt.md`.

**Checks.** `cargo test -p mc-net --lib` on the final tree: 2377 passed, 0
failed, 8 ignored. Final gates, all on the corrected tree:

| Gate | CP-047 |
| --- | --- |
| Cargo baseline | `run correctness` passed: `.analysis/validation/20260925T053848-correctness-ho0j9n8o/result.json` (fmt, code-health and strict clippy exit 0; `cargo test --workspace --all-targets` exit 0 in 1621.6 s = 90 suites, 5165 passed, 0 failed, 192 ignored). |
| Integration | `settlement-cargo` passed: `.analysis/validation/20260925T060652-settlement-cargo-o8o4ljqj/result.json` (component hash match on the rebuilt guest, 7/7 component integration, 1/1 wasm readiness, 27/27 resident settlement, 75/75 resident order, 70/70 settlement, 2/2 warehouse deposit). |
| Focused tests | New moved-victim publication regressions and the blocked-rally retreat regression; each fails on the reverted tree. Guest starvation regression RED/GREEN recorded in the receipt. |
| Vanilla oracle | `not run` - no packet id/layout, registry id or vanilla data changed. |
| Client/manual gate | `deferred_by_owner`: graphical two-client medicine/retreat/rally/demobilization acceptance after CP-096; not run here. |
| Performance | `not run` - no performance claim; one bounded snapshot reconciliation per committed damage. |
| Concurrency | No new worker, lane or lock order: one regional batch off-thread, then one bounded pass under the existing session guard. |
| Data/protocol facts | `not run` - no wire, sidecar or schema change. |
| Persistence/storage | Treatment WIF + acknowledgement, capture intent replay, refused-dismissal recovery continuation and morale `goal_applied` across reopen covered; storage formats unchanged. |
| Dependencies | None added; `Cargo.lock` unchanged by this checkpoint. |
| Known gaps | Graphical acceptance deferred by owner; world-goal journal replay asserted through `goal_applied` plus reopen, not a separate replay test; the demobilisation kill test uses a configured loot table, not a vanilla capture; the guest fix re-authorized the component hash without re-running graphical Loader acceptance. |

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
validated_tree:
  head: 87af4502a79864392798bcb52634e6d2d68ce9d4
  tracked_diff_sha256: bcd122e28fdd928c8eedaf6ebd135710fbc7f08e5b64be44db0f47dcedee01a0  # sha256 of `git diff HEAD` (tracked modifications only)
  tree_manifest_sha256: 642f02b2e1287f6122963973d1c7b3403332f38761c565432392550af8c164ef  # sha256 over sorted `<sha256>  <path>` lines for all 156 dirty paths, tracked and untracked, target/ excluded
  untracked_inputs: 5 paths, including this checkpoint's crates/mc-net/src/play/session/resident_damage_tests.rs and crates/mc-net/src/script/storage/resident_morale.rs (the other three belong to earlier checkpoints of the same route)
  guest_src_manifest_sha256: 036e107538297b133a3f118f031a3b43874fcfebd6ab677d50d32edf76da3e18  # 14 sources/solaris-settlements/src/*.rs
  deployed_component_sha256: a32bb32bf0aa87a2226ff1364fc27f39d53a50d7b746dfefb00ce4dc428c7d74
  note: CP-047 has no commit boundary (HEAD predates the whole uncommitted route; the guest Rust sources are untracked in the sibling), so per-slice attribution is not mechanically recoverable and the fingerprints cover the whole validated tree. This MEMORY.md edit is docs-only after the gates.
changed_files:  # CP-047's own edits per its receipts and work record, not a mechanical boundary
  - crates/mc-net/src/script/storage/{resident_morale.rs (new),resident_orders.rs,resident_order_execution.rs,resident_order_tests.rs,resident_settlement_tests.rs}
  - crates/mc-net/src/play/session/{resident_orders.rs,resident_damage_tests.rs (new),entity_combat.rs,session.rs}
  - crates/mc-net/src/play/{world_journal.rs,world_inventory_journal.rs,world_inventory_journal_tests.rs,simulation.rs,simulation/regional_mutation.rs,simulation/tests.rs}
  - crates/mc-script/src/resident_order_operations.rs
  - crates/mc-script/wit/residents.wit
  - docs/decisions/0009-regional-plugin-boundary.md
  - docs/MEMORY.md
  - ../solaris-default-plugins/sources/solaris-settlements/src/{combat.rs,capture_runtime.rs,capture_tests.rs,demob_runtime.rs,demob_tests.rs,lib.rs}
  - ../solaris-default-plugins/solaris-settlements/plugin.wasm
validation:
  - python3 -m tools.harness run correctness: 20260925T053848-correctness-ho0j9n8o passed; workspace tests 90 suites, 5165 passed, 0 failed, 192 ignored
  - python3 -m tools.harness run settlement-cargo: 20260925T060652-settlement-cargo-o8o4ljqj passed; component hash match; 7/1/27/75/70/2 passed
status: CP-047 headless scope complete with review corrections; graphical acceptance deferred by owner; no commit authorized
next: Begin CP-048 - keep resident identities across population growth and housing changes; reconcile the package's demographic rules with the resident model first, then add only the transitions the contract requires.
```

