# Archived checkpoint log — part 11 of 17

Era: 09-16/09-17. Chronological checkpoint history moved out of `docs/MEMORY.md` so
the live cursor stays small. **Not startup context** (see `AGENTS.md`); read it when
a question is about this era, not to learn the current state.

## Sections in this part

- Historical checkpoint — WASM audit, zone completion coverage, Loader reconciliation (2026-09-16)
- Actual migration state
- Completed outcome: zone result delivery
- Completed outcome: Loader local/main conflict
- Evidence and boundaries
- Historical checkpoint — P1 contract/runtime separation accepted (2026-09-16)
- Completed outcome
- Evidence
- Remaining plan state
- Historical checkpoint — P3 durable storage compatibility accepted (2026-09-16)
- Completed outcome and evidence
- Preserved boundaries and failure history
- Remaining work and next bounded outcome
- Historical checkpoint — P3 inventory/storage transactions accepted (2026-09-16)
- Completed outcome
- Verification and preserved failures
- Next bounded outcome
- Historical checkpoint — P3 WASM inventory menus accepted (2026-09-16)
- Completed outcome
- Verification and boundaries
- Next bounded outcome
- Historical checkpoint — P4 accepted (2026-09-17)
- Completed outcome and exact evidence
- Boundaries retained
- Immediate next outcome
- Historical checkpoint — P5 blocked, partial source preserved (2026-09-17)
- Concrete external blocker
- Exact source state — not a runnable deliverable
- Resume outcome and evidence

---

## Historical checkpoint — WASM audit, zone completion coverage, Loader reconciliation (2026-09-16)

**Route:** `plugins`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`; owning ADR:
[`0009-regional-plugin-boundary.md`](../decisions/0009-regional-plugin-boundary.md).
Core base: `62ea32a37e740f1559355056bde2558d3a6b5c4b`.
Earlier sections are historical evidence, not the current phase acceptance.

### Actual migration state

| Phase | Current result and remaining acceptance |
| --- | --- |
| P0 | Host, SDK component, pre-instantiation limits and hostile-guest tests exist; the earlier safety evidence is recorded above. This checkpoint does not refresh the whole safety/performance baseline. |
| P1 | **Partial, not accepted.** Default `mc-script` is VM-free, but its optional `lua-runtime` still owns `mlua`/`luaur`; `mc-server` and `mc-net` enable it. `cargo tree -p mc-server -i mlua --depth 3` confirms the dependency through `mc-script`. This does not satisfy the plan's requirement to isolate retained Luau outside the runtime-independent contract crate. |
| P2 | Discovery, grants, bounded host and server composition root are implemented. **Graphical hello/join acceptance remains unrecorded**; the prior blanket “P2 done” was too strong. R0's separately deferred acceptance does not waive P2's gate. |
| P3 | Storage batch CAS/operation status (S1), teleport (S3), and zone commands/results (S2) are implemented. This checkpoint adds S2 runtime regression coverage. Menus, inventory transactions, remaining used resident/settlement operations, config/timers and Loader views remain to migrate. |
| P4 | Basic configure/startup rules are wired; ore profile, settlement plan and Loader/client metadata remain incomplete. |
| P5–P8 | Precommit hooks, component reload, first-party package migration and final Luau removal remain open. |

**Correction to the interrupted-S2 note:** the zone answer mapping and the guest's
zone fixture were already in committed `62ea32a`; only the dedicated regression
coverage was missing. No new zone production implementation was needed or added.
The old session-local `local://p3-operation-matrix.md` is not available here;
do not treat that URI as recoverable evidence.

### Completed outcome: zone result delivery

`crates/mc-plugin-host/tests/zone_operations.rs` now drives real Rust components
through the real host and admission boundary:

- Two plugins use the same zone ids. Applied/refused results, including a refused
  removal, reach only the issuing plugin and preserve the owner's one-bit answer.
- An ungranted zone batch publishes no command and retires its package.
- Closing event admission drains queued results and closes the command channel;
  success never depends on a timed silence check.

The test supplies owner completion events through `ScriptPluginTarget`; it is
not a graphical gameplay run or a combined live-server zone-owner test.
The existing zone-owner suite was exercised separately: capacity refusal,
protection, ownership, membership, no-op removal and shutdown behavior.
Zone entry/exit observations are still absent from WIT.

### Completed outcome: Loader local/main conflict

Sibling `../solaris-loader/main` fast-forwarded from `3aaa926` to existing
`origin/main` `0972926e4431ee597cb4903abc43e96872acd732`, without a new commit,
push, stash or reset. The old local wire-3/schema-2 draft overlapped the upstream
implementation. Its full input remains in
`../solaris-loader/.analysis/loader-reconcile/` (snapshot, patch and trace report).
The distinct local `bridge-core/.../ClientCommandsTest.java` class-name correction
is retained and uncommitted; caches and local configuration were untouched.

The old draft's unused preview projection math was not restored. Its input-mode
code only reselected already-open views; the upstream modal screen owns that
interaction. The trace also found a pre-existing gap on both sides: no production
key binding emits `view_request{settlement|army}`. That remains Loader/P4 work,
not functionality completed by this conflict resolution.

### Evidence and boundaries

Receipts: `.analysis/codex-logs/wasm-zones-loader-20260916/`.

- `cargo test -p mc-plugin-host --test zone_operations`: **2 passed**.
- `cargo test -p mc-net --lib script::zone_tests`: **16 passed**.
- Scoped host Clippy with `-D warnings`: **passed**.
- Harness `fmt`: **passed**, `20260916T120851-fmt-m_t5ubqm`.
- Harness `code-health`: **passed**, `20260916T120855-code-health-vq1tkvaz`.
- Harness `java`: **passed**, `20260916T120050-java-r2vs3sah`; all four Loader
  modules compiled/tested (**95 tests, zero failures/errors/skips**);
  bridge/java-agent tasks were up-to-date.
- Harness `fixture-check`: **passed**, `20260916T120130-fixture-check-jv0bzbpo`.
- CodeGraph sync: completed. Independent/negative-code review found one vacuous
  retirement assertion: closing admission already clears routes. Fixed by queuing
  a second callback and proving only one batch is refused; the test no longer
  uses shutdown-cleared routes as evidence. Final 2-case suite, scoped Clippy and
  file-format check passed after this fix. No production/scope defect was found.
- L2 `correctness` and graphical clients **not run**. The previously recorded
  full-workspace `settlement_pause_repro` red remains unresolved; no overall
  green or completed P1/P2/P3 acceptance is claimed.

The initial large generated test suite was replaced with two focused behavioral
regressions; duplicate DTO-forwarding checks and timed-silence assertions were
removed. No production Rust code or Cargo dependency was changed.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: fbf85c7ea34919bf3b4f69fa6ea66e4f6c66b473dd1a55030acc895f290a3e27
changed_files:
  - crates/mc-plugin-host/tests/zone_operations.rs
  - docs/PLUGINS.md
  - docs/decisions/0009-regional-plugin-boundary.md
  - docs/MEMORY.md
validation: [zones_2_passed, zone_owner_16_passed, host_clippy_passed, fmt_passed, code_health_passed, java_passed, fixture_check_passed]
next: Complete P1 runtime separation so mc-script has no VM dependency, preserve retained Luau behavior outside that boundary, and prove both deployment paths still build and run.
```

`diff_hash` hashes `owned.patch` in the receipt directory: the scoped tracked diff
plus the new zone test, excluding this cursor to avoid a self-referential hash.
Unrelated pre-existing analysis-file changes remain untouched. No new commits
were created. Before further P3 API expansion, close the earliest unmet P1
dependency contract; then obtain P2's actual hello/join client evidence.

## Historical checkpoint — P1 contract/runtime separation accepted (2026-09-16)

**Route:** `plugins`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`; owning ADR:
[`0009-regional-plugin-boundary.md`](../decisions/0009-regional-plugin-boundary.md).
Base: `62ea32a37e740f1559355056bde2558d3a6b5c4b`.
The persistent objective remains the entire plan, not just this checkpoint.
All 105 plan items are tracked; this checkpoint closes P1 only.

### Completed outcome

`mc-script` now owns the runtime-independent boundary, contract and shared
deployment/client/startup metadata. It declares no VM feature or VM dependency.
Retained Luau moved into the existing `mc-plugin-host::legacy_luau` module behind
the migration-only `legacy-luau` feature; no second host crate was created.
`mc-server` selects that feature at composition. Production `mc-net` has no host
or VM dependency; its integration tests select the host as a dev-dependency.
All old runtime callers moved; metadata callers remain on `mc-script`.

Reload still shares the original bounded FIFO with events. A narrow
`ScriptHostInputSender` moves a trusted, opaque host-only payload; no Tokio sender,
guest control API, second mailbox or generic runtime provider was added.
`commit_reload` retains admission checks, route replacement, swap and publication
ordering. Component reload remains unimplemented P6 work.

Metadata fields and discovery checks were preserved. Constructors assemble
trusted already-validated metadata; they do not claim to validate arbitrary
input. WASM client/worldgen metadata is still incomplete. WIT, SDK, gameplay
owners, storage formats and runtime budgets were not changed.

### Evidence

Receipts and exact changed-file list:
`.analysis/codex-logs/wasm-p1-separation-20260916/checkpoint.json`.

- Linux x86_64 Cargo graph, all features and normal/build edges: no Wasmtime,
  `mlua` or `luaur` below `mc-script`; no `mc-net` dependency below the host;
  no production VM dependency below `mc-net`. The server retains Luau through
  `mc-plugin-host`, not `mc-script`. `dependency-boundary.json` records the check.
- The standalone Rust SDK guest built successfully for
  `wasm32-unknown-unknown`; a real component was encoded from that artifact.
  The temporary packager source was removed after the smoke.
- Actual `cargo run --locked --bin mc-server -- --check --config ...` passed
  for isolated WASM and Luau deployments. Neither created its world directory.
  This is deployment/check-mode evidence, not client gameplay.
- Workspace strict Clippy passed:
  `20260916T131448-clippy-21rce3dk`.
- Full `cargo test --workspace --all-targets` passed through the canonical
  harness: **5,034 passed, zero failed, 192 ignored**,
  `20260916T132433-test-fsq02t7w`. This includes **138 `mc-script` tests** and
  **135 retained-Luau host unit tests**, plus component integration tests.
- Standalone default-feature `mc-plugin-host` suite: **63 passed, zero failed
  or ignored**. This separately exercises the real component host without
  workspace feature unification selecting Luau.
- Independent static/negative-code review found one lost runtime regression:
  moving the poisoned-ledger test had retained boundary rejection coverage but
  lost the host's once-only `CommandAdmission` disable diagnostic. Restored the
  host test using public APIs to saturate outstanding admissions, with no public
  test hook. Two real Luau ticks produce one diagnostic, zero enabled plugins,
  and normal shutdown. The restored test passed after the workspace run;
  host all-target Clippy with `legacy-luau` and `-D warnings` also passed.
  No production defect or additional negative-code finding was reported.
- Final formatter passed: `20260916T133457-fmt-i9c9lxs1`.
  Final code-health passed: `20260916T133501-code-health-pyo1569o`.
  CodeGraph sync completed; updated documentation link targets exist.

The initial correctness attempt failed on a leaked edit-tool token and an
unused test-only import; both were fixed before the successful workspace Clippy.
The first workspace test attempt then exposed an incorrect new saturation-fence
assertion against a raw command instead of its admitted request. Fixed via the
real admission API; the exact reproduction and full workspace rerun passed.
These failed receipts are retained, not relabeled green.

The historical load-sensitive `settlement_pause_repro` passed in this complete
workspace run. No timeout or VM wall-clock budget was widened, and this is not
evidence that its load sensitivity was fixed. Ignored/local-parity tests and
graphical clients were not run. No performance or fresh AArch64 claim is made.

### Remaining plan state

P0's earlier safety evidence is inherited, not refreshed. P1 is accepted.
P2 has host/server code and headless coverage, but its required real-client
join/`/hello` acceptance is still unrecorded. P3 has storage batch/status,
teleport and zone commands/results; remaining used operations are open.
P4–P8 and the later CP queue remain open. This is still **draft** work.

The preceding checkpoint's zone regression coverage and Loader reconciliation
are preserved. Loader remains at existing upstream
`0972926e4431ee597cb4903abc43e96872acd732`, with the separate uncommitted test
class-name correction retained. No new Loader changes were made here.
Missing key bindings for `view_request{settlement|army}` remain P4 work.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 6d83507505a17d683ee74cdb90a75b0a4a4d47b6af88e7a2763472691fad03be
changed_files: .analysis/codex-logs/wasm-p1-separation-20260916/checkpoint.json#/changed_files
validation: [workspace_test_passed, workspace_clippy_passed, review_fix_test_passed, host_clippy_passed, default_wasm_host_passed, standalone_sdk_passed, both_cli_checks_passed, fmt_passed, code_health_passed]
next: Close P2 with real graphical vanilla-client join and /hello from external components, strict-startup rejection and no-world check-mode evidence.
```

`diff_hash` covers the saved `owned.patch` against the base, including preserved
prior-checkpoint edits and new files; this cursor is excluded to avoid a
self-referential hash. No commit or push was authorized or performed. Unrelated
dirty analysis files remain untouched.

## Historical checkpoint — P3 durable storage compatibility accepted (2026-09-16)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
P0–P2 remain accepted. This checkpoint closes **storage compatibility**, not
all P3: the full **106-task** plan still has **3 completed / 103 incomplete**,
with P3 in progress. The persistent goal remains active; maturity is **draft**.

### Completed outcome and evidence

- A real Luau deployment wrote a standalone CAS record at revision 1 and a
  durable batch at revision 2. A WASM deployment with the same plugin id read
  both, replayed the legacy operation under a fresh request id, and received
  `operation-conflict` for substituted content.
- The component committed its own batch at revision 3. Repeating it with fresh
  request IDs and reopening the world preserved that revision and value,
  rather than applying another mutation. Both old/new operation receipts
  remained queryable; another plugin saw neither records nor receipts.
- These are the existing storage owners, namespace, journal and receipt paths.
  No kernel, production-host, WIT or public SDK API change was needed. Added
  a real SDK guest fixture and one real-owner integration regression, not
  hand-built result events or journal bytes. The owning ADR is intentionally
  unchanged.
- Actual CLI smoke used **five separate `mc-server` processes**: legacy seed,
  WASM migrate, restart, foreign-owner probe, and final-component restart
  verification after cleanup. Every process exited 0; the logs contain no
  plugin errors/warnings. Legacy seeding used a real protocol client. No new
  graphical or fault-injected power-loss/DurabilityUnknown gate is claimed.
  Evidence: `.analysis/codex-logs/wasm-p3-storage-20260916/smoke.json`.
- Canonical `correctness` passed formatter, strict workspace Clippy,
  code-health and workspace/all-target tests:
  **5056 passed, 0 failed, 192 ignored**.
  Receipt:
  `.analysis/validation/20260916T180535-correctness-h_teuzib/result.json`.
  After a final guest-only cleanup removed response-vector aggregation and
  asserted its existing single-outstanding-response invariant, the SDK format
  check, real-owner regression and actual-server restart replay all passed.
  A subsequent fail-fast Luau fixture-load prerequisite passed root fmt,
  targeted strict Clippy and the real-owner regression again. Production
  host/kernel and public APIs did not change; CodeGraph synced.
- Independent read-only review: **pass, no actionable findings**, including the
  negative-code pass. The narrow final fixture cleanup was self-checked and
  exercised as above; no second reviewer wave.

### Preserved boundaries and failure history

The first strict CLI check failed because the prepared server configuration
omitted operator grants. Explicit `[plugins.grants.<id>]` entries corrected it;
strict discovery stayed enabled. Both denied and granted checks left world
state absent. Component manifests omit the Luau-only `required_features`
field; the operator instructions now show the actual grant shape.

The earlier timer checkpoint's catalog-wire timeout remains unexplained,
**not fixed by this work**. This checkpoint's workspace run passed that test;
the earlier failed receipt remains preserved in the timer checkpoint.

No commit or push was authorized or performed. Inherited P1/P2/timer changes
and prior Loader reconciliation are preserved. No new Loader or sibling source
changes. Temporary smoke-client source was moved into ignored evidence, its
temporary executables removed, and all owned processes/agents closed.

### Remaining work and next bounded outcome

P3 still needs the remaining used inventory, query, entity/world and
resident/settlement/order verticals and their acceptance boundaries. Next:
**WASM inventory/storage purchase/refund transactions through the existing
session/coordinator/storage owners**, preserving typed outcomes, inventory
revision, item-component conservation and existing receipts/outbox/fences,
without a new journal. Current consumer evidence is
`../solaris-default-plugins/basic-economy/main.lua:516`.
Use a real WASM caller and actual-client evidence for inventory-visible effects.

The historical `local://p3-operation-matrix.md` is unavailable; do not chase it
or invent its rows. Start with bounded discovery of that current consumer and
the existing transaction/result/durability path. Storage scan/delete were not
found in `solaris-settlements`; they were not added speculatively.

The queued **TUI dashboard** remains pending: TPS/MSPT/percentiles, CPU/memory,
players/entities/chunks, **worldgen chunks/s, ms/chunk and backlog**, and useful
chunk I/O, persistence and plugin metrics; readable layout and keyboard
navigation. Verify the actual running TUI with active world generation and
mark unavailable metrics explicitly. The storage smoke records an existing
`calls=0` / `events_delivered=8` diagnostic observation: establish counter
semantics before using it as guest-call telemetry.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 7f85bad032f9f63460ae7549fd257acd3464b7be1cfba2ab0e03ab1649c9f628
changed_files: .analysis/codex-logs/wasm-p3-storage-20260916/checkpoint.json#/changed_files
validation: [correctness_passed, final_component_regression_passed, sdk_fmt_passed, actual_server_migration_restart_isolation_passed, strict_grants_checked, independent_review_passed, codegraph_synced]
next: P3 inventory/storage transaction vertical through the existing owners, with real WASM and actual-client acceptance.
```

[Checkpoint evidence](../../.analysis/codex-logs/wasm-p3-storage-20260916/checkpoint.json)
contains commands, snapshots and the complete plan state.
`owned.patch` is this storage delta against saved post-timer working-tree
copies, excluding this self-referential cursor. `lock-baseline.json` records
the derived baseline for the one newly added test dependency edge.
The [timer checkpoint](../../.analysis/codex-logs/wasm-p3-timers-20260916/checkpoint.json)
retains the preceding capability and its inherited evidence.

## Historical checkpoint — P3 inventory/storage transactions accepted (2026-09-16)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
This checkpoint closes the inventory/storage vertical, **not all P3**.
The full 106-task plan remains **3 completed / 103 incomplete**, with P3 in
progress. The persistent goal remains active; maturity is **draft**.

### Completed outcome

- WIT and the Rust SDK expose a session-bound inventory/storage command and
  typed committed/refused answer through the existing capability, DTO validators,
  session gate and durable coordinator. This compound operation has **no durable
  operation-id receipt**: `request` is correlation only and may be reused after
  completion. Uncertain durability produces no result for the current command;
  silence is not a refusal or permission to retry.
- Three real-component regressions cover owner/disk purchase and refund,
  insufficient inventory, stale CAS/session, reconnect, item-component
  conservation, missing capability and malformed whole-batch refusal. A legal
  multi-key transaction reproduces and guards the fixed **per-string versus
  aggregate text-bound** error; the pre-lift whole-answer budget remains intact.
- The canonical graphical regression passed with a real no-Solaris-Loader
  26.1.2 client under Xvfb: two purchases, insufficient-inventory refusal, refund,
  stale-CAS refusal, reconnect, old-session refusal and a successful purchase
  from the new live session. Ledger markers and exact inventory counts agree.
  A pickaxe damaged by actual survival interaction retains its damage and
  observable components. Main visually inspected all six inventory screenshots.
  Setup used explicit operator commands; this is not no-debug survival.
- The actual client exposed native debug stack exhaustion on ordinary
  `select_hotbar_item`/container-click ingress. A GDB trace localized it to the
  large click future; boxing that future in `play.rs` fixes the reproduced path
  without increasing thread stacks or changing inventory authority.
- Harness stdout retains its original **INFO-only** contract; `main.rs` is
  restored to its checkpoint baseline. Only the existing `play session
  unregistered` event is promoted to INFO, so reconnect does not require DEBUG
  capture or the removed save-log string. Unregister does **not** assert
  asynchronous save completion; reconnect and native disk checks provide
  separate persistence evidence. Two mocked reconnect tests that did not defend
  waiting behavior remain removed; the actual INFO-only scenario supplies proof.

### Verification and preserved failures

- Real graphical pass:
  `.analysis/validation/20260916T212003-regression-7bycs290/result.json`.
  Structured phase outcomes, inventory/component observations, adversarial
  refusals and screenshots are indexed by
  `.analysis/codex-logs/wasm-p3-inventory-20260916/graphical-acceptance.json`.
- Final logging revision passed canonical **`run correctness`**:
  `.analysis/validation/20260916T212046-correctness-2g_2o0mw/result.json`.
  Formatter, strict workspace Clippy, code-health and workspace/all-target tests
  passed: **5057 passed, 0 failed, 192 ignored**. Harness-check and SDK checks
  also passed; CodeGraph synced. The final graphical run used `RUST_LOG=info`,
  captured both actual session-release events at INFO and no DEBUG stdout rows.
  The earlier `correctness` attempt stopped on Clippy's nested-if diagnostic;
  an earlier workspace run caught an incorrect test control assertion
  (`Broadcast` versus the real guest's `SendMessage`). Those receipts remain
  failed, not relabeled. The intermediate individual-gate pass and its source
  snapshot are retained as prior-revision evidence.
- Independent read-only review found no proven source defect in the initial
  patch, but its graphical run was **blocked** by the stack overflow. Main
  diagnosed the subsequent failures, made the fixes and ran the final gates.
  The later fixes were not independently re-reviewed; no second reviewer wave.
  Original review and each failed receipt remain in the checkpoint evidence.
- One intermediate run timed out on chunk delivery; its cause is **unresolved**,
  not fixed by this work. Later runs, including the full accepted scenario,
  loaded the chunks. The earlier timer catalog-wire timeout also remains
  recorded, not claimed fixed.
- Graphical observation does not expose arbitrary item components or storage
  revisions; native coverage supplies those checks. This checkpoint does not
  claim a server-process restart, crash-injected or durability-unknown recovery
  test for the compound operation. Existing storage restart evidence is retained.

### Next bounded outcome

**WASM market inventory-menu open/action/close through the existing owners**,
using the current `basic-economy` consumer
(`../solaris-default-plugins/basic-economy/main.lua:306,381,515–516`).
Acceptance needs a real component and actual-client purchase/refund through
the menu, component conservation, and stale-menu/session refusals. Do not add a
second transaction authority or journal. Other used inventory/query,
entity/world and resident/settlement/order verticals still keep P3 open.

The queued **TUI dashboard** remains pending: TPS/MSPT/percentiles, CPU/memory,
players/entities/chunks, **worldgen chunks/s, ms/chunk and backlog**, useful
chunk I/O, persistence and plugin metrics, readable keyboard navigation and
actual-runtime verification. Mark unavailable metrics explicitly; establish
guest-call counter semantics before displaying them.

No commit or push was authorized or performed. Inherited work and Loader
reconciliation remain intact; no sibling source changed. There was no dependency
or lockfile change in this checkpoint. Temporary diagnostic scripts were removed
after archiving their source, and all owned processes/agents are closed.
The owning ADR is intentionally unchanged: existing domain owners, persistence
ordering and transaction policy remain authoritative.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 830d7c417501b119543c6a8cc0ef1b4b88ddbc8175ce64bd3846add2bd46f2bf
changed_files: .analysis/codex-logs/wasm-p3-inventory-20260916/checkpoint.json#/changed_files
validation: [info_only_graphical_regression_passed, real_component_native_passed, correctness_passed, harness_check_passed, sdk_fmt_passed, initial_independent_review_blocked_then_main_fixed_and_verified, codegraph_synced]
next: P3 WASM market menu open/action/close through existing owners, with real-component and actual-client purchase/refund and stale-context refusal evidence.
```

[Checkpoint evidence](../../.analysis/codex-logs/wasm-p3-inventory-20260916/checkpoint.json)
contains commands, source fingerprints, review, failure history and the next
cursor. `owned.patch` is the inventory/storage delta against saved post-storage
working-tree copies, excluding this self-referential cursor.
The [storage checkpoint](../../.analysis/codex-logs/wasm-p3-storage-20260916/checkpoint.json)
retains the previous capability and its inherited evidence.

## Historical checkpoint — P3 WASM inventory menus accepted (2026-09-16)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
This closes the menu open/action/close vertical, **not all P3**. The full plan
remains **3 completed / 103 incomplete**, P3 in progress; the persistent goal
remains active and maturity is **draft**.

### Completed outcome

- WIT/SDK expose session-bound menu open/close and owner-targeted typed clicks.
  Clicks carry the original session separately from the player's UUID.
  Existing capability checks, DTO validators, session/menu owners and compound
  inventory/storage authority remain in charge. Open/close are fire-and-forget;
  request markers are not successful-effect acknowledgements.
- The real component fixture opens a market, buys/refunds through its existing
  ledger transaction, and closes via either a button or command. Native wire
  coverage exercises all four click kinds, exact title/button content, a second
  subscribed component proving owner-only delivery, missing grants, malformed
  batches, stale window/revision/menu/session contexts, reconnect and persisted
  named/enchanted/damaged item conservation.
- Final real 26.1.2 no-Solaris-Loader graphical runs passed for both the new menu
  route and the existing command-only trade route. Menu coverage includes two
  purchases, insufficient-inventory refusal, refund, both close paths, reconnect,
  stale requests against a kept live window and a subsequent successful click.
  Every real click was confirmed; no timeout or bridge error is treated as a
  pass. Main visually reviewed all **25 final UI captures** through an indexed
  contact sheet, plus initial full-frame evidence.
- Main removed speculative tolerance of unconfirmed bridge clicks, scoped the
  observer fixture to its own mode, and changed stale wire probes to the
  opposite trade action so a rejected click cannot be falsely “proved” by an
  indistinguishable accepted purchase.
- Independent read-only review found one fixture defect: a stale-session
  transaction forgot the live menu while sending its close to the dead session.
  Main reproduced the missing close frame despite a real refusal marker, fixed
  closing to use the tracked menu's own session, and reran the exact regression
  successfully. The regression also reopens and trades afterwards. No second
  reviewer wave; the final fix and private test-helper cleanup are Main-verified.

### Verification and boundaries

- Final menu graphical receipt:
  `.analysis/validation/20260916T224734-regression-b6qptme7/result.json`.
- Final existing command-route receipt:
  `.analysis/validation/20260916T224817-regression-ptyy1v9e/result.json`.
- Full **L2 scope passed through named gates**: formatter, strict workspace
  Clippy, code-health and workspace/all-target tests. Test receipt:
  `.analysis/validation/20260916T223647-test-5yw6tdti/result.json` —
  **5060 passed, 0 failed, 192 ignored**. Harness-check and SDK formatting passed;
  CodeGraph was synchronized.
- The initial canonical `correctness` attempt remains **failed** at Clippy's
  nine-argument private click helper. Main removed redundant window claims and
  grouped existing verification inputs without new state or allocation, then
  ran the failed and remaining gates individually. This is not a claim that the
  failed canonical receipt turned green.
- The review reproduction remains **failed** before the fix; its exact-command
  post-fix run passed. Both are preserved in
  `.analysis/codex-logs/wasm-p3-menu-20260916/review.json`.
- GUI setup used explicit operator commands with seed 81, not no-debug survival.
  Native tests, not MCP, cover modified clicks and forged packets. GUI checks
  actual windows, exact counts and exposed tool components; native checks cover
  richer components. Neither menu commands nor the older compound transaction
  gain a durable operation-id receipt, new journal or crash-recovery guarantee.
- The earlier chunk-delivery and timer catalog-wire timeouts remain unresolved,
  not fixed by this checkpoint. Prior blocked owner/manual gameplay observations
  are not downgraded by these gates.

### Next bounded outcome

**WASM zone entry/exit drives market open/close through the existing owners.**
The live consumer is
`../solaris-default-plugins/basic-economy/main.lua:358–381`; it requests the
catalog on entry and clears pending state/closes its menu on exit. WIT zone
mutations exist, but those observations are still missing. Acceptance needs a
real component and actual client crossing zone boundaries, owner-only event
delivery with original sessions, menu effects, and existing dimension/disconnect
behavior. Do not introduce a second zone observer or menu authority.

The queued **TUI dashboard** remains pending: TPS/MSPT/percentiles, CPU/memory,
players/entities/chunks, **worldgen chunks/s, ms/chunk and backlog**, useful
chunk I/O, persistence and plugin metrics, readable keyboard navigation and
actual-runtime verification. Mark unavailable metrics explicitly; establish
guest-call counter semantics before displaying them.

No commit or push was authorized or performed. Inherited work and Loader
reconciliation remain intact; no sibling source was changed. No dependency or
lockfile change was needed. Owned agents are closed; canonical harness runs
completed their process cleanup. The owning ADR is intentionally unchanged:
existing domain owners, persistence ordering and transaction policy are retained.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 19c13dda7da3bf5bbe229d96af072a6cee48563714e478b72c182dce83eee28c
changed_files: .analysis/codex-logs/wasm-p3-menu-20260916/checkpoint.json#/changed_files
validation: [final_menu_graphical_passed, final_command_graphical_passed, real_component_native_passed, l2_named_gates_passed, harness_check_passed, sdk_fmt_passed, independent_finding_reproduced_and_fixed, codegraph_synced]
next: P3 WASM zone entry/exit-driven market open/close, with real-component and actual-client boundary-crossing evidence through existing owners.
```

[Checkpoint evidence](../../.analysis/codex-logs/wasm-p3-menu-20260916/checkpoint.json)
indexes exact commands, receipts, source fingerprints and review. `owned.patch`
is the menu delta against saved post-inventory working-tree copies, excluding
this self-referential cursor; `after/` holds the accepted source snapshot.

## Historical checkpoint — P4 accepted (2026-09-17)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
P0–P4 are accepted: **5 completed / 101 incomplete** in the full TODO.
The persistent goal remains active; the owner's full-plan instruction and
predominantly Task-agent implementation remain in force. Maturity: **draft**.

### Completed outcome and exact evidence

- `configure` runs in a disposable Store; `init` creates runtime state in a
  separate Store. Both frontends share strict feature, client-bundle and
  worldgen metadata parsing. Existing native startup validators and effective
  world identity remain authoritative; changing guest bytes or runtime alone
  does not invalidate the same world.
- Typed WASM view open/present/close, owned sounds and block-item grants use
  existing native owner/session/permission/hash paths. Loader wire **3** and
  artifact schema **2** are unchanged. HUDs coexist without replacing modals;
  input bindings and cleanup stay tied to their original connection.
- Real configure/init refusals were exercised in both `--check` and serving
  paths: world storage remained absent and the intended listener ports remained
  bindable. Receipts live under the phase's `startup-refusal-probes/`.
- Final L2 constituent gates passed: formatter, strict workspace Clippy,
  code-health and workspace/all-target tests. Recorded test summaries:
  **5115 passed, 0 failed, 192 ignored**. The initial `correctness` aggregate
  stopped at Clippy; its failed receipt is retained. After fixing the case-table
  annotation and large startup enum, only the failed/remaining gates were run.
  `final-native-validation.json` indexes these receipts and their exact scope.
- Final Java and harness-check profiles passed. All three real graphical
  Loader platform scenarios passed after the reviewed HUD fix:
  - Fabric: `.analysis/validation/20260917T054239-loader-live-kq5_gytm/result.json`
  - NeoForge: `.analysis/validation/20260917T054452-loader-live-37y73l4_/result.json`
  - Forge: `.analysis/validation/20260917T054713-loader-live-lsfkb8_7/result.json`
- The final no-Loader core-client scenario passed:
  `.analysis/validation/20260917T054914-core-client-_h7zhj10/result.json`.
  A real WASM greeting and `/hello` reply reached the client, its ordinary
  inventory opened, and deliberately invalid `rules.lua` was ignored. The
  existing Luau and required-client rejection scenarios were retained.
- Clients were agent-run through the canonical harness under Xvfb/MCP.
  Main inspected actual HUD/update/reconnect and no-Loader inventory images.
  Bounded adversarial coverage includes invalid input batches, owner-local
  sound stop, HUD isolation and reconnect cleanup.
- Independent read-only review found duplicate HUD opens and a lost hide while
  an open was pending. Both real-component regressions failed on the preserved
  old artifact and passed on the fixed source-built artifact. One outstanding
  open now coalesces the latest model/visibility; refusal and reconnect are
  covered too. No permanent replay override or second reviewer wave remains.
- Negative-code cleanup removed root wiring-only tests and redundant assertions;
  four world-compatibility/conflict/cleanup tests live in the focused
  `component_startup_tests.rs`. Existing architecture/API/Loader/SDK docs were
  updated; 25 relative documentation links resolve. CodeGraph is synchronized.

### Boundaries retained

R0 and full survival acceptance remain deferred; Loader compatibility is not a
survival or release claim. At 854×480 transient vanilla login toasts overlap the
HUD state column; later panels are readable. First-party package migration,
WASM reload and precommit hooks remain later phases. The stop-log `calls=0`
observation was independently confirmed pre-existing, not fixed by P4.
Earlier blocked owner/manual scenarios retain their status.

No new commit, staging or push was authorized or performed. The Loader base
remains the already reconciled `0972926`; inherited unrelated changes were
preserved. P4 writers/reviewer are closed; harness runs completed their owned
process lifecycles. The queued TUI dashboard remains in the full TODO.

### Immediate next outcome

**P5: bounded precommit hooks**, first before-build, then before-damage through
native owner tickets. Acceptance: ordered handlers, cancellation without item
or damage effects, stale/expired/duplicate rejection, queue-wide deadline,
revoked permission and fail-closed protection, plus client-visible latency
and zero-subscriber behavior. No guest execution under world/region locks.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
sibling_base_tree: 0972926e4431ee597cb4903abc43e96872acd732
diff_hash: fad77696185d5cb266825075d3185e33011cd3f0bfb933a99e330ce1bfe99caa
changed_files: .analysis/codex-logs/wasm-p4-20260917/phase-evidence.json#/changed_files
validation: [final_L2_constituents_passed, java_passed, harness_check_passed, fabric_passed, neoforge_passed, forge_passed, no_loader_core_client_passed, review_regressions_failed_before_passed_after, documentation_links_passed, codegraph_synced]
next: Complete P5 bounded before-build and before-damage hooks with native ticket, protection, cancellation and client-latency evidence.
```

[P4 phase evidence](../../.analysis/codex-logs/wasm-p4-20260917/phase-evidence.json)
indexes exact receipts, review resolution, accepted component SHA-256 and
99 owned core/Loader file deltas. The hash covers canonical sorted path and
before/after hashes, excluding this self-referential cursor. `before/` and
`after/` retain exact source/binary snapshots; `phase-review.patch` records the
text delta. P3 evidence remains in
`.analysis/codex-logs/wasm-p3-20260916/phase-evidence.json`.

## Historical checkpoint — P5 blocked, partial source preserved (2026-09-17)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
P0–P4 remain accepted: **5 completed / 101 incomplete**. P5 is **not accepted**;
P6 must not start. The TODO tool auto-promoted P6 when P5 was blocked; that is
not a routing decision. The persistent goal remains active. Maturity: **draft**.

### Concrete external blocker

The configured Task provider, `opencode-go/deepseek-v4.1-flash`, returns
HTTP **401**, `CreditsError`, **Insufficient balance**. Five initial writers
stopped after partial edits without an error diagnostic. Explicit resume
messages did not restart execution. After cancelling those sessions, a fresh
four-task recovery batch failed before work with the same explicit billing
error. All P5 writer/recovery/scout/configuration agents were then closed.

The owner requested roughly 80–85% Task delegation. The next decision is to
restore Task availability (balance or provider/model configuration), or
explicitly authorize Main-only completion instead. Do not silently replace
that execution contract or repeatedly retry the same billing failure.

### Exact source state — not a runnable deliverable

- New precommit WIT exports, SDK glue, fixture controls and nonce-fenced status
  reports are written but unbuilt. Main reduced the fixture report to 31
  records plus its summary to respect the host's 32-command bound.
- Native DTO/ticket/approval/FIFO and package/discovery registration changes
  are partial and unverified. Actual Wasmtime hook callbacks, phase enforcement
  and ordered host dispatch remain unfinished.
- Operator `[[plugins.hooks]]` configuration/check/serve source and focused tests
  are written, not run.
- Build plan fields and native helper/channel wiring are partial. Actual
  admission, current-permission checks, final consume and programmatic/settlement
  no-bypass integration are unfinished.
- Damage ingress changes reference a **not-yet-implemented**
  `session/damage_precommit.rs`; authoritative player/entity/projectile/effect
  continuation and commit migration remain unfinished.
- `tools/harness/precommit.py` and its `core-client` stage are written, **not run**.
  They target ordinary placement/mining, ordered raw damage, cancellation,
  fail-closed behavior after a trap, and a zero-subscriber comparison.
- Current WIT does **not** expose the native `SetWorldBlock`/`DamageEntity`
  commands. No new WIT mutation API was added merely for QA. Existing native
  programmatic producers still require no-bypass coverage.

No P5 build, test, formatter, graphical gate, L2 validation or independent review
passed or was claimed. Do not reuse P4's green receipts as evidence for this tree.
No commit, staging, push or source rollback occurred; inherited changes remain.

### Resume outcome and evidence

After resolving the execution choice, revalidate the partial snapshot and
finish **the full P5 before-build/before-damage outcome**, including ordered
handlers, native conservation/revision/session/permission fences, one-use and
deadline rejection, bounded outstanding tickets, programmatic coverage,
real-client latency/direct-path evidence, L2 and independent review.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: df308a5673c3159bb8d6172a64a74affa3e7b24f62c81e8ee57401fde564256b
changed_files: .analysis/codex-logs/wasm-p5-20260917/partial-source-snapshot.json#/changed_files
validation: [partial_source_snapshot_preserved, P5_runtime_gates_not_run]
next: Resolve Task availability or Main-only authorization, then complete and verify P5 before-build and before-damage without bypassing native owners.
```

[P5 blocker and handoff](../../.analysis/codex-logs/wasm-p5-20260917/blocker.json)
records the exact failure, attempts and unfinished acceptance. The phase directory
holds the shared implementation contract, pre-P5 `before/` snapshots, and exact
29-file `partial/` source snapshots. The delta hash excludes this self-referential
cursor; `source-baseline.json` inventories the preserved starting tree.

