# Archived checkpoint log — part 16 of 17

Era: 09-23. Chronological checkpoint history moved out of `docs/MEMORY.md` so
the live cursor stays small. **Not startup context** (see `AGENTS.md`); read it when
a question is about this era, not to learn the current state.

## Sections in this part

- CP-030 shared inventory kernel across client and real WIT guest (2026-09-23)
- CP-031 owned-inventory request outcome after reconnect (2026-09-23)
- CP-032 stable WIT inventory handles and bounded observations (2026-09-23)
- CP-033 induced-work and healthy-player limits (2026-09-23)
- CP-035 plugin reload generation fence (2026-09-23)
- CP-036 durable first-party payment (2026-09-23)
- CP-037 canonical custom item lifecycle (2026-09-23)
- CP-038 Loader adapter verification (non-graphical, 2026-09-23)
- CP-039/040 actionable settlement view and input (headless, 2026-09-23)
- CP-041/042 preview-bound construction and bounded sound (headless, 2026-09-23)
- CP-043 dual independent component acceptance (headless, 2026-09-23)
- CP-044 real resident hiring and paid equipment (headless, 2026-09-23)
- CP-045 native resident melee and physical ranged combat (headless, 2026-09-23)
- CP-046 bounded squad orders and reformation (headless, 2026-09-23)

---

## CP-030 shared inventory kernel across client and real WIT guest (2026-09-23)

The existing owned-inventory operation is the selected ordinary mechanic.
Its Rust guest uses the SDK's `transfer_owned_items` and `query_owned_inventory`
through `inventories.wit`, the single host `domain_inventories::decode`,
`to_script_batch`, the attested `ScriptBoundary` and storage operation route
to the native session inventory owner. A direct unattested privileged
`ScriptCommand::Operation` is rejected by `ScriptRouter`.
No planner, DTO converter, action bus or gameplay authority was added.

The real-component TCP scenario now also swaps a component-bearing tool from
hotbar slot 36 to 37 and back via ordinary `ServerboundContainerClick` packets.
On the identical starting inventory, the real Rust guest then reads its
fresh owner fence through WIT and transfers the same tool to slot 37. The
authoritative player inventory frames show identical slot/component outcomes
and conserve unrelated emeralds. The same component scenario checks durable
replay without reapplication, conflicting operation id, stale revision,
foreign-player refusal, missing owner, and saved tool components. Its separate
grant test refuses an ungranted inventory command by capability name.
Both tests passed. The existing native `before-build` cases also passed for
a cancelled programmatic owner edit and changed protected-zone snapshot; the
selected inventory transfer has **no** `before-build` hook. Source inspection
shows `request_build_hook` returns before queue/await with no registered hook;
this was not a new runtime zero-subscriber benchmark.

This is a headless real server, real component and real TCP client, not
graphical vanilla-client acceptance. CP-029 remains owner-deferred; it is not
substituted here. Maturity `draft`, no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 252b851988effe9688be88f05087f3f2965a9fffd17820f68b602cc19d65e4b0
changed_files:
  - crates/mc-test-harness/tests/wasm_owned_inventory.rs
  - docs/MEMORY.md
validation:
  - mc-test-harness --test wasm_owned_inventory: 2 passed
  - mc-net changed_zone_snapshot_refuses_kept_placement_without_mutation: 1 passed
  - mc-net cancelled_server_owned_edit_cannot_bypass_before_build: 1 passed
  - fmt: 20260923T080147-fmt-zokojtsa passed
  - code-health: 20260923T080147-code-health-wq26p0wb passed
  - graphical-same-mechanic-client: deferred_by_owner
next: Verify CP-031 request result lifecycle on the selected owned-inventory WIT operation.
```

## CP-031 owned-inventory request outcome after reconnect (2026-09-23)

The selected real Rust guest issues owned-inventory requests through the
existing bounded host/boundary queue and receives correlated answers in a
later delivery phase. Its new `status` command explicitly declares and is
granted `storage_batches`, as the existing operation-status contract requires;
`inventory_transfers` remains independently granted. After the original
player leaves and the same identity rejoins with a new session, the guest
queries the stored transfer's operation id without resubmitting its payload.
The result names the old actor and original committed revision, while the new
session's component-bearing tool remains unchanged. A subsequent fresh
request id replays the same durable transfer without another item move.

This scenario initially reproduced a **guest fixture** bug: replay compared
the old-session receipt to the current session and reported an unexpected
answer. The fixture now checks the recorded transfer actor for replay and
status and checks every answer's request id. Pre-fix command/output and
post-fix receipt: `.analysis/codex-logs/cp031-replay-session/repro.txt`.
No second request runtime, callback retry, or server commit policy was added.

A separate real-component TCP reproduction now holds the **committed** move
answer at the guest host log boundary, replaces the old connection with the
same stable player identity, then releases callback output. Before the fix,
the old request's chat report reached the new session because the guest sent
to stable identity and the host re-resolved it at admission. The guest now
targets the invoking **session** for every action report; the new client sees
the committed tool in slot 37 and can query the durable receipt, but receives
no old move report. Exact failing/passing repro:
`.analysis/codex-logs/cp031-late-callback/repro.txt`. This closes the
read-only reviewer's late-disconnect coverage finding without inventing
callback durability.

The pre-existing owner crash test passed after process termination before
callback delivery at its three commit boundaries: the recovered durable
receipt and item effect remain exactly once. Existing required outbox tests
passed overflow, receiver drop with backlog, closed enqueue, and capacity
release; host tests passed bounded full-command refusal without retiring the
guest, refused delivery without committing a timer, targeted result routing,
and compatible generation replacement. These are inherited
boundary/generation checks, **not** a forced same-transfer in-flight reload
or a durable in-memory callback guarantee. Graphical client acceptance
remains deferred by the owner until after CP-096. Maturity `draft`, no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: 8a14053608b3e6350fca8a95414c7716fe2973bbb25b57ae3937747f96c27a20
changed_files:
  - sdk/rust/examples/hello/src/inventory_ops.rs
  - crates/mc-test-harness/tests/wasm_owned_inventory.rs
  - docs/MEMORY.md
validation:
  - mc-test-harness --test wasm_owned_inventory: 3 passed
  - mc-net owned_transfer_recovers_exactly_once_across_a_process_crash: 1 passed
  - mc-script commit_events::tests: 5 passed
  - mc-plugin-host host_runtime full-command queue and targeted result: 2 passed
  - mc-plugin-host timer_operations refused delivery: 1 passed
  - mc-plugin-host host_deployment compatible reload: 1 passed
  - sdk/rust cargo fmt --check: passed
  - fmt: 20260923T082954-fmt-3uhmtztw passed
  - code-health: 20260923T082957-code-health-1fninoab passed
  - graphical-owned-inventory-client: deferred_by_owner
next: Verify CP-032 stable handles, stale identity, bounded WIT observations.
```

## CP-032 stable WIT inventory handles and bounded observations (2026-09-23)

The real owned-inventory component now reads the **old numeric player
session** after the same identity has rejoined: the owner returns typed
`not-found`, while `operation-status` can still return the old committed
receipt and the new session retains the component-bearing tool. The same
check passes when the old move's callback was deliberately held until after
the reconnect. This separates stable player identity, exact session, and
object revision; the existing native resident suite confirms durable
owner-scoped handles keep their UUID after storage/session reopen, report
`alive-unloaded` without a live pose, and refuse foreign, unknown and stale
revisions. The host's compatible-generation reload test and Loader's pending
HUD-open disconnect test cover generation/session isolation. No ECS or
network entity id was added to WIT inventory endpoints.

Settlement `list-sites` already pages by a bounded live cell cursor. A new
regression adds a generated village after the first page: continuation
advances without repeating old sites; a fresh walk discovers the new
village. It does **not** promise snapshot-isolated pages or revisit a cell
already passed. The selected owned-inventory query instead returns one
bounded canonical endpoint image (at most 54 slots), not a cursor.

Host lift accounting remains the existing `PluginLimits::hostcall_bytes`
(8 MiB per callback): Wasmtime charges declared list strides and string
lengths before copying, including repeated/aliased guest pointers; the six
`host_bounds` cases passed. `domain_inventories::decode` moves lifted strings
into the native DTO and rebuilds bounded record vectors, with DTO validation
before an owner sees them (16 transfers, at most 32 expected endpoints,
4,096 moved items). This identifies one bounded guest-to-host lift and
additional bounded native record allocations; it is **not** a measurement of
all downstream host allocations. Owned-inventory input has no floating
fields; the existing WIT player teleport test rejects NaN, Infinity and
oversized values. A wrapped 64-bit session counter or artificial ECS-id
recycling was not forced: the selected WIT endpoint does not expose ECS ids.
Graphical acceptance remains deferred until after CP-096. Maturity `draft`,
no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: efc4f7698016538ec9c60e5aae7d0a43e2b20831eedb58254f03fa5b5b1a43ae
changed_files:
  - sdk/rust/examples/hello/src/inventory_ops.rs
  - crates/mc-test-harness/tests/wasm_owned_inventory.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - docs/MEMORY.md
validation:
  - mc-test-harness --test wasm_owned_inventory: 3 passed
  - mc-net script::storage::resident_tests: 8 passed
  - mc-net list_sites_cursor_stays_bounded_when_the_roster_gains_a_village: 1 passed
  - mc-net list_and_query_sites_are_deterministic: 1 passed
  - mc-plugin-host --test host_bounds: 6 passed
  - mc-plugin-host player_operations nonfinite-bound refusal: 1 passed
  - mc-plugin-host loader_operations pending disconnect: 1 passed
  - mc-script inventory_operations_tests: 1 passed
  - sdk/rust cargo fmt --check: passed
  - fmt: 20260923T084143-fmt-4lr9m1__ passed
  - code-health: 20260923T084148-code-health-zozqi_46 passed
  - graphical-client-handle-acceptance: deferred_by_owner
next: Verify CP-033 induced work and fairness limits for the selected WIT consumer.
```

## CP-033 induced-work and healthy-player limits (2026-09-23)

The selected owned-inventory WIT operation remains one fenced endpoint read or
bounded transfer, not a scan. Existing P0/P2/P3 limits separately constrain
guest fuel (50 million/call), epoch deadline (four ticks), guest linear memory
(64 MiB per memory), aggregate guest-to-host lift (8 MiB/callback), native
inventory DTOs (16 transfers/32 fences/4,096 moved items), event admission
(default 1,024; configurable), and the command queue (default 256). Previously
passed focused real-component cases cover an infinite guest rejected at
startup, a guest blocked inside a host import terminated by the epoch deadline,
oversized/aliased lift rejection, and full-command-queue refusal without
dropping the live instance. These are different budgets; no fuel increase or
new host-worker pool was needed.

The real TCP/component test now runs two distinct packages and two connected
players. After the inventory owner commits a component-bearing tool move,
its callback reply is held; the other player sends 32 `/hello` commands
to the other component, below the TCP ingress's weighted-work burst limit.
Releasing the owner then answers the healthy player's stale endpoint and durable
status without retargeting the old reply; the tool remains moved exactly once.
An actual reply reaches the flood player, not just a socket write.
The healthy stale-endpoint reply
took 4.377 ms in the focused run; this is one observation, not an SLO.
The TCP ingress's weighted-work allowance is 1,024 and each command costs 24; the
initial 64-command probe correctly disconnected the flooder after three
violations, so the smaller simultaneous-load scenario uses 32. The separate
five-package headless workload admitted 11/64 commands, refused 53, and
observed event depth at most eight with an eight-event bound. Another
source-built five-package run answered 100 sequential `/money` requests
(p50 83.337 µs, p95 109.637 µs, p99 137.168 µs), drained 64 queued
requests in 3.415 ms, had zero economy command refusals, and observed
168,824 KiB process peak RSS. These measurements cover a finite workload,
not a global host-memory ceiling or graphical client latency.

The canonical `standard-pack` profile **failed** its sibling source check:
six existing, untracked `plugin.wasm` files differ from fresh source builds.
The ignored source-built staging root was used only for the two focused
headless integration tests. No sibling artifact was overwritten; the
canonical profile remains failed and must not be described as green.
Graphical client acceptance remains owner-deferred until after CP-096.
Maturity `draft`, no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: c7db0f757fc677f3548a2f5414fbb1173f42f550c3ee7eede3b23cd4848384ab
changed_files:
  - crates/mc-test-harness/tests/wasm_owned_inventory.rs
  - docs/MEMORY.md
validation:
  - mc-test-harness --test wasm_owned_inventory: 3 passed
  - source-built standard-pack bounded flood: 1 passed, 53/64 refused, peak event depth 8
  - source-built standard-pack command workload: 1 passed, 164/164 workload replies
  - standard-pack: 20260923T084315-standard-pack-x0445y9s failed (source/artifact mismatch)
  - fmt: 20260923T085635-fmt-0pyveqe0 passed
  - code-health: 20260923T085635-code-health-878yjrux passed
  - codegraph sync: up to date
  - graphical acceptance: deferred_by_owner
next: Assess CP-034 conditional package expansion; preserve the untracked sibling artifacts.
```

## CP-035 plugin reload generation fence (2026-09-23)

The selected owned-inventory and plugin-storage requests now carry the issuing
registration generation from host command admission through owner completion.
Previously an old accepted request finishing after same-ID reload was stamped
with the replacement's *current* generation at result enqueue and could answer
the replacement Store. The pre-fix regression failed; the typed storage and
owned-inventory result regressions now pass. Candidate init commands also bind
their reserved tickets to the newly committed generation before publication:
otherwise a second reload before server acceptance could deliver the first
candidate's result to the second Store. That regression failed before the fix
and passes after it. Both failures and exact commands are in
`.analysis/codex-logs/cp035-late-result/repro.txt`.

The actual Wasm host regression reloads the same package after an admitted
storage read; the replacement reuses its request ID, but receives only its own
value. Existing deployment tests cover refused candidates preserving old
routes, restart-only contract refusal, combined guest-memory budget, and timer
restart. The source fix does not alter WIT, persisted receipts, or durable owner
effects; an accepted operation remains queryable by durable ID if its callback
is discarded. This is an issuing-command result fence, not a promise about
unsolicited events or startup replay lacking an admitted request. Graphical
acceptance remains deferred by owner until after CP-096. CP-034 remains
conditional/blocked: no concrete expanding first-party package currently
requires a modular SDK refactor; the selected inventory WIT consumer uses the
existing Rust SDK. The pre-existing sibling `standard-pack` source/artifact
mismatch remains failed. Maturity `draft`; no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: f228a76a5b9843e8464f336b6724948b43631cdb99b347e16504d403cf0c88db
changed_files:
  - crates/mc-script/src/lib.rs
  - crates/mc-script/src/host_control_tests.rs
  - crates/mc-plugin-host/tests/host_runtime.rs
  - docs/decisions/0009-regional-plugin-boundary.md
  - docs/MEMORY.md
validation:
  - mc-script: 161 passed
  - mc-plugin-host --test host_deployment: 9 passed
  - mc-plugin-host --test host_runtime late_storage_reply_does_not_answer_the_replacement_guest: 1 passed
  - mc-test-harness --test wasm_owned_inventory: 3 passed
  - fmt: 20260923T102143-fmt-56qhwpkj passed
  - code-health: 20260923T102143-code-health-zob7x8vk passed
  - codegraph sync: up to date
  - graphical acceptance: deferred_by_owner
  - standard-pack: 20260923T084315-standard-pack-x0445y9s failed (unchanged sibling artifacts)
next: Select one real first-party CP-036 durable service consumer, then close its restart and CAS recovery path.
```

## CP-036 durable first-party payment (2026-09-23)

The source-built `solaris-economy` Rust component now uses the existing
`StorageBatchCas` and durable `OperationStatus` for `/pay`. One owner operation
CASes the existing `ledger-v1` value, including debit, credit, and bounded
token record; the operation ID derives from sender UUID and transfer token.
The guest checks the receipt before issuing a payment and reloads the ledger
after an unconfirmed result. No second ledger, WIT extension, post-commit
business CAS, or arbitrary retention TTL was introduced. Old `ledger-v1`
values still decode; pre-migration tokens have only their old bounded ledger
window, not retroactive receipts. The new `storage_batches` grant and required
feature are declared by the sibling package.

The **same real-server/TCP regression** failed against the old source-built
guest: after a 7-coin payment, a 1-coin payment evicted the first token; a
same-package reload then paid another 7 instead of refusing the replay.
It passes with the rebuilt guest. Two players observed exact balances
100/100 -> 92/108; duplicate and different-payload reuse after reload and
world/server restart left those balances unchanged. The separate typed
component test loads a pre-existing version-4 ledger with a legacy token,
simulates a lost callback after the named batch, and observes one payment and
no second batch on reinitialization. This last test injects the ledger image,
not an interrupted live owner commit. Existing native owner tests cover
same-ID/different-payload conflict, durable status after compaction/restart,
DurabilityUnknown recovery, and bounded scan/cursor. This payment has **no
post-commit CAS**, so a post-commit business CAS-conflict scenario does not
exist. Commands, old/new component hashes, and assertions are recorded at
`.analysis/codex-logs/cp036-economy/repro.txt`.

The sibling's existing untracked `plugin.wasm` was preserved; focused tests
use an ignored source-built staging directory. Canonical `standard-pack`
remains **failed** on the prior six untracked source/artifact mismatches.
Strict Wasm Clippy passed for the sibling source. A focused root strict
Clippy attempt found one CP-035 helper naming warning (fixed with LSP);
the rerun still fails on three existing `mc-net` argument-count/type-complexity
warnings. It is **not** a Clippy pass; resolve at the mandatory L2 phase gate.
Graphical acceptance remains deferred by owner until after CP-096. Maturity
`draft`; no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
sibling_base_tree: 92c51a37dee282e75b8fce709b8e720509f4f43f
diff_hash: c67f72dac0fa99bdfaed23bfa6a272c3d300a4f392e8944309e288db45c26ea6
changed_files:
  - crates/mc-script/src/lib.rs
  - crates/mc-test-harness/tests/plugin_standard_pack.rs
  - crates/mc-test-harness/tests/wasm_economy_payment.rs
  - ../solaris-default-plugins/sources/solaris-economy/src/lib.rs
  - ../solaris-default-plugins/solaris-economy/plugin.toml
  - ../solaris-default-plugins/solaris-economy/README.md
  - docs/MEMORY.md
validation:
  - real TCP/world payment old component: 1 failed (second 7-coin debit after token eviction/reload)
  - real TCP/world payment rebuilt component: 1 passed (exact balances; reload; restart)
  - typed guest legacy-ledger/lost-callback: 1 passed
  - mc-net script::storage::operations_tests: 3 passed
  - guest wasm32 release clippy -D warnings: passed
  - root focused clippy -D warnings: failed (three unrelated mc-net warnings)
  - fmt: 20260923T114852-fmt-ji73auk_ passed
  - code-health: 20260923T114852-code-health-jiwtxjp3 passed
  - codegraph sync: up to date
  - standard-pack: 20260923T084315-standard-pack-x0445y9s failed (unchanged sibling artifacts)
  - graphical acceptance: deferred_by_owner
next: Wire one real canonical custom-item definition through native lifecycle and Loader presentation; keep graphical acceptance deferred until after CP-096.
```

## CP-037 canonical custom item lifecycle (2026-09-23)

The guest `configure` contribution now defines bounded owner items; startup
requires an exact verified Loader item declaration and `register_items`.
Native `ItemFactsTable` resolves canonical `item_model` identities on vanilla
carrier stacks. Recipe ingredients/results, stack limits, equipment,
player attack damage and wear use those registered facts. The source-built
Ruby/Sapphire Loader fixture declares craftable paper-carried Ruby material
and a Ruby-carried blade; the byte-for-byte fixture rebuild check passed.
World startup records a deterministic item catalog identity and refuses
changed/removed definitions on reopen. ADR-0010 records this owner boundary.

Two real Loader-handshake TCP clients crafted the material and blade; the
second observed its canonical dropped entity model, walked into pickup range,
and received that same model. After graceful shutdown and restart of the same
world, its initial inventory still contained the blade. Native focused tests
cover split/merge, equipment, damage/wear, chest transfer versus vanilla
carrier, chest NBT flush/reopen, malformed item contribution, missing Loader
permission/artifact mismatch, and changed-world catalog refusal. Raw outputs
and exact commands are in `.analysis/codex-logs/cp037-custom-item/repro.txt`.
The hardcoded-port TCP smoke was removed after proof. **Graphical client
acceptance remains deferred by owner until after CP-096**; this is not a
Fabric/NeoForge/Forge graphical pass. Maturity `draft`; no commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
diff_hash: a3a8c9bc486fa9d9a38ac91bb9a7fb550465a3c84ec945567275852185a13d48
diff_scope: CP037 source paths against base, including earlier uncommitted checkpoints; untracked files listed separately
changed_files:
  - crates/mc-data/src/{item_components.rs,item_semantics_26_1_2.rs,recipes.rs}
  - crates/mc-data/tests/custom_item_recipe.rs
  - crates/mc-net/src/{loader.rs,play/containers/chest/component_tests.rs,play/containers/crafting.rs,play/combat/player_actions.rs,play/inventory.rs,play/recipes.rs,play/tests/held_sharpness_damage.rs,script/storage/resident_order_execution.rs}
  - crates/mc-plugin-host/{Cargo.toml,src/check.rs,src/lib.rs,src/startup.rs,tests/startup_contribution.rs}
  - crates/mc-script/wit/lifecycle.wit
  - crates/mc-server/{Cargo.toml,src/main.rs,src/startup/check.rs,src/startup/components.rs,src/startup_rules_tests.rs,src/startup_validation.rs}
  - sdk/rust/examples/hello/src/lib.rs
  - examples/loader-live-gate/plugins/{ruby-live,sapphire-live}
  - tools/build-loader-live-gate-fixture.sh
  - docs/decisions/0010-solaris-loader-contract.md
  - docs/MEMORY.md
validation:
  - real TCP craft/drop/peer pickup/restart: passed; see receipt
  - mc-data custom recipe: 1 passed
  - mc-net weapon, recipe book, chest and Loader artifact: 4 passed
  - mc-plugin-host item-only contribution: 1 passed
  - mc-server world contract: 1 passed
  - mc-world chest world flush/reopen: 1 passed
  - loader fixture deterministic --check: passed
  - fmt: 20260923T135257-fmt-us4tor76 passed
  - code-health: 20260923T135302-code-health-fimmwzo8 passed
  - codegraph sync: up to date
  - graphical acceptance: deferred_by_owner
next: Verify CP038 server-only and rejected Loader handshakes plus three-adapter shared-core schema/activation without graphical acceptance until after CP096.
```

## CP-038 Loader adapter verification (non-graphical, 2026-09-23)

Server wire **3**, archive schema **2**, exact owner permissions and bundle
hashes remain unchanged. The existing shipped-archive Java test had a false
manifest: it requested `sounds`/`play_sounds` although neither fixture declares
sounds. It failed when the changed Ruby archive forced a fresh run. The
corrected test now activates the exact Ruby/Sapphire archives, including
`ruby-live:blade`, through the shared core on Fabric, NeoForge and Forge with
the same block/item/asset identities. Gradle tracks both external fixture ZIPs
as test inputs so a changed archive cannot hide behind `UP-TO-DATE`.
Missing assets now fail explicitly as well as corrupted asset bytes. The
sibling README distinguishes pre-freeze native carrier registration (installing
an adapter needs restart) from per-connection verified pack activation.

Four Java suites executed, not just Gradle cache hits. Shared tests covered
consent denial/permission changes, bad transfer hashes and staging cleanup,
unknown/old schema, exact assets, connection generation cleanup and adapter
transport ACKs. Rust configuration refusal and disconnect reason tests passed;
an embedded vanilla TCP client entered Play on a server with no Loader
manifest. Full graphical `loader-live` for each platform **remains blocked by
owner deferral until after CP-096**. Shared-core activation and Java adapter
unit tests are not a real client run; CP-038 is not accepted. Exact receipts:
`.analysis/codex-logs/cp038-loader-verify/repro.txt`. No commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
sibling_base_tree: 0972926e4431ee597cb4903abc43e96872acd732
sibling_diff_hash: 1d31fe1f2a66ceae7dca0ca258e69914c4fcbefc1ca4725ad160ea1ef3a5ee03
changed_files:
  - ../solaris-loader/loader-core/build.gradle.kts
  - ../solaris-loader/loader-core/src/test/java/dev/solaris/loader/LoaderLiveGateFixtureTest.java
  - ../solaris-loader/loader-core/src/test/java/dev/solaris/loader/LoaderContentArchiveTest.java
  - ../solaris-loader/README.md
  - docs/MEMORY.md
validation:
  - four Java test suites with --rerun-tasks: 21 executed; passed
  - updated Java missing-asset case: passed
  - mc-net configuration Loader ACK tests: 3 passed
  - mc-net explicit refusal/reason tests: 2 passed
  - vanilla embedded TCP Play without Loader: 1 passed
  - graphical three-platform loader-live: deferred_by_owner
next: Implement CP039 bounded actionable views and non-graphical owner tests; retain graphical CP038/039 acceptance for after CP096.
```

## CP-039/040 actionable settlement view and input (headless, 2026-09-23)

The first-party settlements Wasm package now serves a bounded schema-2
overview with paged residents, buildings and warehouse stock, exact-session
player identity, and fresh registry authority checks on funding. Role
revocation, disconnect and delayed view-open acknowledgments close or refuse
the former view; an explicit key/chat request reopens it. The shipped client
archive declares four namespace-qualified actions; the source-built Wasm and
archive are verified together by a strict-deployment integration test.
The server suppresses duplicate or regressed view-action sequences within a
revision and resets that counter on each new revision. The shared Loader input
does not dispatch a HUD press when the same physical key edge is bound to
vanilla attack/use; release remains routed for held-state cleanup. The
26.1.2 KeyEvent/mapping API was checked against the actual client bytecode.

Exact commands, counts and artifact hashes:
`.analysis/codex-logs/cp039-settlement-view/repro.txt`.
First-party view tests passed 4/4, shipped Wasm integration passed 2/2,
native view-owner tests passed 6/6; the final archive was parsed by all three
Java adapters, and all three adapter test tasks passed. Root `fmt` and
`code-health` profiles passed; CodeGraph synced. Independent read-only review
findings were fixed and rechecked. **Neither checkpoint has graphical
acceptance**: the owner deferred real-client checks until after CP-096.
Both checkpoints remain blocked for that gate. No commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
sibling_plugin_base_tree: 92c51a37dee282e75b8fce709b8e720509f4f43f
sibling_loader_base_tree: 0972926e4431ee597cb4903abc43e96872acd732
diff_hash: d78f057a3b3d419a78dfeaa94e50468a5f273b91fa5f07b439d90be74215fb9d
sibling_plugin_manifest_diff_hash: fe14e150b1a01625b05d1776a90050d3cce56affec994e699a7db97e784057d5
sibling_loader_view_diff_hash: e2a0584e51a6159b01c73d22760ed1660881b1c109187677ffded56f6b821358
changed_files:
  - ../solaris-default-plugins/sources/solaris-settlements/src/{lib.rs,view.rs,view_tests.rs}
  - ../solaris-default-plugins/solaris-settlements/{plugin.toml,plugin.wasm,client-src/solaris-client.json,client/settlements.zip}
  - ../solaris-default-plugins/tools/build_settlements_client.py
  - ../solaris-loader/loader-platform-common/src/main/java/dev/solaris/loader/minecraft/{LoaderMinecraftInput.java,LoaderViewScreen.java,LoaderHud.java}
  - crates/mc-net/src/play/session/{loader_views.rs,loader_views_tests.rs}
  - crates/mc-test-harness/tests/settlement_component_haul.rs
  - docs/{AGENT_TOOLING.md,decisions/0010-solaris-loader-contract.md,MEMORY.md}
validation:
  - first-party view tests: 4 passed
  - shipped Wasm strict deployment integration: 2 passed
  - native view owner tests: 6 passed
  - final archive LoaderCore three-platform parser: passed
  - all three Loader adapter Java test tasks: passed
  - fmt: 20260923T145548-fmt-o35kvh1n passed
  - code-health: 20260923T145548-code-health-u04d6ofo passed
  - graphical CP039/040: deferred_by_owner
next: Bind CP041 preview/selection to an authorized committed structure operation; queue graphical acceptance after CP096.
```

## CP-041/042 preview-bound construction and bounded sound (headless, 2026-09-23)

The shipped settlements component now begins a server-owned selection from
its view, presents one of four authored palisade-gate quarter turns, and
forwards the exact preview anchor, rotation, live player/view continuation and
catalog SHA-256 through the existing survey/site/operation pipeline. A changed
catalog refuses preparation before a durable structure reservation; chat
projection without a preview remains available. Server ownership still checks
surveyed/loaded footprint, revision and foreign claims. The shared Loader
refuses undeclared preview references, renders bounded schematic layers, and
retains select/cancel controls for markers that do not declare a schematic.
The first-party construction sound is an original 0.17-second verified Vorbis
clip emitted only for a committed portion with actual placed blocks. Client
preflight checks the full decoded clip; playback permits eight distinct starts
per game tick, at most one instance per sound, and origin-scoped disconnect
cleanup. No guest GPU/native execution or generalized particle renderer.

The independent read-only reviewer found a late-disconnect reservation leak,
missing no-preview marker controls, and a preview hash mistakenly applied to
plain chat. All three were corrected without a second review. Exact commands,
failures and artifact hashes:
`.analysis/codex-logs/cp041-042-preview-effects/repro.txt`.
**Graphical placement and effect acceptance remains deferred by owner until
after CP-096**; neither CP-041 nor CP-042 is accepted. No commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
sibling_loader_base_tree: 0972926e4431ee597cb4903abc43e96872acd732
sibling_plugin_base_tree: 92c51a37dee282e75b8fce709b8e720509f4f43f
diff_hash: 028aaff183f821cabf5fbf5fe7a93b9d5203f291cfdeed335615e7bd7978861c
sibling_loader_diff_hash: 628c5109684a187268fc1f960990a254b3da9a0940a8fc42a6a0e038a2d3cd7b
diff_scope: selected tracked core/Loader boundary files only; excludes untracked package sources and this cursor
changed_files:
  - crates/mc-script/wit/{client-presentation,events,settlements}.wit
  - crates/mc-script/src/settlement_operations.rs
  - crates/mc-plugin-host/src/{adapter,client_presentation,domain_settlements,staging}.rs
  - crates/mc-net/src/script/storage/{settlement,settlement_tests}.rs
  - crates/mc-test-harness/{Cargo.toml,tests/settlement_component_haul.rs}
  - sdk/rust/solaris-plugin-sdk/src/{lib,settlement_ops}.rs
  - sdk/rust/examples/hello/src/settlement_ops.rs
  - ../solaris-loader/loader-core/src/{main/java/dev/solaris/loader/LoaderContentArchive.java,test/java/dev/solaris/loader/LoaderScreenIndexTest.java}
  - ../solaris-loader/loader-platform-common/src/main/java/dev/solaris/loader/minecraft/{LoaderViewScreen,LoaderMinecraftView,LoaderHud,LoaderMinecraftSound}.java
  - ../solaris-default-plugins/{tools/build_settlements_client.py,solaris-settlements/plugin.toml,solaris-settlements/plugin.wasm,solaris-settlements/client-src/,solaris-settlements/client/settlements.zip,sources/solaris-settlements/src/}
  - docs/{MEMORY.md,decisions/0010-solaris-loader-contract.md}
validation:
  - native stale preview hash: 1 passed
  - first-party guest tests: 49 passed
  - shipped Wasm actual host plus quarter-turn catalog: 3 passed
  - three Loader archive activations: passed
  - Fabric client audio decoder, malformed, oversized: passed
  - all three adapter Java compiles: passed
  - guest and preview archive byte-for-byte build checks: passed
  - fmt: 20260923T153708-fmt-uxz6acmp passed
  - code-health: 20260923T153708-code-health-9ijacxxn passed
  - graphical CP041/042: deferred_by_owner
next: Verify CP043 two independent WIT/SDK consumers through headless install, lifecycle, storage and reload; retain graphical acceptance after CP096.
```

## CP-043 dual independent component acceptance (headless, 2026-09-23)

The independent server-only `solaris-audit` Rust component and Loader-backed
`solaris-settlements` Rust component now install together in strict discovery
with distinct grants and one validated Loader bundle. The shipped guests check,
start, consume separate owner-scoped startup storage reads, process a synthetic
committed-block event and emit an admitted Audit CAS request, reload together,
reconstruct the saved Audit record after restart, answer operator status, and
leave Settlements active when Audit is disabled. The host integration supplies
the successful Audit CAS and saved payload from test memory; it does **not**
prove a real block commit, disk/world persistence, Loader execution, or a
graphical client. Core journal reopening and stale-generation replies have
separate focused owner tests. Core `mc-server` checked without compiling siblings.

The CP-041/042 WIT expansion changed the component ABI even for server-only
guests, so all six first-party standard-pack components were rebuilt and
byte-checked against their Rust sources. WIT declarations still route selection
and sound through the already bounded view/Loader owners, and preparation
through the existing native catalog/region/budget gates; this checkpoint
introduced no package-id branch or new native work class. Exact commands,
identities and limitations: `.analysis/codex-logs/cp043-dual-consumer/repro.txt`.
The earlier three-platform Java archive/decoder/compile evidence remains
unchanged; none was repeated. **Real graphical acceptance remains deferred by
owner until after CP-096**; CP-043 is not accepted. No commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
sibling_plugin_base_tree: 92c51a37dee282e75b8fce709b8e720509f4f43f
diff_hash: c148756d7eca0ce4f461e685479237499af4fb44ce3fb4b4f5db19b8016d1035
diff_scope: untracked integration test against /dev/null; sibling component byte identities in receipt
changed_files:
  - crates/mc-test-harness/tests/settlement_component_haul.rs
  - crates/mc-data/src/{item_components.rs,item_semantics_26_1_2.rs,recipes.rs}; crates/mc-script/src/host_control_tests.rs
  - crates/mc-net/src/play/{combat/player_actions.rs,ingress/chat_commands.rs,simulation.rs,recipes.rs,use_item_on_adapter_tests.rs,tests/enchanting_recipe_settlement.rs,tests/inventory_settlement.rs}; crates/mc-test-harness/tests/wasm_owned_inventory.rs (strict-Clippy and repair regression fixes)
  - ../solaris-default-plugins/{solaris-permissions,solaris-essentials,solaris-economy,solaris-towns,solaris-audit,basic-economy}/plugin.wasm
  - docs/MEMORY.md
validation:
  - dual shipped component check/start/action/reload/restart/disable: 1 passed
  - audit saved value recovery and status: 1 passed in same integration
  - first-party standard pack artifact rebuild/check: 6 byte-identical
  - core standalone mc-server check: passed
  - core durable storage restart tests: 2 passed
  - old-generation storage reply isolation: 1 passed
  - enchanted tool repair focused regression: 1 passed after clearing inherited enchantments
  - fmt: 20260923T164446-fmt-e28o8mab passed
  - strict workspace Clippy: 20260923T170659-clippy-db6m8szk passed
  - code-health: 20260923T170753-code-health-7gdu8_em passed
  - full workspace test (bounded 6G scope, one Cargo compiler): 20260923T163804-test-5mjwj48q passed after repair regression fix
  - independent read-only review: pass; no critical defect, synthetic evidence caveat above
  - graphical CP043: deferred_by_owner
next: Implement CP044 real resident hiring and equipment via native owner and durable guest intent; keep graphical acceptance queued after CP096.
```

## CP-044 real resident hiring and paid equipment (headless, 2026-09-23)

`solaris-settlements` now hires an existing core-owned villager under four role
policies (militia, infantry, spearman, archer), with exact real warehouse stock
and an emerald signing wage delivered to resident carry. The guest stores an
owner-scoped CAS hiring intent before the native no-player equipment transfer,
then finalizes role/employer under a second CAS. The same operation id replays
after reload without a second debit; repeat hiring and competing employers are
refused. The native world-journal decision includes physical container image,
resident gear and receipt while preserving UUID and previous work/assignment.
The guest rereads the gear revision after the storage actor resumes paused work
and before publishing the roster fence. Admission consults the live entity owner
and tombstones an observed conversion/death before any stock debit; replay of
an already committed receipt precedes that admission.

Focused native ledger/journal and shipped-component host integration passed;
the latter supplies synthetic storage, resident, inventory and transfer
callbacks, not a real chunk save or graphical client. The native world fixture
models the physical container and reopens core storage; it does not prove a
persisted disk chunk. Full reproduction, independent review findings/fixes,
commands and artifact identities are at
`.analysis/codex-logs/cp044-hiring/repro.txt`. **Owner-deferred graphical
Minecraft acceptance remains queued until after CP-096**; CP-044 is not
accepted. No commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
sibling_plugin_base_tree: 92c51a37dee282e75b8fce709b8e720509f4f43f
diff_hash: 1ff8298098cdf6802180296f9c4c55e5e827e7a01577dbf59201ba0c13eabac4
sibling_diff_hash: b57d2d929611d12a004fd3e60ef2d393b0cc9c29f1f71bbad619a41f379ad2c8
diff_scope: CP044 tracked paths in changed_files except docs/MEMORY.md; full bytes of both untracked core files; sibling README diff plus full source and component bytes
changed_files:
  - crates/mc-net/src/script/storage/{residents.rs,resident_orders.rs,resident_settlement_tests.rs,world_inventory.rs,world_inventory/warehouse_resident.rs}
  - crates/mc-script/src/{inventory_operations.rs,resident_operations.rs}; crates/mc-script/wit/{inventories.wit,residents.wit}
  - crates/mc-plugin-host/src/domain_residents.rs
  - sdk/rust/solaris-plugin-sdk/src/{lib.rs,inventory_ops.rs,resident_ops.rs}
  - crates/mc-test-harness/tests/settlement_component_haul.rs (untracked CP043/CP044 shared fixture)
  - docs/decisions/0009-regional-plugin-boundary.md; docs/MEMORY.md
  - ../solaris-default-plugins/sources/solaris-settlements/src/{lib.rs,hire.rs,hire_runtime.rs,hire_tests.rs,view.rs,view_tests.rs}
  - ../solaris-default-plugins/solaris-settlements/{README.md,plugin.wasm}
validation:
  - native focused pre-fix work-revision reproduction failed as expected; fixed test 1 passed, then conversion and post-conversion replay test 1 passed
  - native resident scope before reviewer changes: 96 passed
  - guest package before reviewer changes: 53 passed; post-resume finalization focused test 1 passed
  - shipped Wasm host integration before reviewer changes: 4 passed; post-resume current-revision flow focused 1 passed
  - mc-script and mc-plugin-host suites before reviewer changes: 301 passed
  - settlements component rebuilt and byte-checked SHA256 729cc6c9dc58d3224bb181dcaa49e6fccb505e0600b4b81ac13fa2a73b64cc45
  - six other first-party standard components rebuilt and byte-checked; hashes in receipt
  - fmt: 20260923T180341-fmt-uz9k0ha7 passed after applying rustfmt
  - code-health: 20260923T180341-code-health-0u6sbjox passed
  - codegraph sync: up to date
  - independent read-only review: two findings corrected with focused regressions; no second review
  - graphical CP044: deferred_by_owner
next: Implement CP045 native melee/ranged orders, cooldown and ammo against real target kernels; defer graphical two-client acceptance until after CP096.
```

## CP-045 native resident melee and physical ranged combat (headless, 2026-09-23)

The first-party Rust/Wasm settlement guest now accepts `defend` from the
settlement owner/steward for a hired resident. It issues one Hold order, takes
one server-issued hostile reference and its policy/expiry fences, then issues
an Attack order with a durable operation counter; it sends no tick damage.
Native owner admission rechecks identity, allowed category, ally policy,
equipment, reach, line of sight and persisted 20-tick cooldown. Melee uses
the entity damage owner. A bow spends one canonical arrow on actual launch;
the server creates one target-restricted world arrow and the ordinary arrow
kernel resolves impact and client-visible health/hurt/despawn later. An
accepted attack is indexed in the storage ledger and continues on pushed
simulation ticks after moving into reach, without changing the guest order
fence. Cancellation, death/invalid refs and empty ammunition stop combat.
Old target-bearing durable receipts decode with inert zero metadata rather
than preventing the world from opening.

Two session outbound observers each received one projectile spawn and one
impact publication in a headless injected-physics fixture; this is not two
graphical Minecraft clients or end-to-end world collision. Native tests cover
bow/ally/LOS/ammo, melee/chase/cooldown, changed weapon, departed target,
reopen/cancel and crowded bystanders. The shipped component host test uses
synthetic owner responses to exercise the real Wasm command/metadata flow.
The projectile spawn and order/ammo journal remain separate, so the narrow
crash window is **not** a cross-journal atomicity proof; CP065 owns durable
projectile outcomes. Full commands, failed pre-fix repros and fixes are in
`.analysis/codex-logs/cp045-combat/repro.txt`. **Owner-deferred graphical
two-client acceptance remains queued until after CP096**; CP045 is not
client-accepted. No commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
sibling_plugin_base_tree: 92c51a37dee282e75b8fce709b8e720509f4f43f
diff_hash: da82eebcf4ccd534c4b19f39ef7891978e6c13c8c2ac844d779f0ec8b803a25c
sibling_diff_hash: bda893dbb74507159d46cb13cdf9564148bdde86a35ca829229dfe3959817e42
diff_scope: CP045 tracked paths below except docs/MEMORY.md, plus full bytes of the untracked shared host test; sibling tracked README/manifest diffs plus full guest combat/lib sources and component bytes
changed_files:
  - crates/mc-net/src/script/storage.rs
  - crates/mc-net/src/script/storage/{resident_order_execution.rs,resident_orders.rs,resident_order_tests.rs}
  - crates/mc-net/src/play/session/{resident_orders.rs,projectiles.rs,projectiles_tests.rs}
  - crates/mc-entity/src/projectile_26_1_2/arrow.rs
  - crates/mc-script/{wit/residents.wit,src/resident_order_operations.rs,src/resident_order_operations_tests.rs}
  - crates/mc-plugin-host/src/domain_residents.rs
  - crates/mc-test-harness/tests/settlement_component_haul.rs (untracked shared fixture)
  - docs/decisions/0009-regional-plugin-boundary.md; docs/MEMORY.md
  - ../solaris-default-plugins/{README.md,solaris-settlements/README.md,solaris-settlements/plugin.toml,solaris-settlements/plugin.wasm}
  - ../solaris-default-plugins/sources/solaris-settlements/src/{combat.rs,combat_tests.rs,lib.rs}
validation:
  - native pre-fix cooldown and chase reproductions failed as expected; fixed distant chase/reopen/cancel focused 1 passed after Applied-state correction; ranged continuation focused 1 passed
  - mc-net resident order scope: 67 passed before final Applied-state assertion; final affected focused test 1 passed
  - two-observer shared projectile kernel focused: 1 passed, injected physics only
  - mc-script and mc-plugin-host suites: 302 passed; old target-bearing receipt migration focused 1 passed
  - sibling settlements Rust guest: 54 passed before final byte rebuild
  - shipped Wasm host integration: 4 of 5 passed with one missing fixture grant, then that one focused test passed after grant; guard flow included in initial 4
  - settlements component rebuilt and byte-checked SHA256 100d95dbf266e03cedb64ec008e43b7789f3b4a050d5303c0713cf90e1afe54f
  - six other standard components rebuilt and byte-checked, hashes in receipt
  - fmt: 20260923T195517-fmt-uuz9vukz passed after rustfmt and Applied-state correction
  - code-health: 20260923T194809-code-health-aptnzb1e passed before one fixture grant and Applied slot restoration; architecture scope unchanged
  - codegraph sync: up to date
  - independent read-only review: two priority-one findings fixed with focused regressions; later advisor finding on Applied state corrected; no second review
  - graphical CP045: deferred_by_owner
next: Implement CP046 group roster order, bounded slot planning, reformation and path/query evidence; defer graphical acceptance until after CP096.
```

## CP-046 bounded squad orders and reformation (headless, 2026-09-23)

The first-party settlement Wasm guest now accepts `march` and `halt` for one
explicit roster of one to 32 hired aliases with per-member order revisions.
It re-reads owner/steward authority and issues one fenced native batch rather
than per-member movement commands. Native group admission remains atomic; an
unreachable passage or slot gives a per-member `blocked_route` outcome
without teleportation. For each distinct anchor a batch computes its
formation once, then checks routes only for eligible members. A later subset
order replans just those handles; cancellation idles the named members.
The selected list is retained in native admission/receipt and per-member
durable orders, not a new named guest squad record. Accepted Move/Retreat/
Patrol destinations survive pending-admission replay exactly: the pre-fix
reproduction collapsed all members to a shared anchor. Cancellation also
replays `Idle` for committed `order=None` members before acknowledging their
pending admission; the pre-fix crash-window fixture left a stale movement goal.

Headless query counts on real world storage: open 3-member squad 3 standable /
3 route calls, reformed 2-member subset 2 / 2, closed passage 3 / 3,
unplaceable slot 5 / 0; repeating the same operation id adds zero queries.
Clearing the slot and issuing a newly fenced order resumes distinct movement.
The shipped Wasm host test exercises command-to-native translation and
cancellation using synthetic native outcomes; the native fixture separately
executes actual group goals. Migration fencing and death-between-admission
are covered headlessly; **owner-deferred graphical client acceptance remains
queued until after CP096**, not green. Source, both pre-fix failures and exact
gates are in `.analysis/codex-logs/cp046-squad/repro.txt`. No commit.

```yaml
base_tree: 87af4502a79864392798bcb52634e6d2d68ce9d4
sibling_plugin_base_tree: 92c51a37dee282e75b8fce709b8e720509f4f43f
diff_hash: 04452a2bafccaf0ecdca15ee271bfc1d4be90dc8c4bbf71d1643e5e4dbab3147
sibling_diff_hash: 1aeeecd2bb230fd4f961a4ed92b9633d63ff0186306887447a39e4e9b0ddbbad
diff_scope: cumulative CP045+CP046 owned paths against base, excluding this MEMORY cursor; exact path set in receipt
changed_files:
  - crates/mc-net/src/script/storage/{resident_order_execution.rs,resident_orders.rs,resident_order_tests.rs}
  - crates/mc-test-harness/tests/settlement_component_haul.rs (untracked shared fixture)
  - docs/decisions/0009-regional-plugin-boundary.md; docs/MEMORY.md
  - ../solaris-default-plugins/sources/solaris-settlements/src/{combat.rs,combat_tests.rs,lib.rs}
  - ../solaris-default-plugins/solaris-settlements/{README.md,plugin.wasm}
validation:
  - native pre-fix pending Move and Cancel replay failed as expected; corrected resident-order scope 68 passed (artifact://2125)
  - native group migration fence focused 1 passed; first-party guest 55 passed
  - shipped Wasm group move/cancel host integration 1 passed after final byte rebuild
  - shipped component byte-check SHA256 dc424c803b94e78d676773b4998621038b27c44503031a4baa3e2ce4bc32d404
  - fmt: 20260923T212848-fmt-fj9xnmss passed
  - code-health: 20260923T212852-code-health-7_f22u2c passed
  - codegraph sync: up to date
  - independent read-only review: pass on initial CP046 diff; late Cancel replay repair verified by regression and gates
  - graphical CP046: deferred_by_owner
next: Implement CP047 retreat, treatment and demobilization with native health/gear ownership; continue through CP096 before owner-deferred graphical acceptance.
```

