# Archived checkpoint log — part 12 of 17

Era: 09-17/09-18. Chronological checkpoint history moved out of `docs/MEMORY.md` so
the live cursor stays small. **Not startup context** (see `AGENTS.md`); read it when
a question is about this era, not to learn the current state.

## Sections in this part

- Checkpoint — P5 accepted (2026-09-18)
- Checkpoint — P6 code complete, full validation interrupted (2026-09-18)
- Checkpoint — P7/P8 component-only cutover (2026-09-18)
- Checkpoint — CP-001 component warehouse hauling (2026-09-18)
- Checkpoint — CP-002 WASM settlement readiness (2026-09-18)
- Checkpoint — CP-003 WASM warehouse issuance (2026-09-18)
- Checkpoint — CP-003 village container source (2026-09-18)
- Checkpoint — CP-004 warehouse stock reservation (2026-09-18)
- Checkpoint — CP-005 atomic construction portions (2026-09-18)
- Checkpoint — CP-006 safe construction lifecycle (2026-09-18)
- Checkpoint — CP-007 atomic resident actions (2026-09-18)
- Checkpoint — architecture-review comprehensibility (2026-09-20)

---

## Checkpoint — P5 accepted (2026-09-18)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

P5's before-build and before-damage WASM hook outcome is accepted. The current
implementation has a single native admission/commit path, ordered handlers,
replacement/cancellation propagation, deadline/reentrancy/failure denial, and
public non-exhaustive ABI types with consumers handling future variants
fail-closed. The final cleanup keeps player-damage admission owned by the
session boundary and reduces `SimulationOwner::process_batch` below the
code-health gateway budget.

The direct P5 rerun first reproduced an intermittent client-mining stall:
`start_sent=true`, with 399 accepted client continuations but no observed block
change. The client-mining result now retains that diagnostic for a future
failure. The next exact real-client rerun passed the complete canonical
`core-client` profile, including server-only natural dirt pickup and direct plus
hooked precommit paths.

P0–P4 audit: P1–P4 retain their historical acceptance evidence. P0's
plan-required Linux AArch64 proof remains unavailable in this workspace and is
not claimed as freshly verified.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: b5f6656c89caa19da814edccbfa114244d187e6d5fa1bf2c71d707eab7cfa7bb
changed_files:
  - crates/mc-script/src/precommit.rs
  - crates/mc-script/src/precommit_tests.rs
  - crates/mc-script/wit/precommit.wit
  - crates/mc-plugin-host/src/discovery.rs
  - crates/mc-plugin-host/src/host.rs
  - crates/mc-plugin-host/src/instance.rs
  - crates/mc-plugin-host/src/lib.rs
  - crates/mc-plugin-host/tests/precommit_host.rs
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/bucket_precommit_tests.rs
  - crates/mc-net/src/play/command_execution.rs
  - crates/mc-net/src/play/player_damage_adapter.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/play/simulation/precommit.rs
  - crates/mc-net/src/play/simulation/queue.rs
  - crates/mc-net/src/play/simulation/tests/precommit_tests.rs
  - crates/mc-net/src/play/tests/player_damage.rs
  - crates/mc-net/src/play/use_item_on_adapter.rs
  - crates/mc-net/src/play/session/damage_precommit.rs
  - crates/mc-net/src/play/session/entity_combat.rs
  - crates/mc-net/src/play/session/explosion_authority.rs
  - crates/mc-net/src/play/session/player_combat.rs
  - crates/mc-net/src/play/session/player_combat/tests/precommit_tests.rs
  - crates/mc-net/src/play/session/player_effects.rs
  - crates/mc-net/src/play/session/player_state.rs
  - crates/mc-net/src/play/session/projectiles.rs
  - crates/mc-net/src/play/session/survival_action_authority.rs
  - crates/mc-net/src/play/session/transactions.rs
  - crates/mc-net/src/play/session/transactions_precommit_tests.rs
  - crates/mc-net/src/play/session/village_defense.rs
  - sdk/rust/examples/hello/src/precommit.rs
  - tools/harness/precommit.py
validation:
  - .analysis/validation/20260918T041516-core-client-o204mnk0/result.json:
      passed complete real-client core-client profile, including P5 direct/hooked
  - .analysis/validation/20260918T023332-core-client-3vk_ache/scenario.json:
      historical first P5 direct/hooked pass; superseded by the final full pass
  - .analysis/validation/20260918T033535-correctness-bze01xms/result.json:
      passed (fmt, code-health, strict workspace Clippy, all workspace targets)
  - P5FinalReview: pass; no findings
next: Assess P6 scope against the owning WASM plan.
```

## Checkpoint — P6 code complete, full validation interrupted (2026-09-18)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

P6 implements strict component deployment reload without replacing the server's
`ScriptBoundary`, player-session handle, network, or world. A candidate is
re-discovered from re-read strict WASM configuration, built in separate stores,
and staged behind the host FIFO. It compares ordered identities, package catalog,
player/operator command roots, payload channels, effective grants, pre-commit
roster, deployment surface, and startup contribution. A static bound rejects
combined old/candidate guest memory before candidate construction. Commit
replaces routes and registrations atomically; old timers begin no new callbacks,
and targeted results admitted under an old registration cannot reach a same-id
replacement. Buffered targeted results still drain during graceful shutdown.

The independent P6 review found two concrete issues: candidate init timers used
tick zero after a live reload, and the new targeted-event fence discarded
already-admitted results during graceful shutdown. Both are fixed: candidates
now use the active simulation tick, while closed admission retains registration
fences until the FIFO drains. The reviewer was not re-run, per the one-review
rule.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 13386dd30f74a6c47f2de1ed6bcd76538fb2ef8cb10e351963c4067bf6ae9ebc
changed_files:
  - crates/mc-plugin-host/Cargo.toml
  - crates/mc-plugin-host/src/host.rs
  - crates/mc-plugin-host/src/limits.rs
  - crates/mc-plugin-host/tests/host_deployment.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-server/src/component_startup_tests.rs
  - crates/mc-server/src/main.rs
  - docs/PLUGINS.md
  - docs/OPERATING.md
  - docs/decisions/0009-regional-plugin-boundary.md
  - docs/MEMORY.md
validation:
  - cargo test -p mc-script targeted_event_admitted_before_replacement_is_not_delivered_afterwards:
      passed before the final shutdown-drain correction
  - cargo test -p mc-script closing_event_admission_drains_buffered_targeted_events:
      passed
  - cargo test -p mc-plugin-host --test host_deployment:
      passed (8 lifecycle/reload cases)
  - cargo test -p mc-server component_reload_from_config_replaces_the_live_generation:
      passed
  - .analysis/validation/20260918T044847-fmt-qfingqa5/result.json:
      passed before the final mechanical reload-context refactor; affected Rust source
      was subsequently formatted directly with rustfmt --edition 2024
  - .analysis/validation/20260918T044854-code-health-p04k8y6j/result.json:
      passed before the final mechanical reload-context refactor
  - .analysis/validation/20260918T044948-clippy-787y668_/result.json:
      passed after the final refactor
  - P6ReloadReview:
      changes requested: timer origin and shutdown drain; both source-backed findings fixed
  - .analysis/validation/20260918T045014-correctness-woiq81bf:
      interrupted during workspace test compilation with exit 241; no result.json written
  - .analysis/validation/20260918T045131-test-c8sb7lsk:
      interrupted during workspace test compilation with exit 241; no result.json written
status: validation-blocked
next: Restore a harness workspace-test run that does not terminate under its 4G scope and writes a receipt, then rerun correctness before closing P6 and starting P7.
```

## Checkpoint — P7/P8 component-only cutover (2026-09-18)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

P7 now deploys five real first-party Rust components: permissions, essentials,
economy, towns, and audit. They run through strict package discovery with the
ordinary component host, retain session-scoped replies and exact TPA correlation,
and have aggregate standard-pack acceptance coverage.

P8 removes the retired Luau path end-to-end: `mc-server` always discovers
components; the `runtime` selector, host module, manifest dual-version default,
Lua dependencies, source fixtures, and current documentation are gone. API
0.7 is the sole package contract. Component migration tests now register a
receiving plugin route before asserting targeted answers. The zone precommit
fixture builds its component before opening the bounded native request, so guest
build work cannot consume the request deadline.

The independent P8 review found four concrete test-cutover defects: unregistered
query result delivery, an obsolete CLI runtime selector, obsolete CLI JSON
expectations, and 0.6 API test expectations. All were fixed and the exact
affected suites passed. The reviewer was not re-run under the one-review rule.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: df99c2a8a71ee354150f14481dc3ca6aa578ec5688f8b070b1dcae10738c0aae
changed_files:
  - Cargo.toml
  - Cargo.lock
  - README.md
  - example.toml
  - docs/PLUGINS.md
  - docs/OPERATING.md
  - docs/ARCHITECTURE.md
  - crates/mc-script/src/lib.rs
  - crates/mc-script/src/gameplay_rules.rs
  - crates/mc-script/src/plugin_metadata.rs
  - crates/mc-script/src/client_view.rs
  - crates/mc-script/wit/events.wit
  - crates/mc-script/wit/world-events.wit
  - crates/mc-script/wit/commands.wit
  - crates/mc-script/wit/lifecycle.wit
  - crates/mc-plugin-host/Cargo.toml
  - crates/mc-plugin-host/src/legacy_luau/
  - crates/mc-plugin-host/src/{adapter,client_bundle,host,lib,limits,package,required_features,startup,timers,world_events,worldgen}.rs
  - crates/mc-plugin-host/tests/{audit_component,component_roundtrip,deployment_discovery,economy_component,essentials_component,first_party_component_pack,host_deployment,host_runtime,loader_operations,package_contract,permissions_component,player_operations,startup_contribution,towns_component}.rs
  - crates/mc-net/Cargo.toml
  - crates/mc-net/src/{play.rs,script/mod.rs,script/router.rs,script/storage.rs,script/zone.rs}
  - crates/mc-net/src/play/{persistence/inventory_recovery_tests.rs,session/entity_lifecycle.rs,session/script_client_sound_endpoint_tests.rs,session/script_client_view_endpoint_tests.rs,session/script_menu_endpoint_tests.rs,simulation/tests/precommit_tests.rs}
  - crates/mc-net/src/script/{inventory_tests,player_query_tests,teleport_tests}.rs
  - crates/mc-server/Cargo.toml
  - crates/mc-server/src/{component_startup_tests,lib,main,structure_rules_tests}.rs
  - crates/mc-server/tests/{cli,play}.rs
  - crates/mc-test-harness/Cargo.toml
  - crates/mc-test-harness/tests/commands.rs
  - sdk/rust/Cargo.toml
  - sdk/rust/solaris-plugin-sdk/src/lib.rs
  - sdk/rust/packages/solaris-{permissions,essentials,economy,towns,audit}/
validation:
  - cargo check -p mc-server -p mc-plugin-host -p mc-net:
      passed
  - cargo test -p mc-plugin-host --test permissions_component --test essentials_component --test economy_component --test towns_component --test audit_component --test first_party_component_pack --test component_roundtrip --test deployment_discovery --test host_deployment --test startup_contribution:
      passed 34 tests
  - cargo test -p mc-plugin-host --test player_operations:
      passed 6 tests
  - cargo test -p mc-server --test play component_plugin_loaded_from_disk_replies_to_join_and_command_over_the_wire:
      passed
  - cargo test -p mc-server --test cli check_:
      passed 43 tests
  - cargo test -p mc-net teleport_adapter_publishes_exact_unavailable_result:
      passed
  - cargo test -p mc-net player_query_adapter_publishes_authoritative_targeted_snapshot:
      passed
  - cargo test -p mc-net changed_zone_snapshot_refuses_kept_placement_without_mutation:
      passed
  - .analysis/validation/20260918T073558-fmt-g6ehae19/result.json:
      passed
  - .analysis/validation/20260918T073602-code-health-xg0kf2_f/result.json:
      passed
  - P8ComponentOnlyReview:
      changes requested; all four source-backed findings fixed
status: P7/P8 complete; P6 and P0 remain externally blocked
next: Implement CP-001 planned implementation and evidence.
```

## Checkpoint — CP-001 component warehouse hauling (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

CP-001 ships `solaris-settlements` as a Rust/WASM component. It persists its
operation counter before issuing work, restores the counter on guest restart,
and validates player input before it can reach host DTO admission. A bind stores
only a matching committed opaque warehouse binding. A haul takes the caller's
exact current resident revision and emits the existing directed
`ResidentCarry -> Warehouse` order; it never accepts a chest coordinate or
warehouse handle from a player.

The focused component scenario builds the real guest, deploys its shipped
manifest, restores the durable counter through host storage callbacks, binds a
real core warehouse, sends its admitted typed haul through the inventory
runtime, and observes the chest moving from 12 to 15 logs while carry empties.
The scenario runs with `NoSessions`, so no online player principal is invented.
Existing core cases prove the same atomic path retains cargo on a full chest,
continues after a rejected mixed stack, and replays one transfer after reopening
durable storage. A refused or unavailable binding remains a typed core refusal;
the guest retains no synthetic empty warehouse and cannot accept a destination
handle or chest coordinate from a player.

The independent CP-001 review found four defects: unbounded player DTO input,
operation-id reuse after guest restart, reanimated completed binding
correlations, and silently upgraded freshness fences. All are fixed. The review
was not rerun under the one-review rule.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 3f437625fdfe3807e464f028abf6b68b24ee0a78a08d0758f841f78ca639c449
changed_files:
  - sdk/rust/Cargo.toml
  - sdk/rust/packages/solaris-settlements/Cargo.toml
  - sdk/rust/packages/solaris-settlements/plugin.toml
  - sdk/rust/packages/solaris-settlements/config.toml
  - sdk/rust/packages/solaris-settlements/src/lib.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - crates/mc-plugin-host/tests/first_party_component_pack.rs
  - docs/MEMORY.md
validation:
  - cargo check -p solaris-settlements-plugin --target wasm32-unknown-unknown:
      passed
  - cargo test -p mc-net --lib settlement_component_binds_warehouse_and_delivers_resident_carry:
      passed
  - cargo test -p mc-net --lib worker_haul_deposits_its_cargo_into_the_bound_warehouse:
      passed
  - cargo test -p mc-net --lib a_full_container_leaves_the_cargo_with_the_worker:
      passed
  - cargo test -p mc-net --lib a_replayed_haul_deposits_once:
      passed after reopening durable storage
  - cargo test -p mc-net --lib a_haul_deposits_past_a_stack_the_container_refuses:
      passed
  - cargo test -p mc-plugin-host --test first_party_component_pack:
      passed
  - .analysis/validation/20260918T081018-fmt-b8eu1_nb/result.json:
      passed
  - .analysis/validation/20260918T081235-code-health-kw0k4bdj/result.json:
      passed
  - CP001SettlementReview:
      changes requested; all four source-backed findings fixed
  - python3 -m tools.harness run correctness:
      not rerun; the active P6 4G workspace-test blocker exits 241 without a
      receipt, so no unavailable L2 pass is claimed
status: CP-001 complete under the existing P6 L2 blocker; P0 and P6 remain externally blocked
next: Verify CP-002 WASM settlement readiness.
```

## Checkpoint — CP-002 WASM settlement readiness (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

The historical `settlement_pause_repro` was a removed Lua-host scenario. Its
old standalone pass is not treated as a fix. The current Rust/WASM
`solaris-settlements` guest instead has one bounded real-component readiness
route: strict package discovery, public deployment through compile,
startup-instantiation/configure/runtime-instantiation/init, the durable
counter callback, then bind and haul completion callbacks. Every command
receipt now has a ten-second failure boundary which identifies the stalled
phase; elapsed phase lines are evidence, not performance limits.

The route passed at rest and under four owned CPU-spinner processes. It reached
its boot and all queue receipts under the latter without changing limits,
epochs, fuel, retries, queue capacity, or a wait budget. The ported real-world
WASM settlement fixture also passed two scenarios concurrently: its full
server/world owner flow and its capability refusal. Separate host controls
passed actual configure/init refusal, queue backpressure, guest CPU exhaustion,
and a guest blocked in a host-native import. Thus current WASM readiness is
observable and did not reproduce the historic failure signature; this is not a
claim that the historical Lua load sensitivity was repaired.

Raw receipts and the independent review are
`.analysis/codex-logs/cp002-wasm-readiness-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
source_hash: 69ac0a676f073ae3426f0b37dd622861b70e6e4c8f78d7b64647124d22e3157a
changed_files:
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib settlement_component_binds_warehouse_and_delivers_resident_carry -- --nocapture:
      passed; actual component readiness receipts recorded
  - four owned CPU spinners + current component scenario:
      passed; raw log recorded
  - RUST_TEST_THREADS=2 cargo test -p mc-test-harness --test wasm_settlement_operations -- --nocapture:
      passed; 2 current WASM settlement scenarios
  - cargo test -p mc-plugin-host --test host_runtime --test startup_lifecycle -- --nocapture:
      passed; 16 readiness-control tests
  - .analysis/validation/20260918T082351-fmt-2vesexe3/result.json:
      passed
  - .analysis/validation/20260918T082400-code-health-gqsmljxh/result.json:
      passed
  - CP002ReadinessReview:
      pass; no source-backed findings
  - python3 -m tools.harness run correctness:
      not rerun; active P6 4G workspace-test blocker remains unresolved
status: CP-002 complete under the existing P6 L2 blocker; P0 and P6 remain externally blocked
next: Implement CP-003 core-issued resident tool supply.
```

## Checkpoint — CP-003 WASM warehouse issuance (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

The native warehouse-to-resident composite already owned binding resolution,
real container slots, revision/capacity/item validation, one journal decision,
and no player principal. CP-003 exposes that existing authority to the shipped
Rust/WASM settlements guest without a new WIT endpoint or inventory authority:
`/settlement issue <resident> <revision> <carry|equipment> [item]` emits only
`Warehouse(binding) -> ResidentCarry/ResidentEquipment` work after durable
operation-counter reservation.

The actual compiled component binds its core-owned warehouse, deposits three
logs, then issues all fifteen logs into resident carry and the resident's iron
axe into equipment. The core chest ends empty; every move has no player
participant and all four component operations have durable receipts. Existing
native checks also pass named carry/equipment withdrawals, missing input, full
carry, unknown warehouse, and exact replay. The guest keeps the caller-supplied
revision; it cannot choose a warehouse handle or a player inventory endpoint.

Raw receipt and independent review:
`.analysis/codex-logs/cp003-wasm-warehouse-issue-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
source_hashes:
  sdk/rust/packages/solaris-settlements/src/lib.rs: 5f53b4d45a0db52ae8d6cab1262d351c9bfc4e3d121cddb3c620086c63fec67e
  crates/mc-net/src/script/storage/resident_settlement_tests.rs: bcdfd6d8813a4531cba495018e929cd89365f0fd32e730c08cef268cf9742d6f
changed_files:
  - sdk/rust/packages/solaris-settlements/src/lib.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib settlement_component_binds_warehouse_and_delivers_resident_carry -- --nocapture:
      passed; real carry and equipment issuance
  - cargo test -p mc-net --lib worker_withdraws -- --nocapture:
      passed; 2 native endpoint tests
  - cargo test -p mc-net --lib a_withdrawal -- --nocapture:
      passed; 3 native refusal tests
  - cargo test -p mc-net --lib a_replayed_withdrawal -- --nocapture:
      passed; one-transfer replay
  - .analysis/validation/20260918T083940-fmt-0ceq9upi/result.json:
      passed
  - .analysis/validation/20260918T083949-code-health-ck8ei13h/result.json:
      passed
  - CP003IssuanceReview:
      pass; no source-backed findings
  - python3 -m tools.harness run correctness:
      not rerun; active P6 4G workspace-test blocker remains unresolved
status: CP-003 warehouse issuance complete; village container-source scope remains open; P0 and P6 remain externally blocked
next: Complete CP-003 village container source before CP-004.
```

## Checkpoint — CP-003 village container source (2026-09-18)

The settlements component now binds a warehouse to one loaded vanilla village
container with `settlement bind-village <site-id> <container-ordinal>`. The
core resolves the selection through the existing generator village inventory
path, verifies it is loaded, and persists the resolved block position as the
binding identity. The ordinal therefore selects a source only at bind time:
later container enumeration changes cannot make the original chest available
to another settlement. Foreign claims, unknown/unloaded sources, and missing
sources are refused. Existing flat authored warehouse receipts deserialize
into the explicit authored source form, so the WIT/source-model migration
preserves durable settlement bindings.

The independent review found and prompted fixes for ordinal-shift duplicate
binding and legacy-receipt decoding. The focused tests exercise both fixes.
Raw evidence: `.analysis/codex-logs/cp003-village-warehouse-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: 8e0932c7e7caf65a843393d78e64a7aaed699d01311768ed3fdd3c7f9ca014fb
source_hashes:
  crates/mc-script/wit/settlements.wit: ff44fd25124f878e23d410a246b22dd540b6178dec42ceee5981e1b7ec72fecf
  crates/mc-script/src/settlement_operations.rs: a2bfe19733b7a81c4df95bbc06b209e572a8b40d366059e183a42649bc0d93a0
  crates/mc-plugin-host/src/domain_settlements.rs: 81c94f1ecc1126c2cc7d2c7645ebcce318063f16e8bffe61cdbb2a0f3a066b96
  crates/mc-net/src/script/storage/settlement.rs: f58f15e91310f5d0e2714f1f634014ab96bf47f5b29121194f33b0e6fa090329
  crates/mc-net/src/script/storage/settlement_tests.rs: 3ba1058f297c74555dab366599c6c4b6e204d6062d85630d2f83d43fc5dc5989
  crates/mc-net/src/settlement.rs: 937c449ce88f372e0dd1a98023297e5b74b6aad7b10859374be42c59ad6bd0e6
  sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs: fee463c46fe40b2e0c77949c5e8b2e32563d5494f01d973d0d359085fc12b248
  sdk/rust/packages/solaris-settlements/src/lib.rs: 5036f847eb0f3883a816bbcaa8de83ab47b0db26dc4caa24e02be0eb729a66b9
changed_files:
  - crates/mc-script/wit/settlements.wit
  - crates/mc-script/src/settlement_operations.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-plugin-host/src/domain_settlements.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - crates/mc-net/src/settlement.rs
  - sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs
  - sdk/rust/packages/solaris-settlements/src/lib.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-script warehouse_binding_and_handle_are_bounded_and_owner_scoped -- --nocapture:
      passed; legacy flat receipt migration
  - cargo test -p mc-net --lib village_warehouse_binds_one_materialized_position_without_rebinding_its_ordinal -- --nocapture:
      passed; exact source identity across ordinal shifts
  - cargo test -p mc-net --lib settlement_component_binds_warehouse_and_delivers_resident_carry -- --nocapture:
      passed; compiled component compatibility
  - .analysis/validation/20260918T091802-fmt-0kfrdzhp/result.json:
      passed
  - .analysis/validation/20260918T091810-code-health-xfjenx9c/result.json:
      passed
  - CP003VillageSourceReview:
      changes applied; no second review per repository policy
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-003 complete; P0 and P6 remain externally blocked
next: Begin CP-004 against the existing warehouse reservation authority.
```

## Checkpoint — CP-004 warehouse stock reservation (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

Construction now reserves quantities from the exact bound vanilla warehouse
container rather than promising a material plan from stale snapshot data. The
physical reservation floor is shared by all three chest mutation authorities:
the ordinary regional client route, the fallback route, and server-owned
resident warehouse transfers. A player or resident transfer that would reduce
a reserved item below its aggregate outstanding quantity is rejected and
resynchronized. Reservation admission and every chest mutation use one
non-blocking gate, so a successful reservation cannot race a previously
checked chest withdrawal.

Reservation recovery rebuilds floors from durable reservations. An unloaded
but otherwise valid bound source retains its durable position floor until it
can be resolved again; missing, invalid, or destroyed sources contribute no
new live floor and further reservation requests refuse. The focused recovery
test closes two projects against twelve logs, rejects an overpromise and a
destroyed source, restarts while the source is unloaded, then confirms the
recovered floor still rejects an eleven-log after-image.

Raw receipt and independent review:
`.analysis/codex-logs/cp004-warehouse-reservations-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: 9fa847e3dcd485f155eccc88ce9525805e4517215f08d9e023e1cd40a3d30728
source_hashes:
  crates/mc-net/src/play/session.rs: 70e5a69b7ab22edc16eef6a116e220f87cdd4b185aadda20f9888ad57e653a6b
  crates/mc-net/src/play/session/container_views.rs: 602f01952d2e1606e66fc37edac1089eda0722e27dae6718f3fdfde97586416b
  crates/mc-net/src/play/session/transactions.rs: d05345faf8645f9f222c48bb5c5cd7ddaf9c8ee1380a0fef9ebedad0052800b8
  crates/mc-net/src/play/session/tests.rs: 0e699dea8e462932bcb226acd139890b912eb92fd4c8e3fbb21ef16a8ddb09aa
  crates/mc-net/src/play/simulation.rs: 73910f5359f650e90be078f0025c8238155c4a990b984319a445f4127a63ca7d
  crates/mc-net/src/script/storage.rs: 7eeef7f9dc6eb4f920c78379ecf4f3de5a03d202608fdb4141f9fe392ec98e38
  crates/mc-net/src/script/storage/world_inventory.rs: 1e13993c36470870bf2cb70d093e83fe12855894fc5e6953af7b22040c4eecb7
  crates/mc-net/src/script/storage/settlement.rs: 301b1e638e6edb7a0b8f5e40269841b988b88a041b49374cef979436d13f16dc
  crates/mc-net/src/script/storage/settlement_tests.rs: acd84769c6a02fac7a16cd5205afc965e107e0890d127d7318d8fd2125de92c9
changed_files:
  - crates/mc-net/src/play/session.rs
  - crates/mc-net/src/play/session/container_views.rs
  - crates/mc-net/src/play/session/transactions.rs
  - crates/mc-net/src/play/session/tests.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/script/storage.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - crates/mc-net/src/script/storage/world_inventory.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib warehouse_reservation -- --nocapture:
      passed; five reservation admission, physical floor, unload/restart, fallback, and server-owned tests
  - cargo test -p mc-net --lib regional_chest_commit_resyncs_when_a_warehouse_floor_would_be_consumed -- --nocapture:
      passed; ordinary regional client chest route
  - cargo test -p mc-net --lib owned_reservation_blocks_a_drawdown_of_the_reserved_quantity -- --nocapture:
      passed; durable reservation projection
  - cargo test -p mc-net --lib warehouse_transfer_commits_both_participants_under_one_decision -- --nocapture:
      passed; existing receipt-backed composite path
  - .analysis/validation/20260918T095754-fmt-o75at0xe/result.json:
      passed
  - .analysis/validation/20260918T095759-code-health-axcu3j07/result.json:
      passed
  - CP004ReservationReview:
      changes fixed; no second review per repository policy
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-004 complete; P0 and P6 remain externally blocked
next: Begin CP-005 from the active plugin route.
```

## Checkpoint — CP-005 atomic construction portions (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

Construction no longer applies blocks before separately committing progress. One
receipt-bearing `ApplyBlockEdits` regional decision conditionally changes and
stamps the finite portion, persists its chunk after-images with the encoded
prepared settlement batch, then publishes. The storage projection and
`mark_inventory_projected` run only after that decision id returns. Recovery
decodes that same generic decision batch before admitting new work.

Receipt portions always acquire exact block mutation preconditions even without
a `before-build` handler. Immediately before regional mutation, the worker
rechecks its zone fence and consumes any current build approval. Ordinary
server-owned block edits retain the canonical staged lane. The structure's next
footprint revision is predicted from the post-portion image, so its own commit
does not pause subsequent portions while a foreign in-footprint edit still does.

The new recovery test injects a storage append fault after the world decision,
reopens the world journal, projects the receipt once, and proves replay submits
no second portion. A competing advance against the consumed structure revision
is refused before a second block or reservation consumption. The regional-path
precommit test cancels a receipt portion, leaves the block unchanged, and closes
its reserved journal id without an after-image or recoverable receipt.

ADR authority/persistence ordering:
`docs/decisions/0004-staged-single-writer-simulation.md`.
Raw receipt and review evidence:
`.analysis/codex-logs/cp005-atomic-structure-portions-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: abbc21e65d81a1cb454a5bd9522594073761344a613f6f1ee0ac8081b1b6b99d
source_hashes:
  crates/mc-net/src/settlement.rs: 26a6caf59c608ed98cb7f2d57c30492965d409d745421c72c02f54b0ec2275fd
  crates/mc-net/src/play/simulation.rs: b71618e7a522552c093dea0a7671ebb08c3ba3409ddd70cd25604bf9ffc61a1c
  crates/mc-net/src/play/simulation/regional_mutation.rs: e4f3c03b06cd341c6e51f69d2b7fc0f028962d6e188a9c2ccb883905d9f07426
  crates/mc-net/src/play/simulation/tests/precommit_tests.rs: 3d60b39d786047ede41c2dd30664994177e307fab86a0c123f11d3e28f240dd2
  crates/mc-net/src/play/world_inventory_journal.rs: 21edf2987b46d3ec0b740a7175796c33909e5f85074787a43a6f909f0c89c33e
  crates/mc-net/src/script/storage/settlement.rs: b81b2df1559a167a5a70aa60b908baf5d2974b7cbcadb19f139a48bb9289ecd8
  crates/mc-net/src/script/storage/world_inventory.rs: 6b0d6a37723113740e5af6ee9f57581432d0f0f88d1a71bfd6c0f73fa517d529
  crates/mc-net/src/script/storage/settlement_tests.rs: c47938e7d82cbdbdd48928dae2b4dc27226166ff2a1b7a4b210eb201ad7fc128
  crates/mc-net/src/script/storage/resident_settlement_tests.rs: 04e066aba548f5b038fcb10d6071310d02e8a1ed9d5ca2b28e5e004abb645681
  docs/decisions/0004-staged-single-writer-simulation.md: 4860c3456c06579f52fdc14a7c05e5ccc85a0430787199cb751d8a83d136b47b
changed_files:
  - crates/mc-net/src/settlement.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/play/simulation/regional_mutation.rs
  - crates/mc-net/src/play/simulation/tests/precommit_tests.rs
  - crates/mc-net/src/play/world_inventory_journal.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/world_inventory.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - docs/decisions/0004-staged-single-writer-simulation.md
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib cancelled_journaled_structure_portion_keeps_blocks_and_receipt_uncommitted -- --nocapture:
      passed; regional worker consumes the cancelled build approval before mutation and closes an empty decision
  - cargo test -p mc-net --lib structure_portion -- --nocapture:
      passed; regional receipt decision and append-failure recovery
  - cargo test -p mc-net --lib competing_structure_portions_admit_only_one_revision -- --nocapture:
      passed; competing builders share one structure revision fence
  - cargo test -p mc-net --lib advance_ -- --nocapture:
      passed; advance, replay, foreign footprint, and reservation-plan coverage
  - cargo test -p mc-net --lib construct_work_consumes_the_reserved_portion_and_commits_the_stage -- --nocapture:
      passed; resident construction consumes one durable reservation portion
  - .analysis/validation/20260918T103644-fmt-_0o21unu/result.json:
      passed
  - .analysis/validation/20260918T103648-code-health-1purrfh7/result.json:
      passed
  - CP005Advisor and CP005SecondPOV:
      material routing finding fixed and refusal branch covered; reservation-fold concern disproved by storage receipt indexing and focused tests; no second review per repository policy
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-005 complete; P0 and P6 remain externally blocked
next: Begin CP-006 from the active plugins route.
```

## Checkpoint — CP-006 safe construction lifecycle (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

`resume-structure` is now a durable WIT/DTO/host/SDK operation. It only accepts
the owner’s exact paused revision after the authoritative footprint still
matches its last accepted image and any bound reservation remains compatible and
unreleased. It restores `Prepared` before a first portion or `Running` after a
partial build. A changed or unavailable footprint refuses without recording a
new operation; callers must reread authoritative status and do not recreate an
already accepted portion.

Cancellation remains terminal. It never removes built blocks, retained receipt
consumption stays spent, and the reservation fold returns only its unspent
quantities. The lifecycle test covers pause/resume before and after a portion,
foreign-footprint refusal, partial cancellation, and repeated cancellation.

Raw receipt and review evidence:
`.analysis/codex-logs/cp006-construction-lifecycle-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: d93b19916a0256ee9211363d882080f161e690ad3c9c3f3ed7a1d67b563946d4
source_hashes:
  crates/mc-script/wit/settlements.wit: cdea625d81f2a9f1a4da5e464498a18873a4f964d056e03480d0942d6a1f8ecf
  crates/mc-script/src/settlement_operations.rs: f54c869bcc8f2fa60f3b9285b62d808f157f50af2920c46998cdfefae8aa2308
  crates/mc-script/src/lib.rs: 44397c2f2fb52e3d2b0509717f9f95ccf4a7763c870806d92c7c4748f4059b87
  crates/mc-plugin-host/src/domain_settlements.rs: 2df81af87fefd6d43d018ce5cf991f4c22c79321a57936715f434070a874ae77
  sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs: 8312eaec0204551f726b301bdd2eae006ae365e5529466c445e4a5f667741592
  crates/mc-net/src/script/storage/settlement.rs: 7e951cf2ae40e00cc6b7619b0560f0408ed430095136b1e35822df6c13b86ff6
  crates/mc-net/src/script/storage/settlement_tests.rs: 8a695a432fcfe73bb401e1853acd53c4b4ff7c18dd1408d29071f9cce0e897cc
changed_files:
  - crates/mc-script/wit/settlements.wit
  - crates/mc-script/src/settlement_operations.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-plugin-host/src/domain_settlements.rs
  - sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib pause_resume_requires_the_accepted_footprint_and_releases_only_unspent_materials -- --nocapture:
      passed; prepared/running resume, foreign footprint, partial cancel, and repeat-cancel coverage
  - cargo test -p mc-net --lib prepare_builds_nothing_and_cancel_preserves_built_blocks -- --nocapture:
      passed; cancel before first and after partial portion
  - cargo test -p mc-net --lib advance_replays_after_reopen_without_a_second_portion -- --nocapture:
      passed; restart/replay between accepted portion and caller replay
  - cargo test -p mc-script --lib operation_id_is_exposed_only_for_idempotent_mutations -- --nocapture:
      passed; durable resume DTO identity
  - cargo test --manifest-path sdk/rust/solaris-plugin-sdk/Cargo.toml --lib -- --nocapture:
      passed; updated guest contract compiles
  - .analysis/validation/20260918T104617-fmt-pjh_t_3b/result.json:
      passed
  - .analysis/validation/20260918T104621-code-health-imgrvfsq/result.json:
      passed
  - CP006LifecycleReview:
      pass; no concrete lifecycle or conversion findings
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-006 complete; P0 and P6 remain externally blocked
next: Begin CP-007 from the active plugins route.
```

## Checkpoint — CP-007 atomic resident actions (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

Harvest, mine and cut-tree work now preview its exact source state/token and
canonical loot, proves cargo capacity before a world change, then makes the
block after-image and prepared resident cargo/tool/progress batch one
receipt-bearing regional decision. Storage projection follows the decision id;
recovery installs its single receipt before replay, so an append failure cannot
lose or duplicate resident loot.

A stale-replacement adapter changes a crop after preview and proves the
conditional decision leaves the new block, carry and work watermark untouched.
One work decision remains in one regional owner lane. Crop and ore breaks do
not schedule leaf propagation and therefore admit a valid edge cell through the
journaled path; logs retain their leaf scheduling.

The shared structure and resident commit helpers now size/compact the projection
before asking the world to commit. A quota refusal therefore happens while the
source blocks and prepared resident receipt are both still uncommitted.

ADR authority/persistence ordering:
`docs/decisions/0004-staged-single-writer-simulation.md`.
Raw receipt and review evidence:
`.analysis/codex-logs/cp007-atomic-resident-actions-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: 6eefda196ca21adabd8a7e11dbd3164716fea45d92b1802d7c650c56b811332d
changed_files:
  - crates/mc-net/src/play/resident_work.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/play/simulation/regional_mutation.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - crates/mc-net/src/script/storage/world_inventory.rs
  - crates/mc-net/src/server.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-script/src/settlement_operations.rs
  - docs/decisions/0004-staged-single-writer-simulation.md
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib replaced_crop_after_preview_does_not_create_cargo_or_work_progress -- --nocapture:
      passed; replacement between preview and decision preserves the replacement and creates neither cargo nor work progress
  - cargo test -p mc-net --lib harvest_ -- --nocapture:
      passed; canonical crop cargo, storage-fault recovery, and owner-edge harvest
  - cargo test -p mc-net --lib mined_ore_reaches_the_worker_cargo -- --nocapture:
      passed
  - cargo test -p mc-net --lib cut_tree_commits_the_log_and_worker_cargo_together -- --nocapture:
      passed
  - cargo test -p mc-net --lib a_full_worker_reports_no_storage_and_leaves_the_crop_standing -- --nocapture:
      passed
  - cargo test -p mc-net --lib structure_portion -- --nocapture:
      passed; construction composite regression after shared projection-admission ordering
  - cargo test -p mc-net --lib resident -- --nocapture:
      passed; 75 resident-focused tests
  - .analysis/validation/20260918T115820-fmt-v03_h9s8/result.json:
      passed
  - .analysis/validation/20260918T115824-code-health-_8llogyb/result.json:
      passed
  - CP007AtomicReview:
      two material findings fixed; no second review per repository policy
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-007 complete; P0 and P6 remain externally blocked
next: Begin CP-008 from the active plugins route.
```

## Checkpoint — architecture-review comprehensibility (2026-09-20)

Accepted boundaries from
`.rpiv/artifacts/architecture-reviews/2026-09-20_18-44-21_solaris-model-comprehensibility.md`
are now localized without changing their public contracts: server configuration
and startup composition, supervisor checkpoint/physics paths, named play
ingress handlers, simulation/storage/entity test contracts, and the native
script manifest/capability domain. `CommandBatch` remains in the root
admission boundary; `manifest.rs` owns capability declarations, manifest DTOs,
validation and capability matching.

```yaml
base_tree: 1d39e864b291eb3e8c45ba5081b6ce10d40e4c46
implementation_diff_hash: c5c6c2d59ca57c265ab3036dce118d3fa0e38e6d450714a36e15fe877db20b28
changed_files:
  - AGENTS.md
  - README.md
  - crates/mc-net/src/{lib.rs,play.rs,play/session.rs,play/simulation.rs,play/simulation/,play/ingress.rs,play/ingress/,server.rs,server/checkpoint.rs,server/entity_physics.rs,server/tests.rs,server/tests/}
  - crates/mc-server/src/{lib.rs,main.rs,config/,startup.rs,startup/,main_tests.rs,main_tests/,access_control_file_tests.rs}
  - crates/mc-world/src/{storage.rs,storage/tests.rs,storage/tests/}
  - crates/mc-entity/src/{lib.rs,tests.rs}
  - crates/mc-script/src/{lib.rs,manifest.rs}
  - docs/MEMORY.md
validation:
  - .analysis/validation/20260920T151433-test-l2f2ivvk/result.json: mc-script passed
  - .analysis/validation/20260920T151212-test-poeyxry7/result.json: mc-entity passed
  - .analysis/validation/20260920T150433-test-g6e_mxch/result.json: mc-world passed
  - .analysis/validation/20260920T151456-test-h25y1lui/result.json: mc-server passed
  - .analysis/validation/20260920T152602-fmt-108inf37/result.json: formatter passed
  - .analysis/validation/20260920T153324-clippy-biaqd8wa/result.json: strict workspace Clippy passed
  - .analysis/validation/20260920T153411-code-health-o5ty7qc6/result.json: code-health passed
  - .analysis/validation/20260920T154043-correctness-7bw83l4v/result.json:
      formatter, strict workspace Clippy, and code-health passed; workspace tests failed only at the recorded precommit assertion (2298 passed, 8 ignored)
  - codegraph sync .: up to date
  - ArchitectureExtractionReview: pass; no extraction-attributable regression found
status: implementation checkpoint complete; L2 test stage blocked by the recorded mc-net precommit assertion; no commit authorized
next: triage the recorded mc-net precommit failure before treating the full workspace gate as green
```
