# Archived checkpoint log — part 10 of 17

Era: 09-16. Chronological checkpoint history moved out of `docs/MEMORY.md` so
the live cursor stays small. **Not startup context** (see `AGENTS.md`); read it when
a question is about this era, not to learn the current state.

## Sections in this part

- WASM plugin host, P0 (2026-09-16, uncommitted, new plan)
- Post-closeout: what changed after the L2 pass, and the machine bounds (2026-09-16)
- P0 closeout — the host's own memory bound, hostile-guest evidence, and the baseline (2026-09-16, tree dirty, no commit authorization)
- Composition root: the component host runs in the server (2026-09-16, tree dirty, no commit authorization)

---

## WASM plugin host, P0 (2026-09-16, uncommitted, new plan)

**Task added by the owner** (`/home/user/Загрузки/solaris-wasm-plugin-plan-deepseek.md`):
replace the Luau server-plugin runtime with WebAssembly components on Wasmtime,
behind a versioned WIT contract and a Rust guest SDK, finishing with the removal
of the Luau path (phases P0-P8). Owner also directed: implement everything first,
run the heavy gates once at the end.

**Landed (P0 vertical, executed, not documented-only):**

- `crates/mc-script/wit/` - the single public schema, `solaris:plugin@0.7.0`:
  `types`, `host` (log + own id), `commands` (`send-message` to a player, a
  session or the operator log), `events` (join/left/chat/command/operation
  settled), `lifecycle` (`configure` -> rule plan, `init`, `shutdown`) and the
  `plugin` world. One package statement, one source of truth.
- `crates/mc-plugin-host` - the Wasmtime host: engine with fuel and epoch
  interruption, per-store limits applied *before* instantiation
  (`memory_size`/tables/instances/stack), an independent epoch-ticker thread, a
  bounded `CommandBatch` staging area, `configure`/`init`/`on-events`/`shutdown`
  calls that re-arm the budget per call and retire a trapped instance, and
  `HostError` separating a package that is wrong from a guest that misbehaved.
- `sdk/rust/` - a separate workspace with `solaris-plugin-sdk` (guest bindings
  generated from the same WIT, a `Plugin` trait with defaults, config parsing,
  message helpers, `export_plugin!`) and a real example plugin
  (`examples/hello`) that greets a joining player and answers `/hello`.
- `crates/mc-plugin-host/tests/component_roundtrip.rs` - five cases against the
  bytes of that real component: it loads and answers a join and a command with
  the configured greeting; a raw core module is refused before any guest code
  runs; the artifact bound is enforced before compilation; a guest that never
  returns is stopped by the epoch watchdog as `HostError::Budget`; a guest that
  allocates without bound is stopped by the store's memory limit. All five pass
  (`cargo test -p mc-plugin-host`, 5/5).

**Dependency decision (P0 requires it).** Wasmtime 48 needs rustc 1.95 and the
workspace MSRV is 1.94; the plan says not to bump Rust without established
necessity, so the host pins **wasmtime 36.0.15**, which builds on the pinned
1.94.1 toolchain. Guest target `wasm32-unknown-unknown` installed and used.
AArch64 is not verified here (no such machine) - recorded as unverified rather
than assumed.

**P1 started.** The batch-submission seam is open and runtime-neutral:
`HostCommandAdmission` (public, built from a validated manifest) and
`ScriptHostEndpoint::try_submit_plugin_batch` (public) no longer sit behind
`cfg(any(test, feature = "lua-runtime"))`, together with the admission ledger,
`AdmittedScriptCommand::for_issued_host_plugin`, the manifest's capability
conversion and the `CommandCapabilities` builders it needs. Provenance is
unchanged: the endpoint's ledger is still the only thing that stamps
`ScriptCommand::HostAttached`, a batch that already claims provenance is
rejected, and every command is checked against the admission's capabilities
before one is queued. `cargo check --workspace --all-targets` passes both with
and without `lua-runtime`.

Package fixes found by review after the CP-003 closeout (sibling, uncommitted):
the duplicate `S.valid_resource_id` is gone in favour of the package's existing
`S.resource_id`, and `S.issue_stock_call` now sets `entry.job = "hauling"` before
assigning - the shared work-result path writes `entry.job or entry.detail or
DONE` into `resident.job`, so without it an issue would have cleared the worker's
job label (or written an item id into it).

**P1 metadata extraction.** The runtime-neutral types lost their language
prefix: `PluginPackage`, `PluginDeployment`, `PluginDiscovery`,
`PluginDisableStage`/`PluginDisableDiagnostic`, `PluginReload*`,
`ClientBundle`/`ClientBundleDiscovery`/`ClientContentKind`/`ClientLoader`/
`ClientPermission` and `GameplayRules` (16 symbols, 12 files). Genuinely
Luau-owned names stay: `LuaHost*`, `LuaString`, `LuaScriptRuntime`,
`LuaRuntimeLimits`, `LuaPlugin`. The startup-rule payload types
(`LuaClayRule`, `LuaTreeRule`, `LuaSpawnPlacement`, `LuaWorldgen*`,
`LuaSettlement*`, `LuaBiomeSpawns`) are deliberately left until P4 materializes
the WASM `configure` plan, so they are named once, next to the WIT rule plan
they become. `cargo check --workspace --all-targets` passes after both changes.

**Package layer landed (P2 manifest half).** `crates/mc-plugin-host/src/package.rs`
reads a package directory (`plugin.toml` + `plugin.wasm`) into the repository's
existing `ScriptPluginManifest` and validates it *for the component contract*:
`validate_for(COMPONENT_PLUGIN_API_VERSION)` was added next to `validate()` so the
2 runtimes keep their own versions without loosening either check (Luau packages
keep requesting 0.6.0, and the existing test that refuses 0.7.0 there still
holds). The capability vocabulary moved out of `lua.rs` into the contract
(`ScriptPluginManifest::declare_capability` + `parse_api_version`), so both
runtimes name the same capabilities and an unknown name fails the package. Five
cases in `crates/mc-plugin-host/tests/package_contract.rs` pass: a package loads
with its contract (id, event subscription, capability, command root, artifact
bytes); a Luau `api = "0.6.0"` package is refused here; an unknown capability is
refused rather than ignored; `entry = "../outside.wasm"`/absolute/empty is
refused by the canonical-path check; an artifact past the bound is refused before
compilation.

**Discovery, grants, `--check` and the adapter landed (P2 remainder, minus the
composition-root wiring).** `crates/mc-plugin-host/src/discovery.rs` reads a
deployment directory with the *unchanged* production contract: strict mode
requires every entry to be a package directory (a stray file fails), the
discovered id set must equal `expected` exactly (an empty declaration admits
nothing, a malformed/duplicated expected id is refused), duplicate ids fail in
both modes because two packages of one id would share durable state, and
permissive mode skips an ordinary broken package *with a diagnostic the caller
sees*. Grants are the operator's: with `require_grants`, every capability a
manifest requests must be granted by `[plugins.grants.<id>]` or the package fails
instead of silently running with fewer rights.

`src/check.rs` implements `--check` with no game-side effect: it compiles every
selected component, runs `configure` for the rule plan and `init` against a real
`script_boundary_pair` whose command queue nobody drains - so the opening batch
passes the same admission a live run would give it while nothing is applied - and
never creates a world, writes storage or opens a listener.
`src/adapter.rs` converts a staged batch into `mc_script::CommandBatch`: a session
target becomes `SendChatMessage`, and a *stable player identity* is resolved to
the session that identity holds at admission through a `PlayerSessions` lookup,
so an offline player is refused rather than addressed by a stale runtime id. The
WIT `message-target` lost `server-log`: an operator log line changes nothing in
the game and is the `host.log` import, not an admissible command.

Evidence: 21 focused tests in `crates/mc-plugin-host` pass (component round-trip
5, package contract 5, discovery 7, check + adapter 4), including a check of the
real SDK-built component (`api = 0.7.0`, no rule plan, no opening command) and a
check refusing a package whose `init` never returns (reported as a budget, not a
hang). `cargo check --workspace --all-targets` exits 0.

**Isolation pinned.** `a_trapped_guest_does_not_disturb_a_live_one` hosts two
instances of the same component in one engine and asserts that the spinning one is
retired with a budget error while the other still answers an `on_events` call -
the property the composition root relies on when it hosts a whole deployment
(6 tests in `component_roundtrip.rs` now).

**The host runtime landed (`crates/mc-plugin-host/src/host.rs`).**
`start_deployment(packages, limits, queues, sessions)` builds one engine, one
epoch watchdog, one `script_boundary_pair`, starts every package (compile,
`configure`, `init` through the real admission), registers each manifest's routes
and runs one `mc-plugin-host` thread that owns the endpoint. The loop maps each
`ScriptEvent` to the contract's events, delivers to the instances that want it and
submits what they answer through `to_script_batch` + `try_submit_plugin_batch`.
Decisions taken while writing it, rather than guessed:

- A player command is delivered by *routing* on the declared root, not by
  subscription: `player.command` is not a subscribable name in the contract, and a
  guest must not receive commands it never claimed.
- `player-left` now carries the session only. The server's leave event has no uuid,
  and a host-held session->identity map would be lost by the host's own restart and
  then guess; a plugin correlates a leave with the join it already saw.
- The stable-identity -> session lookup is the *server's* (`PlayerSessions`,
  implemented by the composition root), because sessions are a game-side owner.
  The host never invents a runtime id: an offline player is refused.
- A failed callback retires the instance and unregisters its routes; a package
  whose `init` never returns fails the whole deployment start instead of
  half-running.

Evidence: `tests/host_runtime.rs` (3 cases, all green with the other 22 focused
host tests): a `player.joined` from the real boundary becomes an admitted
`HostAttached { provenance: hello, request: SendChatMessage { player_id: 7,
"Hi there Ada" } }` command, with the counters reporting one delivered event and
one submitted command; a deployment whose second package cannot finish `init` is
refused at start; a package loads through the public path.

**Composition-root surface landed (P2).** `[plugins] runtime = "luau" | "wasm"`
selects the deployment's runtime - one runtime per deployment, and the Luau
loader now refuses a directory configured as `wasm` instead of silently loading
nothing. `[plugins.grants.<id>] capabilities = [...]` carries the operator's
grants; a strict (production) component deployment requires them, an
unrestricted local one does not. `mc-server --check` on a component deployment
runs the host's own `check_deployment` (compile every selected component, then
`configure` and `init` against a boundary nobody drains) and **fails closed when
any package was skipped**, because a check that reports success for a deployment
it could not read end to end is worse than no check. Evidence: `mc-server --test
cli` 43/43, including a new case where a `runtime = "wasm"` deployment holding a
package that asks for the Luau contract version is refused by name.

**Independent review of the host, and its fixes (reviewer `HostReview`).** Verdict
`changes`, four majors and three nits, all folded:

- **The runtime watchdog was dead.** `start_deployment` dropped the `EpochTicker`
  before returning, so no callback deadline could ever elapse for the whole host
  lifetime; `epoch_ticks_per_call` was an inert knob. The host now owns the ticker
  and stops it in `stop()`, and a watchdog that cannot spawn fails the start
  (`EpochTicker::start` returns `io::Result`) instead of running unbounded.
- **Retirement was far too broad.** A guest answering a contract-sanctioned
  `plugin-error`, a batch refused for transient state (full command queue, full
  admission ledger) or a message to a player who disconnected before admission all
  retired the instance *and* unregistered its routes - one "not found" answer from
  one event killed the package. `deliver` now retires only when the call itself
  retired the instance (trap, budget, answer past a bound) or when the *contract*
  refused the batch (forged provenance, invalid DTO, denied capability);
  everything else drops the batch, counts it and keeps serving.
- **The context tick was fabricated**: it counted deliveries rather than the
  server's tick. The loop now tracks `ScriptEventKind::ServerTick` and passes that
  value, with 0 before the first tick.
- **`operation-settled` left the contract**: no guest could ever receive it (no
  event kind mapped to it, and no command carried the `request-id` it needs), so
  it comes back with the phase that adds request ids, per the file's own rule.
- Retirement now records the classified failure, so a trap or a budget is no
  longer reported as an invalid answer.

Evidence: two new host-runtime cases pin the policy with the real component - a
guest that answers `plugin-error` on every event keeps its `/hello` route and is
not counted as delivering commands, and a message to an offline player costs the
batch (`commands_refused == 1`) while the instance stays live; 27 focused host
tests pass; `cargo check --workspace --all-targets` exits 0.

**P3 first slice: durable storage in the contract.** The guest-facing storage
surface is now part of `solaris:plugin@0.7.0` (`crates/mc-script/wit/storage.wit`
plus the two requests in `commands.wit` and their typed answers in `events.wit`),
and the host runs the whole two-phase path on real DTOs: a guest's `storage-get`
becomes `ScriptCommand::PluginStorageGet`, the server's typed result comes back
as `storage-get-answered` correlated by the plugin's own request id, and the
plugin's reaction is what a player sees. Semantics are the server's, not a
paraphrase: `expected-version` absent means "only if the key holds nothing", a
swap that did not commit reports only `refused`, and a failure is
`unavailable`/`durability-failed` - never "the key holds nothing". Two decisions
worth naming:

- **Conversion runs under the package's own grants.** `to_script_batch` now takes
  the capabilities of the instance's admission and pushes through
  `try_push_authorized`, so a command the manifest never declared fails in the
  adapter as the plugin's own bug instead of being silently dropped; the boundary
  checks the same grants again on submission. `HostCommandAdmission::capabilities`
  exposes what the manifest declared, never what a guest claims.
- **A malformed answer retires; transient state does not.** An unconvertible
  command (`AdapterError::InvalidCommand`) or a batch past the server's own bound
  is a broken plugin and loses its routes; an offline player, a full command queue
  and a full admission ledger drop the batch and keep the instance serving.

Evidence: `mc-plugin-host` 32 tests green, including three new cases - a full
round trip where the player is shown exactly what storage reported, a durability
failure that reaches the plugin as a failure rather than an empty key, and a
package that never declared `storage` being refused and un-routed (with the
route asserted *present* before the event so the test is not vacuous). The
fixture build is now memoized per test process: ten tests in one binary were
racing on the same guest `cargo build` and failing on the package lock.

**P3 second slice: the online-players query, and targeted results.** The same
shape as storage (`list-online-players { request, limit }` ->
`ScriptCommand::ListOnlinePlayers`, answered by `online-players-answered {
request, list<player-snapshot>, truncated }`), with the snapshot renamed from the
server's own DTO: stable identity, session and dimension all come from the
server, nothing is re-derived. Two things this slice proves that storage did
not:

- **A guest's list bound is the plugin's own.** The requested limit travels
  unchanged into `ScriptOnlinePlayersRequest`, which validates it against the
  server's 256 bound, so a plugin cannot ask for an unbounded snapshot.
- **A result addressed to another plugin does not reach this instance.** The
  test reads the very same storage result twice, once with another package as
  the target and once with this one: the first must be silence and the second
  must be answered, so neither half can pass by the host dropping results
  wholesale. This is the targeted-delivery branch that had no user before.

Evidence: `mc-plugin-host` 34 tests green (host_runtime 12), and
`cargo check --workspace --all-targets` exits 0.

**Next: P2's composition-root wiring, `serve()` half.** `serve()` still starts
only the Luau host (`start_prepared_lua_host` :980, `bind_with_scripts` :987,
`join_lua_host` :1229). The component path needs `prepare` (already written as
`component_deployment`) plus `start_deployment` and the same bind/join, and it
depends on P4 for one thing it cannot fake: `serve()` feeds `EffectiveConfig`,
`StartupData` and the Loader manifest from the prepared plugins (worldgen ore
profile, settlement plan, gameplay rules, client bundles), which for a component
deployment come from `configure`'s rule plan and the package's `[client]`
metadata. Wiring `serve()` before that would silently run a deployment with no
startup contribution, so P4's startup half comes first or lands together.

**Next after that: P3.** A package is
`plugin.toml` + `plugin.wasm` (+ optional `config.toml`). Two decisions already
made and to keep: the manifest is parsed with `toml` + `deny_unknown_fields` and
turned into the *existing* `mc_script::ScriptPluginManifest` (one contract, no
second schema), and the `capabilities` names are the ones that already exist in
this repository (the `declare_*` vocabulary the Luau manifest uses), not a new
dotted vocabulary - the plan's `chat.send` sample is illustrative, not the
contract. The host's own contract version is `ScriptApiVersion::new(0, 7, 0)` and
a component whose world differs must be refused at instantiation. Then:
`mc-script` gains a name-to-capability constructor so the vocabulary lives with
the contract, discovery adds strict/expected + grants + `--check`, and P2 wires
the host into `crates/mc-server/src/main.rs` (`prepare_configured_luau_plugins`
:150, start/bind :980-987, reload :1110, join :1229) with the WIT -> ScriptCommand
conversion in `mc-net` (including resolving a stable player id to a live
session).

**Not done yet (P1 onward).** `mc-script` still owns the Luau host: the runtime-
independent extraction (`try_submit_plugin_batch` is `pub(crate)` and cfg-gated,
so no outside host can submit a batch today) and the composition-root rewiring in
`crates/mc-server/src/main.rs` (`prepare_configured_luau_plugins` :150, host
start/bind :980-987, reload :1110, join :1229) are the next seams. P0's API
matrix is done (61 registered `solaris.*` functions: 32 direct command pushers,
29 operation variants, 20 with no first-party consumer, 3 with none at all;
admission runs `ScriptBoundary::accept_host_command` -> `HostAdmissionLedger`;
the only un-admitted routed commands are chat/broadcast/disconnect).

## Post-closeout: what changed after the L2 pass, and the machine bounds (2026-09-16)

**After the passing `correctness` run** (`20260916T051759-correctness-z1ry_tg2`) an
independent reviewer (`Cp003Review`, read-only) returned `changes` with one
material finding: `S.command_issue` had no resident guard, so `/settlement issue`
on a resident whose record carries `handle = "-"` committed a real warehouse bind
before failing, and a resident whose record says `dead`/`released` could really
receive warehouse stock that no command can take back. Fixed in the package
(`S.start_issue` now applies the same `life`/`handle == DONE` guards as
`S.start_job`/`S.start_hire`) with a harness case that rewrites the stored
resident field and proves nothing is bound, plus three smaller fixes: the
selective resident→resident test now reads each endpoint on its own instead of the
concatenated `gear()` view, the `issue` endpoint argument is case-folded like
every other keyword argument, and one refusal comment in
`stage_warehouse_move` now matches the code (`interrupted`, not `missing_input`,
for a full worker endpoint while withdrawing). Falsified by hand: removing the
guards fails `issue_refuses_a_resident_without_a_core_handle_or_a_life`;
restored and re-verified 13/13.

**L2 on the final tree is NOT re-run yet** - the owner directed that heavy gates
wait until the work is done ("сначала ВСЕ СДЕЛАЙ, потом гоняй тесты"), so the last
green `correctness` covers the pre-review-fix tree and the current tree carries
only focused suites (`settlement_lifecycle` 13/13, `resident_order_tests` 16/16,
`resident_settlement_tests` 15/15, `mc-script` DTO 7/7). Re-run
`python3 -m tools.harness run correctness` before any commit.

**Every harness run is now bounded** (owner: a workspace test run was freezing the
desktop). `run`/`client` re-exec into one systemd user scope whose properties every
child inherits: `CPUQuota=600%`, `MemoryHigh=3G`, `MemoryMax=4G`,
`MemorySwapMax=1G`, and `RUST_TEST_THREADS` matched to the quota (`600%` -> 6) so
each concurrent test keeps the CPU share it has unbounded. Measured: 12 spinners
for 4 s consume 45.6 CPU-seconds unbounded, 12.3 at `CPUQuota=300%`, 4.1 at
`100%`; a full `correctness` run peaked at ~2.2 GB inside the scope and never
touched `MemoryHigh`. Knobs (each takes a systemd value or `off`):
`SOLARIS_HARNESS_CPU_QUOTA`, `SOLARIS_HARNESS_MEMORY_HIGH`,
`SOLARIS_HARNESS_MEMORY_MAX`, `SOLARIS_HARNESS_MEMORY_SWAP_MAX`,
`SOLARIS_HARNESS_TEST_THREADS`. Note for the next run: at `CPUQuota=300%` with
unmatched threads the fixed 5 s packet waits in `plugin_examples.rs` and
`commands.rs` fail; at `600%` with matched threads they are the same share as an
unbounded run. `docs/AGENT_TOOLING.md` carries the table.

**The caps failed to protect the session once, on 2026-09-16.** A `correctness` run
under the then-defaults (`5G`/`7G`) while the desktop already held ~11 GiB was
killed by `systemd-oomd`, which acts on the *whole* `user@1000.service` tree's 50%
memory-pressure limit and then picks the largest units in it — the run's scope (10
processes, 15:30:27) *and* an interactive terminal scope (`vte-spawn-…`, 12h CPU,
15:30:24). The kernel OOM killer never fired (`journalctl -k` is empty): a cgroup
cap does not stop oomd, because oomd never looks at the scope's own limit.
Measured afterwards: a full `test` phase peaks at **1.13 GiB** (483 samples of the
scope's `memory.current`), so no run needed the allowance it had — the tree was
already near its limit and the run's share tipped it. Defaults are now
`MemoryHigh=off` (throttling is what feeds oomd: `MemoryHigh` reclaims *inside*
the scope, and that reclaim is the pressure signal) with `MemoryMax=4G` — three
times the measured need — and `run`/`client` refuse to start when `MemAvailable <
MemoryMax + 1 GiB`, naming the knob; a refusal is "the gate did not run", never a
pass. Two operational rules
follow: run one heavy thing at a time (plain `cargo test`/`clippy` outside the
harness get **no** cap, and two concurrent workspace builds is how the machine fell
over earlier in the day), and do not raise `SOLARIS_HARNESS_MEMORY_MAX` while the
desktop is loaded.

## P0 closeout — the host's own memory bound, hostile-guest evidence, and the baseline (2026-09-16, tree dirty, no commit authorization)

**Outcome.** P0's safety gate is closed with running evidence: a package can no
longer make the host allocate what it claims. The plan blocks P2's admission to
the game host on exactly this row ("отказ до неограниченного раскрытия в host
memory; учитывается суммарный объём копий"), and it was open — every limit the
host set bounded the *guest*, and nothing bounded the host's own lifting.

**The hole, measured.** Wasmtime charges a guest→host transfer budget
(`Store::set_hostcall_fuel`) and its default is `2 << 30`
(`wasmtime-36.0.15/src/runtime/component/store.rs:15`, rustdoc `:196-211` calling
it "a DoS mitigation mechanism"); the host never set it. The store limiter cannot
stand in: it bounds the guest's memories/tables/instances, not the `Vec`/`String`
the host builds while lifting an answer. For this contract's answer type the claim
is unbounded in the ways the plan names — `WasmList::new`
(`func/typed.rs:1859`) charges the descriptor array the guest declares, then the
host allocates `size_of::<Command>()` per element, and strings the guest aliases
are each copied whole.

**What landed.** `PluginLimits::hostcall_bytes` (`crates/mc-plugin-host/src/limits.rs`),
default 8 MiB derived from the contract's own admitted maximum (32 commands at
the 8192-byte text bound with the largest storage value each, plus the widest
`configure` plan) and 256x below Wasmtime's default; `store()` sets it
(`src/lib.rs`); the guest fixture grew `oversized`/`wide`/`nested`/`trap`/`recurse`
modes (`sdk/rust/examples/hello/src/lib.rs`); `tests/host_bounds.rs` holds five
cases, each running the *same* guest with the bound as the only difference, so
none can pass by the guest being unable to make the claim — with Wasmtime's
default the claim is copied and only staging refuses it, with the shipped bound
the transfer is refused and the cause is asserted to be the transfer budget.

**Falsification (run, not argued).** With the `set_hostcall_fuel` line removed and
nothing else changed, `one_answer_past_the_transfer_bound_is_never_copied` fails
with `the refusal must be the transfer budget, not guest answer rejected: on-events
returned text past the bound of 8192 bytes` — i.e. the 12 MiB claim was lifted into
host memory first. Line restored, binary green. Full reasoning and the not-proven
list: `.analysis/codex-logs/p0-transfer-bound/README.md`.

**Same checkpoint, agent lanes (each verified on this tree).**

- **Contract refusal** (`tests/contract_refusal.rs`, 2 cases): a component that
  carries the accepted version string but a foreign world is refused at
  instantiation with `HostError::Instantiate` naming the missing import, and the
  same bytes through a lax linker instantiate fine — the linker's type check
  decides, not the encoder (proved with `.validate(false)`). The matched half: an
  unmodified-world dummy component is admitted and its first callback's fault is
  reported as a guest `Trap` with the instance retired. This required a real fix:
  `HostError::Instantiate` was **unreachable** — `PluginInstance::instantiate`
  funnelled every failure through `classify` into `Trap`, so a package that did
  not match the contract was reported as a misbehaving guest
  (`src/instance.rs`). Non-`Trap` instantiation failures now map to `Instantiate`;
  a guest initializer fault stays `Trap`/`Budget`.
- **Baseline** (`tests/host_baseline.rs` + `.analysis/codex-logs/p0-bounds/README.md`):
  one callback p50 11.7 µs / p95 12.5 / p99 17.4 (250 iterations, 1 event, debug,
  i5-12400), fixture compile 4.4 s, process `VmHWM` 8.7 MB → 46 MB for one hosted
  instance. These are a P7 comparison reference, not an acceptance; the fuel and
  memory numbers in `limits.rs` are still **not** calibrated against them.
- **One fixture build** (`tests/fixture/mod.rs`): the guest build and its
  component encoding now exist once, shared by every test binary, instead of four
  copies. `host_runtime` gained a deployment-level case for the unpublished batch
  (a guest that traps is retired and loses the routes it can no longer answer,
  with zero commands submitted).

**Two pre-existing gate blockers on this route, fixed here.** `harness run fmt`
was **red** on 18 files of this route's uncommitted work (the crate was never
formatted; `cargo fmt --all`, no semantic change) and `harness run code-health`
was **red** on `ScriptBatchSubmissionError` missing `#[non_exhaustive]` (fixed,
plus the wildcard policy in `host.rs`: an unknown refusal variant is treated as
backpressure, because retiring an instance is the destructive answer).

**Gates.** `cargo test -p mc-plugin-host`: 43 tests in 8 binaries, 0 failed.
`harness run fmt` PASS `20260916T074511-fmt-xzbh_llj`; `harness run code-health`
PASS `20260916T074611-code-health-8dnybcsy`. L2 `correctness` was **not** run for
this checkpoint.

**Not proven, and not to be implied.** No byte-level measurement of host
allocations during lifting (the obvious instrument is a counting
`#[global_allocator]`, which needs `unsafe impl` and the workspace forbids unsafe
code — the plan says not to weaken that); address aliasing is argued, not
reproduced (a Rust guest cannot alias two live strings; the cumulative charge
covers it); post-return/cleanup hostility; a fault inside a component initializer
(no way to build one with `dummy_module`); AArch64; aggregate instance/table
limits beyond the memory-growth and stack cases.

**Checkpoint state (no commit authorization).**

```yaml
base_tree: b676669c02bcb4b4d5d886aa6f3b19ed2b072f17
diff_hash: 9f04fd7381ad569a7d8c5b96db3240692e7971d14b2848e3317ff4f6d9d29f45
paths: [crates/mc-plugin-host, crates/mc-script/wit, sdk/rust, docs/PLUGINS.md, .gitignore]
validation: [cargo test -p mc-plugin-host 43/0, harness run fmt PASS, harness run code-health PASS]
next: land the two in-flight lanes, then wire serve()
```

The digest is SHA-256 over `git diff -- <paths>` followed by the path and bytes of
each untracked file under those paths, sorted; recompute it the same way (the
recipe is the seven lines this checkpoint used) because the two in-flight lanes
were started after it and will move `crates/mc-plugin-host/src/{lib.rs,check.rs}`
and `crates/mc-net`.

`.gitignore` gained `/sdk/rust/target/` in the same checkpoint: the guest SDK is
its own workspace and its build output was untracked-but-unignored, which made
`git status -uall` report 1376 fixture artifacts. `sdk/rust/Cargo.lock` stays
untracked on purpose — whether the guest lockfile is committed is P7's call (the
plan asks for reproducible fixture builds and says to account for guest lockfiles
separately).

## Composition root: the component host runs in the server (2026-09-16, tree dirty, no commit authorization)

**Outcome.** A `[plugins] runtime = "wasm"` deployment is prepared, started, bound
and stopped by `serve()`, and the rules its `configure` produced are the rules the
world is opened with. The Luau path is unchanged.

**Three lanes, disjoint write sets, one integration owner.**

1. **Startup contribution** (`crates/mc-script/src/gameplay_rules.rs` new,
   `crates/mc-plugin-host/src/startup.rs` new): the startup-rule payload and its
   validator moved out of `mc-script`'s `lua-runtime` gate to the crate root (a
   startup contract is not a Luau detail), and a WIT `rule-plan` now converts into
   it with one typed refusal per field — a value wider than its contract field is
   refused rather than truncated, because a truncated rule set would fingerprint
   as a different world. `PluginHost::contribution()` reports per package
   `NoPlan | Rules | Refused`, and `check_deployment` runs the same conversion and
   validation the run path does, so a plan cannot pass one and fail the other.
   Falsified twice by hand: removing the shared `validate()` fails four cases;
   truncating instead of refusing fails the width case.
2. **Session lookup** (`crates/mc-net/src/server.rs` and the session module):
   `mc_net::PlayerSessionsHandle` keeps no table of its own — it holds a
   `Weak<SessionRegistry>` that `BoundServer::register_player_sessions` publishes,
   and delegates to the registry `list-online-players` already answers from, so a
   plugin's uuid-addressed command reaches the connection that identity holds and
   an offline player is refused instead of addressed by a stale runtime id.
3. **Wiring** (mine, `crates/mc-server/src/main.rs`):
   `prepare_configured_plugins` dispatches on the configured runtime; the component
   deployment is discovered through the *same* `component_deployment` constructor
   `--check` uses (strict/expected/grants unchanged), the host starts **before**
   the world is opened because that is where the rules come from, a refused plan or
   two packages that each declare rules stops startup with the host already
   stopped, `bind_with_scripts` binds the host's own boundary, and the server
   registers the session handle after binding.

**Evidence on this tree.** New mc-server cases: the deployment prepares, starts and
really claims its command (`boundary().player_command_roots() == ["hello"]`); the
rules carry the package's own values; a package with no plan contributes none; two
declarers refuse the deployment naming both. Existing suites: `mc-plugin-host`
49/0 (10 binaries), `mc-script` 130/0 and 267/0 with `lua-runtime`, `mc-net`
session-filtered 605/0 plus the 2 new lookup cases, `mc-server` 237/0 + the 3 new
cases. `harness run fmt` PASS, `harness run code-health` PASS (`0 fail`,
`verdict: KEEP`), workspace `clippy -D warnings` clean (it was **not** clean before
this wave: four findings in this route's crate, three in `mc-plugin-host` and one
in `startup_contribution`'s test binary, all fixed).

**Named limits of this stage, stated rather than implied.** A component deployment
declares no ore profile, settlement plan or client bundle — the WIT contract has no
record for them — so the world contract records what a server with no plugin
directory records and `serve()` logs that fact; SIGHUP reload remains Luau-only
until P6, and a component deployment logs that the reload was ignored rather than
reporting one; `PluginLimits`/`HostQueues` are still the documented defaults (P7
measures them). The fixture-build recipe for the SDK guest now exists in two places
(`crates/mc-plugin-host/tests/fixture/mod.rs` and mc-server's test module); P7 owns
centralizing it in the harness.

**The L2 gate, honestly — red on one pre-existing test, four receipts.** The first
two `harness run correctness` attempts never reached a verdict: `systemd-oomd`
killed the run's scope before its test phase finished (machine protection, not a
test result; see the machine-bounds section). Run with
`SOLARIS_HARNESS_MEMORY_HIGH=off SOLARIS_HARNESS_MEMORY_MAX=10G`, the `test` phase
completed twice — `20260916T084138-test-ct64wtlj` (458.7 s) and
`20260916T085042-test-vz5yi4m0` (241.1 s, scope peak 1.13 GiB) — and both failed on
exactly one test, the same one:
`settlement_pause_repro::settlement_fund_reserves_materials_and_answers_the_player`.
68 of 69 test binaries were green in the second run; `cargo test -p
mc-test-harness --test settlement_pause_repro` passes standalone in 3.83 s, and so
does the same command **inside the identical scope properties** (`systemd-run
--user --scope -p CPUQuota=600% -p MemoryHigh=3G -p MemoryMax=4G -p
MemorySwapMax=1G env RUST_TEST_THREADS=6`), so the quota is not the trigger — the
accumulated load of a full workspace run is. The failure is the signature CP-002
already recorded (`["Unknown command"]`), and its stated residual is the owner:
the metric behind that wall-clock slice. **L2 is therefore not green on this
tree**, nothing in this wave is in that path, and the receipts above are the
evidence rather than a claim that the gate passed.

**Checkpoint state (no commit authorization).**

```yaml
base_tree: b676669c02bcb4b4d5d886aa6f3b19ed2b072f17
diff_hash: aaf3dc156af854655a868cb6814c119b06bc4b0603e7dc155b299d49dbec865a
paths: [crates/mc-plugin-host, crates/mc-script/wit, crates/mc-script/src/gameplay_rules.rs,
        crates/mc-script/Cargo.toml, crates/mc-server, crates/mc-net/src/server.rs,
        sdk/rust, tools/harness/__main__.py, docs/PLUGINS.md, .gitignore]
validation: [mc-plugin-host 49/0, mc-script 130/0 and 267/0, mc-net session 605/0 + 2, mc-server 237/0 + 3 new,
             harness run fmt PASS 20260916T085637, harness run code-health PASS 20260916T085641,
             L2 test phase RED on settlement_pause_repro only]
next: P3, first vertical in flight
```

Recompute it the same way: SHA-256 over `git diff -- <paths>` followed by the path
and bytes of each untracked file under those paths, sorted. `docs/MEMORY.md` is
excluded on purpose (it is this cursor). The P3 wave that starts next moves
`crates/mc-script/wit` and `crates/mc-plugin-host/src/adapter.rs`, so this digest
describes the tree as of this closeout, not the tree in flight.

